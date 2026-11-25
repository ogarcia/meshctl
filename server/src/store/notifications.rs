// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Notifications: whether they are on and show what messages say, among the
//! preferences; and where messages are notified of, each place as an
//! automatic reply's, a channel by the conversation it is.

use super::Store;
use super::auto_replies::{PlaceRow, place_rows};
use anyhow::Result;
use meshctl_api::{AutoReplyPlace, NotificationPlace, RadioId};
use serde::{Deserialize, Serialize};

/// What is kept among the preferences: nothing until set.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    show_text: Option<bool>,
    /// Whether the places were ever set: none set is every radio's direct
    /// conversations, none at all once set.
    #[serde(default)]
    places_set: bool,
}

/// Notifications as kept: what was never set, none.
#[derive(Debug, PartialEq)]
pub struct KeptNotifications {
    pub enabled: Option<bool>,
    pub show_text: Option<bool>,
    pub places: Option<Vec<NotificationPlace>>,
}

/// A place, as told: its radio and node, its channel's slot now,
/// conversation and name, and whether only mentions.
type PlaceTold = (
    RadioId,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    bool,
);

impl Store {
    pub async fn notifications(&self) -> Result<KeptNotifications> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM preference WHERE key = 'notifications'")
                .fetch_optional(&self.pool)
                .await?;
        let kept: Kept = row
            .and_then(|(value,)| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        let places = if kept.places_set {
            // Each channel with the slot it is in now, the first if in more,
            // and its name.
            let rows: Vec<PlaceTold> = sqlx::query_as(
                "SELECT p.radio_id, p.node,
                        (SELECT min(s.slot) FROM channel_slot s WHERE s.channel_id = p.channel_id),
                        p.channel_id, c.name, p.mentions
                    FROM notification_place p LEFT JOIN channel c ON c.id = p.channel_id
                    ORDER BY p.radio_id, p.channel_id IS NULL DESC, p.node, p.channel_id",
            )
            .fetch_all(&self.pool)
            .await?;
            Some(
                rows.into_iter()
                    .map(|(radio, node, slot, channel_id, name, mentions)| {
                        let place = match (node, channel_id) {
                            (Some(node), _) => AutoReplyPlace::Node {
                                radio,
                                node: node as u32,
                            },
                            (None, Some(_)) => AutoReplyPlace::Channel {
                                radio,
                                channel: slot.and_then(|slot| u32::try_from(slot).ok()),
                                name,
                            },
                            (None, None) => AutoReplyPlace::Directs { radio },
                        };
                        let mentions = mentions && matches!(place, AutoReplyPlace::Channel { .. });
                        NotificationPlace { place, mentions }
                    })
                    .collect(),
            )
        } else {
            None
        };
        Ok(KeptNotifications {
            enabled: kept.enabled,
            show_text: kept.show_text,
            places,
        })
    }

    /// Sets the notifications, their places among them; `false`, and
    /// nothing changed, if a place is on a radio, a channel or a node there
    /// is not. A place on a channel in no slot now, as it was told, is kept
    /// as it was.
    pub async fn set_notifications(
        &self,
        enabled: bool,
        show_text: bool,
        places: &[NotificationPlace],
    ) -> Result<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        // Its channels in no slot, by name, to keep those still named.
        let away: Vec<(RadioId, i64, String)> = sqlx::query_as(
            "SELECT p.radio_id, p.channel_id, c.name
            FROM notification_place p JOIN channel c ON c.id = p.channel_id
            WHERE NOT EXISTS (SELECT 1 FROM channel_slot s WHERE s.channel_id = p.channel_id)",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut rows: Vec<(PlaceRow, bool)> = Vec::with_capacity(places.len());
        for place in places {
            let Some(found) =
                place_rows(&mut tx, std::slice::from_ref(&place.place), &away).await?
            else {
                return Ok(false);
            };
            let mentions = place.mentions && matches!(place.place, AutoReplyPlace::Channel { .. });
            for row in found {
                if !rows.iter().any(|(kept, _)| *kept == row) {
                    rows.push((row, mentions));
                }
            }
        }
        sqlx::query("DELETE FROM notification_place")
            .execute(&mut *tx)
            .await?;
        for ((radio, channel, node), mentions) in rows {
            sqlx::query(
                "INSERT INTO notification_place (radio_id, channel_id, node, mentions)
                VALUES (?, ?, ?, ?)",
            )
            .bind(radio)
            .bind(channel)
            .bind(node)
            .bind(mentions)
            .execute(&mut *tx)
            .await?;
        }
        let kept = Kept {
            enabled: Some(enabled),
            show_text: Some(show_text),
            places_set: true,
        };
        sqlx::query(
            "INSERT INTO preference (key, value) VALUES ('notifications', ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(serde_json::to_string(&kept)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{Slot, fingerprint};
    use meshctl_api::{RadioKind, RadioSettings};

    #[tokio::test]
    async fn notifications_keep_their_places_as_auto_replies_do() {
        let store = Store::in_memory().await;
        // Never set: nothing kept.
        assert_eq!(
            store.notifications().await.unwrap(),
            KeptNotifications {
                enabled: None,
                show_text: None,
                places: None,
            }
        );
        let radio = store
            .add_radio(&RadioSettings {
                name: "Casa".into(),
                kind: RadioKind::Meshtastic,
                address: "192.0.2.1".into(),
                enabled: false,
                shown: true,
                badge: Default::default(),
            })
            .await
            .unwrap()
            .id;
        let slot = |index: u32, name: &str| Slot {
            index,
            fingerprint: fingerprint(name, &[1]),
            name: name.into(),
        };
        store
            .follow_channels(radio, &[slot(0, "LongFast"), slot(1, "Galicia")], 10)
            .await
            .unwrap();
        let places = vec![
            NotificationPlace {
                place: AutoReplyPlace::Channel {
                    radio,
                    channel: Some(1),
                    name: None,
                },
                mentions: false,
            },
            NotificationPlace {
                place: AutoReplyPlace::Channel {
                    radio,
                    channel: Some(0),
                    name: None,
                },
                mentions: true,
            },
            // Mentions mean nothing but on a channel.
            NotificationPlace {
                place: AutoReplyPlace::Directs { radio },
                mentions: true,
            },
        ];
        assert!(store.set_notifications(true, false, &places).await.unwrap());
        let galicia = |channel: Option<u32>| NotificationPlace {
            place: AutoReplyPlace::Channel {
                radio,
                channel,
                name: Some("Galicia".into()),
            },
            mentions: false,
        };
        let longfast = NotificationPlace {
            place: AutoReplyPlace::Channel {
                radio,
                channel: Some(0),
                name: Some("LongFast".into()),
            },
            mentions: true,
        };
        let directs = NotificationPlace {
            place: AutoReplyPlace::Directs { radio },
            mentions: false,
        };
        let kept = store.notifications().await.unwrap();
        assert_eq!(kept.enabled, Some(true));
        assert_eq!(kept.show_text, Some(false));
        assert_eq!(
            kept.places.unwrap(),
            [directs.clone(), longfast.clone(), galicia(Some(1))]
        );

        // Taken off the radio: in no slot, and kept when set again as told.
        store
            .follow_channels(radio, &[slot(0, "LongFast")], 20)
            .await
            .unwrap();
        let told = store.notifications().await.unwrap().places.unwrap();
        assert_eq!(told[2], galicia(None));
        assert!(store.set_notifications(false, true, &told).await.unwrap());
        store
            .follow_channels(radio, &[slot(0, "LongFast"), slot(3, "Galicia")], 30)
            .await
            .unwrap();
        assert_eq!(
            store.notifications().await.unwrap().places.unwrap()[2],
            galicia(Some(3))
        );

        // A radio there is not: nothing changed.
        let wrong = [NotificationPlace {
            place: AutoReplyPlace::Directs { radio: radio + 1 },
            mentions: false,
        }];
        assert!(!store.set_notifications(true, true, &wrong).await.unwrap());
        assert_eq!(store.notifications().await.unwrap().enabled, Some(false));

        // Set to none: none, not every radio's direct conversations.
        assert!(store.set_notifications(true, true, &[]).await.unwrap());
        assert_eq!(store.notifications().await.unwrap().places, Some(vec![]));
    }
}
