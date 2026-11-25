// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The automatic replies, in the order they are tried in, and where each
//! answers: a channel by the conversation it is, wherever it is on its radio.

use super::Store;
use anyhow::Result;
use meshctl_api::{
    AutoReply, AutoReplyId, AutoReplyLimits, AutoReplyMatch, AutoReplyPlace, AutoReplySettings,
    BROADCAST, RadioId,
};
use sqlx::SqliteConnection;
use std::collections::HashMap;

/// What came of keeping an automatic reply.
#[derive(Debug, PartialEq)]
pub enum Keeping {
    Kept(AutoReply),
    /// A place on a radio, a channel or a node there is not.
    Invalid,
    NotFound,
}

/// A place, as kept: its radio, and its channel's conversation or its node.
pub(super) type PlaceRow = (RadioId, Option<i64>, Option<i64>);

/// A place, as told: its reply, radio and node, and its channel's slot now,
/// conversation and name.
type PlaceTold = (
    AutoReplyId,
    RadioId,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

/// An automatic reply, as kept, but for its places.
type ReplyRow = (
    AutoReplyId,
    String,
    bool,
    String,
    String,
    bool,
    bool,
    Option<String>,
    Option<String>,
);

fn matching_name(matching: AutoReplyMatch) -> &'static str {
    match matching {
        AutoReplyMatch::Whole => "whole",
        AutoReplyMatch::Word => "word",
    }
}

impl Store {
    /// Every automatic reply, in order, with its places.
    pub async fn auto_replies(&self) -> Result<Vec<AutoReply>> {
        let mut conn = self.pool.acquire().await?;
        let rows: Vec<ReplyRow> = sqlx::query_as(
            "SELECT id, name, enabled, triggers, matching, react, sign, reply, direct_reply
            FROM auto_reply ORDER BY position, id",
        )
        .fetch_all(&mut *conn)
        .await?;
        // Each channel with the slot it is in now, the first if in more, and
        // its name.
        let places: Vec<PlaceTold> = sqlx::query_as(
            "SELECT p.auto_reply_id, p.radio_id, p.node,
                    (SELECT min(s.slot) FROM channel_slot s WHERE s.channel_id = p.channel_id),
                    p.channel_id, c.name
                FROM auto_reply_place p LEFT JOIN channel c ON c.id = p.channel_id
                ORDER BY p.radio_id, p.channel_id IS NULL DESC, p.node, p.channel_id",
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut by_reply: HashMap<AutoReplyId, Vec<AutoReplyPlace>> = HashMap::new();
        for (reply, radio, node, slot, channel_id, name) in places {
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
            by_reply.entry(reply).or_default().push(place);
        }
        Ok(rows
            .into_iter()
            .map(
                |(id, name, enabled, triggers, matching, react, sign, reply, direct_reply)| {
                    AutoReply {
                        id,
                        settings: AutoReplySettings {
                            name,
                            enabled,
                            triggers: serde_json::from_str(&triggers).unwrap_or_default(),
                            matching: if matching == "word" {
                                AutoReplyMatch::Word
                            } else {
                                AutoReplyMatch::Whole
                            },
                            react,
                            sign,
                            reply,
                            direct_reply,
                            places: by_reply.remove(&id).unwrap_or_default(),
                        },
                    }
                },
            )
            .collect())
    }

    async fn auto_reply(&self, id: AutoReplyId) -> Result<Option<AutoReply>> {
        Ok(self.auto_replies().await?.into_iter().find(|r| r.id == id))
    }

    /// Adds an automatic reply, the last.
    pub async fn add_auto_reply(&self, settings: &AutoReplySettings) -> Result<Keeping> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let (id,): (AutoReplyId,) = sqlx::query_as(
            "INSERT INTO auto_reply (position, name, enabled, triggers, matching, react, sign,
                reply, direct_reply)
            VALUES ((SELECT ifnull(max(position), 0) + 1 FROM auto_reply), ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING id",
        )
        .bind(&settings.name)
        .bind(settings.enabled)
        .bind(serde_json::to_string(&settings.triggers)?)
        .bind(matching_name(settings.matching))
        .bind(settings.react)
        .bind(settings.sign)
        .bind(&settings.reply)
        .bind(&settings.direct_reply)
        .fetch_one(&mut *tx)
        .await?;
        let Some(places) = place_rows(&mut tx, &settings.places, &[]).await? else {
            return Ok(Keeping::Invalid);
        };
        put_places(&mut tx, id, &places).await?;
        tx.commit().await?;
        Ok(self
            .auto_reply(id)
            .await?
            .map_or(Keeping::NotFound, Keeping::Kept))
    }

    /// Changes an automatic reply. A place on a channel in no slot now, as
    /// it was told, is kept as it was.
    pub async fn set_auto_reply(
        &self,
        id: AutoReplyId,
        settings: &AutoReplySettings,
    ) -> Result<Keeping> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query(
            "UPDATE auto_reply SET name = ?, enabled = ?, triggers = ?, matching = ?, react = ?,
                sign = ?, reply = ?, direct_reply = ?
            WHERE id = ?",
        )
        .bind(&settings.name)
        .bind(settings.enabled)
        .bind(serde_json::to_string(&settings.triggers)?)
        .bind(matching_name(settings.matching))
        .bind(settings.react)
        .bind(settings.sign)
        .bind(&settings.reply)
        .bind(&settings.direct_reply)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(Keeping::NotFound);
        }
        // Its channels in no slot, by name, to keep those still named.
        let away: Vec<(RadioId, i64, String)> = sqlx::query_as(
            "SELECT p.radio_id, p.channel_id, c.name
            FROM auto_reply_place p JOIN channel c ON c.id = p.channel_id
            WHERE p.auto_reply_id = ?
                AND NOT EXISTS (SELECT 1 FROM channel_slot s WHERE s.channel_id = p.channel_id)",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let Some(places) = place_rows(&mut tx, &settings.places, &away).await? else {
            return Ok(Keeping::Invalid);
        };
        sqlx::query("DELETE FROM auto_reply_place WHERE auto_reply_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        put_places(&mut tx, id, &places).await?;
        tx.commit().await?;
        Ok(self
            .auto_reply(id)
            .await?
            .map_or(Keeping::NotFound, Keeping::Kept))
    }

    /// What holds every automatic reply back, or the defaults when nothing
    /// was set. A row that does not read counts as none.
    pub async fn auto_reply_limits(&self) -> Result<AutoReplyLimits> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM preference WHERE key = 'auto_replies'")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|(value,)| serde_json::from_str(&value).ok())
            .unwrap_or_default())
    }

    pub async fn set_auto_reply_limits(&self, limits: &AutoReplyLimits) -> Result<()> {
        sqlx::query(
            "INSERT INTO preference (key, value) VALUES ('auto_replies', ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(serde_json::to_string(limits)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Removes an automatic reply; `false` if there is no such one.
    pub async fn remove_auto_reply(&self, id: AutoReplyId) -> Result<bool> {
        let removed = sqlx::query("DELETE FROM auto_reply WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(removed > 0)
    }

    /// Puts the automatic replies in the order given; `false`, and nothing
    /// changed, unless it names every one once.
    pub async fn set_auto_reply_order(&self, order: &[AutoReplyId]) -> Result<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut ids: Vec<(AutoReplyId,)> = sqlx::query_as("SELECT id FROM auto_reply")
            .fetch_all(&mut *tx)
            .await?;
        let mut asked = order.to_vec();
        ids.sort_unstable();
        asked.sort_unstable();
        if ids.into_iter().map(|(id,)| id).ne(asked.iter().copied())
            || asked.windows(2).any(|w| w[0] == w[1])
        {
            return Ok(false);
        }
        for (position, id) in order.iter().enumerate() {
            sqlx::query("UPDATE auto_reply SET position = ? WHERE id = ?")
                .bind(position as i64 + 1)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}

/// The places as kept, each once; `None` if one is on a radio there is not,
/// a channel in no slot of it (but for one of `away`, kept by its name), or
/// a node that is no node.
pub(super) async fn place_rows(
    conn: &mut SqliteConnection,
    places: &[AutoReplyPlace],
    away: &[(RadioId, i64, String)],
) -> Result<Option<Vec<PlaceRow>>> {
    let mut rows: Vec<PlaceRow> = Vec::with_capacity(places.len());
    for place in places {
        let radio = match place {
            AutoReplyPlace::Directs { radio }
            | AutoReplyPlace::Node { radio, .. }
            | AutoReplyPlace::Channel { radio, .. } => *radio,
        };
        let known: Option<(i64,)> = sqlx::query_as("SELECT id FROM radio WHERE id = ?")
            .bind(radio)
            .fetch_optional(&mut *conn)
            .await?;
        if known.is_none() {
            return Ok(None);
        }
        let row = match place {
            AutoReplyPlace::Directs { .. } => (radio, None, None),
            AutoReplyPlace::Node { node, .. } => {
                if *node == 0 || *node == BROADCAST {
                    return Ok(None);
                }
                (radio, None, Some(i64::from(*node)))
            }
            AutoReplyPlace::Channel {
                channel: Some(slot),
                ..
            } => {
                let channel: Option<(i64,)> = sqlx::query_as(
                    "SELECT channel_id FROM channel_slot WHERE radio_id = ? AND slot = ?",
                )
                .bind(radio)
                .bind(i64::from(*slot))
                .fetch_optional(&mut *conn)
                .await?;
                let Some((channel,)) = channel else {
                    return Ok(None);
                };
                (radio, Some(channel), None)
            }
            AutoReplyPlace::Channel {
                channel: None,
                name,
                ..
            } => {
                let kept = away
                    .iter()
                    .find(|(r, _, n)| *r == radio && Some(n) == name.as_ref());
                let Some((_, channel, _)) = kept else {
                    return Ok(None);
                };
                (radio, Some(*channel), None)
            }
        };
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    Ok(Some(rows))
}

async fn put_places(
    conn: &mut SqliteConnection,
    id: AutoReplyId,
    places: &[PlaceRow],
) -> Result<()> {
    for (radio, channel, node) in places {
        sqlx::query(
            "INSERT INTO auto_reply_place (auto_reply_id, radio_id, channel_id, node)
            VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(radio)
        .bind(channel)
        .bind(node)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{Slot, fingerprint};
    use meshctl_api::{RadioKind, RadioSettings};

    #[tokio::test]
    async fn a_channel_place_goes_with_its_channel() {
        let store = Store::in_memory().await;
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
        let galicia = |channel: Option<u32>| AutoReplyPlace::Channel {
            radio,
            channel,
            name: Some("Galicia".into()),
        };
        store
            .follow_channels(radio, &[slot(0, "LongFast"), slot(1, "Galicia")], 10)
            .await
            .unwrap();
        let mut settings = AutoReplySettings {
            name: "Ping".into(),
            enabled: true,
            triggers: vec!["ping".into()],
            matching: AutoReplyMatch::Whole,
            react: true,
            sign: true,
            reply: None,
            direct_reply: None,
            places: vec![
                AutoReplyPlace::Channel {
                    radio,
                    channel: Some(1),
                    name: None,
                },
                AutoReplyPlace::Directs { radio },
            ],
        };
        let Keeping::Kept(added) = store.add_auto_reply(&settings).await.unwrap() else {
            panic!("not kept");
        };
        assert_eq!(
            added.settings.places,
            [AutoReplyPlace::Directs { radio }, galicia(Some(1))]
        );

        // Moved to another slot: told there.
        store
            .follow_channels(radio, &[slot(0, "LongFast"), slot(2, "Galicia")], 20)
            .await
            .unwrap();
        let told = store.auto_replies().await.unwrap();
        assert_eq!(told[0].settings.places[1], galicia(Some(2)));

        // Taken off the radio: told in no slot, and kept when changed as told.
        store
            .follow_channels(radio, &[slot(0, "LongFast")], 30)
            .await
            .unwrap();
        settings.places = store.auto_replies().await.unwrap()[0]
            .settings
            .places
            .clone();
        assert_eq!(settings.places[1], galicia(None));
        settings.enabled = false;
        let Keeping::Kept(changed) = store.set_auto_reply(added.id, &settings).await.unwrap()
        else {
            panic!("not kept");
        };
        assert_eq!(changed.settings.places[1], galicia(None));

        // Back: there again.
        store
            .follow_channels(radio, &[slot(0, "LongFast"), slot(1, "Galicia")], 40)
            .await
            .unwrap();
        let told = store.auto_replies().await.unwrap();
        assert_eq!(told[0].settings.places[1], galicia(Some(1)));

        // A channel in no slot by a name it never had: not a place.
        settings.places = vec![AutoReplyPlace::Channel {
            radio,
            channel: None,
            name: Some("Lugo".into()),
        }];
        assert_eq!(
            store.set_auto_reply(added.id, &settings).await.unwrap(),
            Keeping::Invalid
        );

        // Its radio removed: its places go with it.
        store.remove_radio(radio).await.unwrap();
        assert!(
            store.auto_replies().await.unwrap()[0]
                .settings
                .places
                .is_empty()
        );
    }
}
