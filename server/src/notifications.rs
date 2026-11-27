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
//!
//! Waypoints too: one shared new by another node, on a channel notified of
//! whole (not only its mentions); and a node going in or out of a fence the
//! radio watches, as its author asked, told of wherever notifications are
//! on.

use crate::radios::{Change, Radios};
use crate::store::now;
use meshctl_api::waypoints::Waypoint;
use meshctl_api::{
    AutoReplyPlace, BROADCAST, Message, Notification, NotificationPlace, Peer, RadioId,
    WaypointEvent, WaypointNotice,
};
use meshctl_core::mesh::{self, LinkState, MeshState};
use meshctl_core::waypoint::Waypoint as Shared;
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
        let mut fences = Fences::default();
        // The waypoints told of as shared: once each, however often they
        // change after.
        let mut told: std::collections::HashSet<(RadioId, u32)> = std::collections::HashSet::new();
        loop {
            let change = tokio::select! {
                change = changes.recv() => change,
                () = radios.stopped() => return,
            };
            match change {
                Ok(Change::Mesh(radio, mesh::Event::Link(LinkState::Ready))) => {
                    ready.insert(radio, Instant::now());
                }
                Ok(Change::Waypoint(radio, waypoint)) => {
                    fences.forget(radio);
                    if ready
                        .get(&radio)
                        .is_some_and(|at| at.elapsed() >= CATCHING_UP)
                        && told.insert((radio, waypoint.id))
                    {
                        shared(&radios, radio, &waypoint).await;
                    }
                }
                Ok(Change::WaypointGone(radio, id)) => {
                    fences.forget(radio);
                    told.remove(&(radio, id));
                }
                Ok(Change::Mesh(radio, mesh::Event::Node(node))) => {
                    fences.moved(&radios, radio, &node).await;
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
    if radios.viewed(radio, peer) || radios.muted(radio, peer).await {
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
        waypoint: None,
    };
    if let Some(host) = radios.host() {
        host.notify(&notification);
    }
    radios.notify(notification);
}

/// Notifies of a waypoint shared new by another node, on a channel whose
/// messages are all notified of, and not silenced.
async fn shared(radios: &Radios, radio: RadioId, waypoint: &Waypoint) {
    let new =
        waypoint.created_at == waypoint.updated_at && now() - waypoint.created_at <= FRESH_SECS;
    if !new {
        return;
    }
    let Some(mesh) = radios.mesh(radio) else {
        return;
    };
    let local = mesh.read(|state| state.local.as_ref().map(|l| l.num));
    if local == Some(waypoint.created_by) {
        return;
    }
    let settings = match radios.notifications().await {
        Ok(settings) => settings,
        Err(err) => {
            tracing::warn!(error = format_args!("{err:#}"), "notifications");
            return;
        }
    };
    let whole = settings.places.iter().any(|place| {
        !place.mentions
            && matches!(&place.place, AutoReplyPlace::Channel { radio: r, channel: slot, .. }
                if *r == radio && *slot == Some(waypoint.channel))
    });
    let peer = Peer::Channel {
        channel: waypoint.channel,
    };
    if !settings.enabled || !whole || radios.muted(radio, peer).await {
        return;
    }
    let (sender, channel) = mesh.read(|state| {
        (
            node_name(state, waypoint.created_by),
            slot_name(state, waypoint.channel),
        )
    });
    notify_waypoint(
        radios,
        Notification {
            radio,
            peer,
            message: 0,
            sender,
            radio_name: radios.radio_name(radio).await.unwrap_or_default(),
            channel,
            text: settings
                .show_text
                .then(|| format!("{} {}", waypoint.icon, waypoint.name)),
            waypoint: Some(WaypointNotice {
                id: waypoint.id,
                event: WaypointEvent::Shared,
            }),
        },
    );
}

fn notify_waypoint(radios: &Radios, notification: Notification) {
    if let Some(host) = radios.host() {
        host.notify(&notification);
    }
    radios.notify(notification);
}

/// A node's name, as a notification gives it: its long name, or its id.
fn node_name(state: &MeshState, num: u32) -> String {
    state
        .nodes
        .get(&num)
        .and_then(|n| n.user.as_ref())
        .map(|u| u.long_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("!{num:08x}"))
}

/// A channel's name, by its slot.
fn slot_name(state: &MeshState, slot: u32) -> Option<String> {
    crate::channels::slots_of(state)
        .into_iter()
        .find(|s| s.index == slot)
        .map(|s| s.name)
}

/// Whether a point is within a waypoint's fence: its circle or its box,
/// edges included.
fn inside(fence: &Shared, latitude: f64, longitude: f64) -> bool {
    let in_circle = fence.fence_radius > 0
        && distance(fence.latitude, fence.longitude, latitude, longitude)
            <= f64::from(fence.fence_radius);
    let in_box = fence.fence_box.is_some_and(|b| {
        (b.south..=b.north).contains(&latitude) && (b.west..=b.east).contains(&longitude)
    });
    in_circle || in_box
}

/// How far apart two points are on the Earth, in metres.
fn distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH: f64 = 6_371_000.0;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH * a.sqrt().asin()
}

/// The fences each radio watches, read as needed, and whether each node
/// was last inside each: the first time a node is placed against one, it
/// is only noted, as the firmware and the apps do.
#[derive(Default)]
struct Fences {
    watched: HashMap<RadioId, Vec<Shared>>,
    inside: HashMap<(RadioId, u32, u32), bool>,
}

impl Fences {
    /// A radio's fences changed: read again when next needed.
    fn forget(&mut self, radio: RadioId) {
        self.watched.remove(&radio);
    }

    /// A node of a radio's heard of again: told of if it went in or out of
    /// a fence watched, as the fence asks.
    async fn moved(&mut self, radios: &Radios, radio: RadioId, node: &mesh::Node) {
        let Some(at) = node.position else {
            return;
        };
        let Some(mesh) = radios.mesh(radio) else {
            return;
        };
        if mesh.read(|state| state.local.as_ref().map(|l| l.num)) == Some(node.num) {
            return;
        }
        let watched = match self.watched.entry(radio) {
            std::collections::hash_map::Entry::Occupied(kept) => kept.into_mut(),
            std::collections::hash_map::Entry::Vacant(empty) => {
                match radios.store.watched_waypoints(radio).await {
                    Ok(watched) => empty.insert(watched),
                    Err(err) => {
                        tracing::warn!(error = format_args!("{err:#}"), "fences");
                        return;
                    }
                }
            }
        };
        let inside_now = &mut self.inside;
        let crossed: Vec<(Shared, bool)> = watched
            .iter()
            .filter_map(|fence| {
                let now_inside = inside(fence, at.latitude, at.longitude);
                let was = inside_now.insert((radio, fence.id, node.num), now_inside);
                (was.is_some_and(|was| was != now_inside)).then(|| (fence.clone(), now_inside))
            })
            .collect();
        for (fence, went_in) in crossed {
            let asked = if went_in {
                fence.notify_on_enter
            } else {
                fence.notify_on_exit
            };
            if asked && (!fence.notify_favorites_only || node.favorite) {
                crossing(radios, radio, &fence, node.num, went_in).await;
            }
        }
    }
}

/// Tells of a node gone in or out of a fence, wherever notifications are on.
async fn crossing(radios: &Radios, radio: RadioId, fence: &Shared, node: u32, went_in: bool) {
    let Ok(settings) = radios.notifications().await else {
        return;
    };
    if !settings.enabled {
        return;
    }
    let Some(mesh) = radios.mesh(radio) else {
        return;
    };
    let kept = radios.store.waypoint(radio, fence.id).await.ok().flatten();
    let channel = kept.as_ref().map_or(0, |(w, _)| w.channel);
    let (sender, channel_name) =
        mesh.read(|state| (node_name(state, node), slot_name(state, channel)));
    notify_waypoint(
        radios,
        Notification {
            radio,
            peer: Peer::Channel { channel },
            message: 0,
            sender,
            radio_name: radios.radio_name(radio).await.unwrap_or_default(),
            channel: channel_name,
            text: settings
                .show_text
                .then(|| format!("{} {}", fence.emoji(), fence.name)),
            waypoint: Some(WaypointNotice {
                id: fence.id,
                event: if went_in {
                    WaypointEvent::Entered
                } else {
                    WaypointEvent::Left
                },
            }),
        },
    );
}

/// Whether a message is one to notify of at `now`, wherever it came: a
/// text, not a reaction nor a position.
fn notifiable(message: &Message, now: i64) -> bool {
    !message.outgoing
        && !message.emoji
        && message.position.is_none()
        && now - message.at <= FRESH_SECS
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

    #[test]
    fn a_fence_holds_its_circle_and_its_box() {
        let fence = Shared {
            latitude: 43.36,
            longitude: -8.41,
            fence_radius: 250,
            ..Default::default()
        };
        assert!(inside(&fence, 43.36, -8.41));
        // About 220 m north, and about 330 m.
        assert!(inside(&fence, 43.362, -8.41));
        assert!(!inside(&fence, 43.363, -8.41));
        let boxed = Shared {
            fence_radius: 0,
            fence_box: Some(meshctl_core::waypoint::FenceBox {
                south: 43.0,
                west: -9.0,
                north: 44.0,
                east: -8.0,
            }),
            ..fence
        };
        assert!(inside(&boxed, 43.5, -8.5));
        assert!(!inside(&boxed, 44.5, -8.5));
        assert!((distance(43.36, -8.41, 43.37, -8.41) - 1_112.0).abs() < 2.0);
    }

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
            position: None,
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
        let mut reaction = fresh.clone();
        reaction.emoji = true;
        assert!(!notifiable(&reaction, 1_060));
        let mut position = fresh;
        position.position = Some(meshctl_api::Position {
            latitude: 43.36,
            longitude: -8.41,
            altitude: None,
            radius: None,
            source: None,
        });
        assert!(!notifiable(&position, 1_060));
    }
}
