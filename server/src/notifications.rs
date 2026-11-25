// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Notifications: a message heard where they are set to be heard of is
//! notified of, by the app meshctl runs within, on its own, and on the event
//! stream, for a browser with the panel open to show where it is let.
//!
//! Never a message of the radio's own, nor a reaction, nor one heard as the
//! radio catches up on what it kept while away, nor one from long ago; nor
//! one in a conversation a client shows, and is seen. On a channel, maybe
//! only those that mention the radio by its name.

use crate::radios::{Change, Radios};
use crate::store::now;
use meshctl_api::{
    AutoReplyPlace, BROADCAST, Message, Notification, NotificationPlace, Peer, RadioId,
};
use meshctl_core::mesh::{self, LinkState, MeshState};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;

/// How long a radio is taken to be catching up, after it is ready: what it
/// kept while nobody was there comes then, and is not notified of.
const CATCHING_UP: Duration = Duration::from_secs(20);

/// The oldest a message is notified of: by when it was heard.
const FRESH_SECS: i64 = 120;

/// Notifies of what the radios hear, until they stop; and tells the app
/// of each conversation read, for its notification to go.
pub fn spawn(radios: Radios) {
    tokio::spawn(async move {
        let mut changes = radios.subscribe();
        let mut ready: HashMap<RadioId, Instant> = HashMap::new();
        loop {
            let change = tokio::select! {
                change = changes.recv() => change,
                () = radios.stopped() => return,
            };
            match change {
                Ok(Change::Mesh(radio, mesh::Event::Link(LinkState::Ready))) => {
                    ready.insert(radio, Instant::now());
                }
                Ok(Change::Message(radio, message)) => {
                    if ready
                        .get(&radio)
                        .is_some_and(|at| at.elapsed() >= CATCHING_UP)
                    {
                        heard(&radios, radio, &message).await;
                    }
                }
                Ok(Change::ConversationRead { radio, peer, .. }) => {
                    if let Some(host) = radios.host() {
                        host.read(radio, peer);
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    });
}

async fn heard(radios: &Radios, radio: RadioId, message: &Message) {
    if !notifiable(message, now()) {
        return;
    }
    let settings = match radios.notifications().await {
        Ok(settings) => settings,
        Err(err) => {
            tracing::warn!(error = format_args!("{err:#}"), "notifications");
            return;
        }
    };
    if !settings.enabled {
        return;
    }
    let Some(mesh) = radios.mesh(radio) else {
        return;
    };
    let (own, sender, channel) = mesh.read(|state| {
        (
            own_names(state),
            sender(state, message),
            channel_name(state, message),
        )
    });
    if !settings
        .places
        .iter()
        .any(|place| notifies(place, radio, message, &own))
    {
        return;
    }
    let peer = peer_of(message);
    if radios.viewed(radio, peer) {
        return;
    }
    let notification = Notification {
        radio,
        peer,
        message: message.id,
        sender,
        radio_name: radios.radio_name(radio).await.unwrap_or_default(),
        channel,
        text: settings.show_text.then(|| message.text.clone()),
    };
    if let Some(host) = radios.host() {
        host.notify(&notification);
    }
    radios.notify(notification);
}

/// Whether a message is one to notify of at `now`, wherever it came.
fn notifiable(message: &Message, now: i64) -> bool {
    !message.outgoing && !message.emoji && now - message.at <= FRESH_SECS
}

/// Where a message heard is: its channel, or who sent it.
fn peer_of(message: &Message) -> Peer {
    if message.to == BROADCAST {
        Peer::Channel {
            channel: message.channel,
        }
    } else {
        Peer::Node { node: message.from }
    }
}

/// Whether a place notifies of a message heard by a radio.
fn notifies(place: &NotificationPlace, radio: RadioId, message: &Message, own: &[String]) -> bool {
    let channel = message.to == BROADCAST;
    match &place.place {
        AutoReplyPlace::Directs { radio: r } => *r == radio && !channel,
        AutoReplyPlace::Node { radio: r, node } => *r == radio && !channel && *node == message.from,
        AutoReplyPlace::Channel {
            radio: r,
            channel: slot,
            ..
        } => {
            *r == radio
                && channel
                && *slot == Some(message.channel)
                && (!place.mentions || mentions(&message.text, own))
        }
    }
}

/// The names a radio goes by: a Meshtastic one's long and short, a
/// MeshCore one's.
fn own_names(state: &MeshState) -> Vec<String> {
    if let Some(meshcore) = &state.meshcore {
        return vec![meshcore.name.clone()];
    }
    state
        .local
        .as_ref()
        .and_then(|local| state.nodes.get(&local.num))
        .and_then(|node| node.user.as_ref())
        .map(|user| vec![user.long_name.clone(), user.short_name.clone()])
        .unwrap_or_default()
}

/// Whether a text mentions one of the names, as `@Name` or `@[Name]`, in
/// any case, and not as the start of a longer word.
fn mentions(text: &str, names: &[String]) -> bool {
    let text = text.to_lowercase();
    names
        .iter()
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .any(|name| {
            if text.contains(&format!("@[{name}]")) {
                return true;
            }
            let wanted = format!("@{name}");
            text.match_indices(&wanted).any(|(at, _)| {
                text[at + wanted.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            })
        })
}

/// Who sent a message: the name it came under, their node's, or their
/// node's id.
fn sender(state: &MeshState, message: &Message) -> String {
    let node = message.author.unwrap_or(message.from);
    let user = state.nodes.get(&node).and_then(|n| n.user.as_ref());
    if let Some(name) = message.sender_name.as_ref().filter(|n| !n.is_empty()) {
        return name.clone();
    }
    if let Some(name) = user.map(|u| u.long_name.clone()).filter(|n| !n.is_empty()) {
        return name;
    }
    match user {
        // MeshCore's nodes go by the start of their key.
        Some(user) if state.meshcore.is_some() && user.public_key.len() >= 4 => user.public_key
            [..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        _ => format!("!{node:08x}"),
    }
}

/// The name of the channel a message came on; none in a direct one.
fn channel_name(state: &MeshState, message: &Message) -> Option<String> {
    (message.to == BROADCAST)
        .then(|| {
            crate::channels::slots_of(state)
                .into_iter()
                .find(|slot| slot.index == message.channel)
                .map(|slot| slot.name)
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(to: u32, channel: u32, text: &str) -> Message {
        Message {
            id: 1,
            packet_id: 7,
            from: 0x1234,
            sender_name: None,
            to,
            channel,
            text: text.into(),
            at: 1_000,
            outgoing: false,
            status: meshctl_api::MessageStatus::Received,
            error: None,
            reply_id: None,
            emoji: false,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
            scope: None,
            heard: 0,
            sent_at: None,
            repeaters: 0,
            author: None,
        }
    }

    #[test]
    fn a_mention_is_the_name_whole_after_an_at() {
        let names = ["Óscar".to_owned(), "OGA".to_owned()];
        assert!(mentions("hola @óscar, ¿qué tal?", &names));
        assert!(mentions("@[Óscar] mira esto", &names));
        assert!(mentions("@oga", &names));
        assert!(!mentions("@ogarcia", &names));
        assert!(!mentions("Óscar, sin arroba", &names));
        assert!(!mentions("@", &[String::new()]));
    }

    #[test]
    fn places_notify_of_what_comes_where_they_are() {
        let radio = 3;
        let place = |place, mentions| NotificationPlace { place, mentions };
        let direct = message(0x99, 0, "hola");
        let galicia = message(BROADCAST, 1, "hola a todos");
        let called = message(BROADCAST, 1, "@Casa ¿estás?");
        let own = ["Casa".to_owned()];

        let directs = place(AutoReplyPlace::Directs { radio }, false);
        assert!(notifies(&directs, radio, &direct, &own));
        assert!(!notifies(&directs, radio, &galicia, &own));
        assert!(!notifies(&directs, radio + 1, &direct, &own));

        let ana = place(
            AutoReplyPlace::Node {
                radio,
                node: 0x1234,
            },
            false,
        );
        assert!(notifies(&ana, radio, &direct, &own));

        let channel = |mentions| {
            place(
                AutoReplyPlace::Channel {
                    radio,
                    channel: Some(1),
                    name: None,
                },
                mentions,
            )
        };
        assert!(notifies(&channel(false), radio, &galicia, &own));
        assert!(!notifies(&channel(true), radio, &galicia, &own));
        assert!(notifies(&channel(true), radio, &called, &own));
        assert!(!notifies(&channel(false), radio, &direct, &own));
    }

    #[test]
    fn only_what_others_wrote_lately_is_notified_of() {
        let fresh = message(0x99, 0, "hola");
        assert!(notifiable(&fresh, 1_060));
        assert!(!notifiable(&fresh, 1_000 + FRESH_SECS + 1));
        let mut own = fresh.clone();
        own.outgoing = true;
        assert!(!notifiable(&own, 1_060));
        let mut reaction = fresh;
        reaction.emoji = true;
        assert!(!notifiable(&reaction, 1_060));
    }
}
