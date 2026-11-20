// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The MeshCore driver: keeps a companion radio connected and its mesh in the
//! same [`MeshState`] the Meshtastic one keeps, so that nothing above this
//! module has to know which of the two a radio speaks.
//!
//! Where MeshCore differs, it is made to fit here:
//!
//! - Nodes are the **contacts** the radio keeps, each known by a public key.
//!   The first four bytes of the key stand for the node number everything
//!   else uses; the whole key stays here, for sending to it.
//! - A **channel message** carries no sender. Clients write their name into
//!   the text, `NAME: message`, and that is where it is read back from.
//! - Messages are **pulled**: the radio announces it has some, and is asked
//!   for them one by one. Those that arrived while nobody was connected were
//!   announced to nobody, and are drained on connecting.
//! - A **direct message** is acknowledged with a tag the radio gives when it
//!   takes the message; a channel message is not acknowledged at all.
//!
//! What this module knows of the protocol it learned from loratrix, which
//! relays MeshCore to Matrix and has been running against real radios.

use crate::link::Transport;
use crate::mesh::{
    Ack, BROADCAST, Channel, Event, LinkState, LocalNode, MeshState, Node, Position, TextMessage,
    User,
};
use crate::radio::{
    Mesh, OutgoingText, RECONNECT_MAX, RECONNECT_MIN, STABLE_SESSION, SendError, Sent, now,
};
use meshcore_rs::commands::CommandHandler;
use meshcore_rs::events::{ChannelMessage, Contact, ContactMessage, SelfInfo};
use meshcore_rs::{EventPayload, EventType, MeshCore, MeshCoreEvent};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

/// The port a MeshCore companion radio serves on over the network.
pub const DEFAULT_TCP_PORT: u16 = 5000;

/// What the radio is told it is talking to: the firmware prints it, as the
/// app that connected.
const APP_NAME: &[u8] = b"meshctl";

/// The `APP_START` command, which opens a companion session.
const CMD_APP_START: u8 = 0x01;

/// How many channel slots to look at when the radio does not say.
const CHANNELS_DEFAULT: u8 = 8;

/// How long after a contact is heard of to read the contacts again: adverts
/// come in bursts, and each read returns every contact that changed.
const CONTACTS_SETTLE: Duration = Duration::from_secs(2);

/// What the radio's path length says for a path it does not know.
const NO_PATH: u8 = 0xff;

/// The node number a public key stands for.
fn num_of(key: &[u8]) -> u32 {
    let mut first = [0u8; 4];
    first.copy_from_slice(&key[..4]);
    u32::from_be_bytes(first)
}

/// What a contact type is called, as roles are elsewhere.
fn role_name(kind: u8) -> String {
    match kind {
        1 => "CHAT".to_owned(),
        2 => "REPEATER".to_owned(),
        3 => "ROOM".to_owned(),
        4 => "SENSOR".to_owned(),
        other => other.to_string(),
    }
}

/// A position as MeshCore gives it, in millionths of a degree; none at zero.
fn position(lat: i32, lon: i32) -> Option<Position> {
    (lat != 0 || lon != 0).then(|| Position {
        latitude: f64::from(lat) / 1e6,
        longitude: f64::from(lon) / 1e6,
        altitude: None,
    })
}

/// When the radio last heard of a contact. Not the time in its advert, which
/// is the sender's clock and may be anything (adverts from next year are
/// common), but when the radio itself last updated the contact, by its own
/// clock; never later than now.
fn last_heard(contact: &Contact) -> Option<u32> {
    let heard = match contact.last_modification_timestamp {
        0 => contact.last_advert,
        updated => updated,
    };
    (heard > 0).then(|| heard.min(now()))
}

fn node_from(contact: &Contact) -> Node {
    Node {
        num: num_of(&contact.public_key),
        user: Some(User {
            long_name: contact.adv_name.clone(),
            short_name: String::new(),
            hw_model: String::new(),
            role: role_name(contact.contact_type),
        }),
        position: position(contact.adv_lat, contact.adv_lon),
        last_heard: last_heard(contact),
        snr: None,
        rssi: None,
        hops_away: u32::try_from(contact.path_len).ok(),
        via_mqtt: false,
        favorite: contact.flags & 1 != 0,
        battery_level: None,
        voltage: None,
    }
}

/// Splits the `NAME: message` a MeshCore client writes, at the first
/// colon-space: whatever the person typed comes after it. A name too long or
/// spanning lines was punctuation, not a name, and the text is left whole.
fn split_sender(text: &str) -> Option<(&str, &str)> {
    let (name, body) = text.split_once(": ")?;
    if name.is_empty() || body.is_empty() || name.chars().count() > 32 || name.contains('\n') {
        return None;
    }
    Some((name, body))
}

/// An id for a message that has none of its own, so that the same message
/// heard twice (replayed on reconnecting, say) is known to be the same.
fn message_id(from: &[u8], timestamp: u32, text: &str) -> u32 {
    let mut hasher = DefaultHasher::new();
    from.hash(&mut hasher);
    timestamp.hash(&mut hasher);
    text.hash(&mut hasher);
    // The low half of the hash; zero is kept for "none".
    (u32::try_from(hasher.finish() & 0xffff_ffff).unwrap_or(1)).max(1)
}

/// How many hops a message came, from the path length the radio gives.
fn hops(path_len: u8) -> Option<u32> {
    (path_len != NO_PATH).then_some(u32::from(path_len))
}

/// The open session with a radio: what sending needs.
#[derive(Clone)]
pub(crate) struct Session {
    commands: Arc<tokio::sync::Mutex<CommandHandler>>,
    /// Every contact's whole public key, by the node number it stands for.
    keys: Arc<Mutex<HashMap<u32, [u8; 32]>>>,
    /// Direct messages waiting for their acknowledgement: the tag the radio
    /// gave, and who they went to.
    pending: Arc<Mutex<HashMap<[u8; 4], u32>>>,
}

impl Session {
    pub(crate) async fn send_text(
        &self,
        mesh: &Mesh,
        outgoing: &OutgoingText,
    ) -> Result<Sent, SendError> {
        let stamp = Some(now());
        if outgoing.to == BROADCAST {
            let channel = u8::try_from(outgoing.channel).map_err(|_| SendError::Refused)?;
            self.commands
                .lock()
                .await
                .send_channel_msg(channel, &outgoing.text, stamp)
                .await
                .map_err(|_| SendError::Refused)?;
            // Nothing will ever say what became of it; an id of its own, as
            // every message kept needs one.
            let me = mesh.read(|s| s.local.as_ref().map_or(0, |l| l.num));
            return Ok(Sent {
                packet_id: message_id(&me.to_be_bytes(), now(), &outgoing.text),
                acknowledged: false,
            });
        }
        let key = self
            .keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&outgoing.to)
            .copied()
            .ok_or(SendError::Refused)?;
        let sent = self
            .commands
            .lock()
            .await
            .send_msg(key.to_vec(), &outgoing.text, stamp)
            .await
            .map_err(|_| SendError::Refused)?;
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sent.expected_ack, outgoing.to);
        // Given up on, as the radio suggests, if no acknowledgement comes.
        let timeout = Duration::from_millis(u64::from(sent.suggested_timeout).max(5_000) * 2);
        let pending = Arc::clone(&self.pending);
        let mesh = mesh.clone();
        let tag = sent.expected_ack;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let to = pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&tag);
            if let Some(to) = to {
                mesh.update(|_| {
                    vec![Event::Ack(Ack {
                        packet_id: u32::from_le_bytes(tag),
                        from: to,
                        error: Some("TIMEOUT".to_owned()),
                    })]
                });
            }
        });
        Ok(Sent {
            packet_id: u32::from_le_bytes(sent.expected_ack),
            acknowledged: true,
        })
    }
}

/// Connects to the radio and keeps connected, for as long as the task runs.
pub(crate) async fn run(mesh: Mesh, transport: Transport) {
    let mut delay = RECONNECT_MIN;
    loop {
        mesh.set_link(LinkState::Connecting);
        let connected = match &transport {
            Transport::Tcp { host, port } => MeshCore::tcp(host, *port).await,
            Transport::Serial { path, baud } => MeshCore::serial(path, *baud).await,
        };
        match connected {
            Ok(client) => {
                tracing::info!(radio = %transport, "connected");
                let started = Instant::now();
                let why = session(&mesh, &client).await;
                *mesh.meshcore.lock().unwrap_or_else(|e| e.into_inner()) = None;
                // The read and write tasks stop only here; a reconnect would
                // leave a pair of them behind every time otherwise.
                let _ = client.disconnect().await;
                if started.elapsed() >= STABLE_SESSION {
                    delay = RECONNECT_MIN;
                }
                tracing::warn!(radio = %transport, retry_in = delay.as_secs(), "{why}, reconnecting");
            }
            Err(error) => {
                tracing::warn!(radio = %transport, %error, retry_in = delay.as_secs(), "cannot connect");
            }
        }
        mesh.set_link(LinkState::Disconnected);
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RECONNECT_MAX);
    }
}

/// One connection, from saying who we are until it drops. Returns why it
/// ended.
async fn session(mesh: &Mesh, client: &MeshCore) -> &'static str {
    // Before anything is asked: a receiver only sees what is sent after it
    // exists, and the answers are events like any other.
    let mut events = client.dispatcher().receiver();
    mesh.set_link(LinkState::Syncing);
    let commands = Arc::clone(client.commands());

    let me = match appstart(&commands).await {
        Ok(me) => me,
        Err(why) => return why,
    };
    let device = commands.lock().await.send_device_query().await.ok();
    let local = LocalNode {
        num: num_of(&me.public_key),
        firmware_version: device
            .as_ref()
            .and_then(|d| match (&d.version, &d.fw_build) {
                (Some(version), Some(build)) => Some(format!("{version} ({build})")),
                (version, _) => version.clone(),
            }),
        hw_model: device.as_ref().and_then(|d| d.model.clone()),
        role: Some(role_name(me.adv_type)),
        // Frequency in kHz, bandwidth in Hz: as people write them.
        region: Some(format!("{:.3} MHz", f64::from(me.radio_freq) / 1000.0)),
        modem_preset: Some(format!(
            "BW {} kHz · SF{} · CR{}",
            f64::from(me.radio_bw) / 1000.0,
            me.sf,
            me.cr
        )),
    };
    let channels_max = device
        .as_ref()
        .and_then(|d| d.max_channels)
        .unwrap_or(CHANNELS_DEFAULT);
    let session = Session {
        commands: Arc::clone(&commands),
        keys: Arc::default(),
        pending: Arc::default(),
    };

    let contacts = match commands.lock().await.get_contacts(0).await {
        Ok(contacts) => contacts,
        Err(_) => return "the radio did not send its contacts",
    };
    let mut last_change = contacts
        .iter()
        .map(|c| c.last_modification_timestamp)
        .max()
        .unwrap_or(0);
    let mut channels = Vec::new();
    for index in 0..channels_max {
        match commands.lock().await.get_channel(index).await {
            Ok(channel) if !channel.name.is_empty() => channels.push(Channel {
                index: u32::from(index),
                name: channel.name,
                role: if index == 0 { "PRIMARY" } else { "SECONDARY" }.to_owned(),
            }),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let me_node = Node {
        position: position(me.adv_lat, me.adv_lon),
        last_heard: Some(now()),
        hops_away: Some(0),
        ..node_from(&Contact {
            public_key: me.public_key,
            contact_type: me.adv_type,
            flags: 0,
            path_len: 0,
            out_path: Vec::new(),
            adv_name: me.name.clone(),
            last_advert: now(),
            adv_lat: me.adv_lat,
            adv_lon: me.adv_lon,
            last_modification_timestamp: 0,
        })
    };
    remember_keys(&session, &contacts);
    mesh.update(|state| {
        // Another radio behind the same address keeps the nodes heard so
        // far, as with Meshtastic; only its own description is its own.
        if state.local.as_ref().is_some_and(|l| l.num != local.num) {
            state.channels.clear();
        }
        state.local = Some(local);
        state.channels = channels
            .into_iter()
            .map(|channel| (channel.index, channel))
            .collect();
        for node in contacts.iter().map(node_from).chain([me_node]) {
            state.nodes.insert(node.num, node);
        }
        Vec::new()
    });
    *mesh.meshcore.lock().unwrap_or_else(|e| e.into_inner()) = Some(session.clone());

    // Whatever arrived while nobody was connected: announced once, to nobody.
    loop {
        match commands.lock().await.get_msg().await {
            Ok(Some(event)) => take(mesh, &session, &event),
            Ok(None) => break,
            Err(_) => return "the radio did not hand over its messages",
        }
    }
    client.start_auto_message_fetching().await;
    let nodes = mesh.read(|s| s.nodes.len());
    tracing::info!(nodes, "in sync with the radio");
    mesh.set_link(LinkState::Ready);

    let mut contacts_due: Option<Instant> = None;
    loop {
        let settle = async {
            match contacts_due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) if event.event_type == EventType::Disconnected => {
                    return "the radio closed the link";
                }
                Ok(event) => {
                    if matches!(
                        event.payload,
                        EventPayload::Advertisement(_) | EventPayload::PathUpdate(_)
                    ) {
                        contacts_due.get_or_insert(Instant::now() + CONTACTS_SETTLE);
                    }
                    take(mesh, &session, &event);
                }
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "fell behind the radio and lost events");
                    contacts_due.get_or_insert(Instant::now());
                }
                Err(RecvError::Closed) => return "the link closed",
            },
            () = settle => {
                contacts_due = None;
                match commands.lock().await.get_contacts(last_change).await {
                    Ok(changed) => {
                        last_change = changed
                            .iter()
                            .map(|c| c.last_modification_timestamp)
                            .max()
                            .unwrap_or(last_change);
                        remember_keys(&session, &changed);
                        mesh.update(|state| {
                            changed
                                .iter()
                                .map(node_from)
                                .map(|node| {
                                    state.nodes.insert(node.num, node.clone());
                                    Event::Node(node)
                                })
                                .collect()
                        });
                    }
                    Err(error) => tracing::debug!(%error, "contacts not read again"),
                }
            }
        }
    }
}

/// Opens a companion session, saying meshctl is what is connecting. The
/// published crate names itself instead, so the frame is put together here:
/// the command byte, seven reserved bytes, and the name.
async fn appstart(commands: &tokio::sync::Mutex<CommandHandler>) -> Result<SelfInfo, &'static str> {
    let mut frame = vec![0u8; 8];
    frame[0] = CMD_APP_START;
    frame.extend_from_slice(APP_NAME);
    let answer = commands
        .lock()
        .await
        .send(&frame, Some(EventType::SelfInfo))
        .await
        .map_err(|_| "the radio did not answer APP_START")?;
    match answer.payload {
        EventPayload::SelfInfo(me) => Ok(me),
        _ => Err("the radio answered APP_START with something else"),
    }
}

fn remember_keys(session: &Session, contacts: &[Contact]) {
    let mut keys = session.keys.lock().unwrap_or_else(|e| e.into_inner());
    for contact in contacts {
        keys.insert(num_of(&contact.public_key), contact.public_key);
    }
}

/// Takes in what the radio says, as the same events a Meshtastic radio's
/// make.
fn take(mesh: &Mesh, session: &Session, event: &MeshCoreEvent) {
    match &event.payload {
        EventPayload::ContactMessage(message) => mesh.update(|state| direct(state, message)),
        EventPayload::ChannelMessage(message) => mesh.update(|state| channel(state, message)),
        EventPayload::Contact(contact) => {
            remember_keys(session, std::slice::from_ref(contact));
            let node = node_from(contact);
            mesh.update(|state| {
                state.nodes.insert(node.num, node.clone());
                vec![Event::Node(node)]
            });
        }
        EventPayload::Ack { tag } => {
            let to = session
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(tag);
            if let Some(to) = to {
                mesh.update(|_| {
                    vec![Event::Ack(Ack {
                        packet_id: u32::from_le_bytes(*tag),
                        from: to,
                        error: None,
                    })]
                });
            }
        }
        _ => {}
    }
}

fn direct(state: &mut MeshState, message: &ContactMessage) -> Vec<Event> {
    let from = num_of(&message.sender_prefix);
    let at = now();
    let mut events = Vec::new();
    if let Some(node) = state.nodes.get_mut(&from) {
        node.last_heard = Some(at);
        node.hops_away = hops(message.path_len);
        if message.snr.is_some() {
            node.snr = message.snr;
        }
        events.push(Event::Node(node.clone()));
    }
    events.push(Event::Message(TextMessage {
        packet_id: message_id(
            &message.sender_prefix,
            message.sender_timestamp,
            &message.text,
        ),
        from,
        to: state.local.as_ref().map_or(0, |l| l.num),
        channel: 0,
        text: message.text.clone(),
        at,
        reply_id: None,
        sender_name: None,
        emoji: false,
        snr: message.snr,
        rssi: None,
        hops_away: hops(message.path_len),
        via_mqtt: false,
    }));
    events
}

fn channel(state: &mut MeshState, message: &ChannelMessage) -> Vec<Event> {
    let (name, text) = match split_sender(&message.text) {
        Some((name, body)) => (Some(name.to_owned()), body.to_owned()),
        None => (None, message.text.clone()),
    };
    // The name stands for a node only when exactly one contact has it.
    let from = name
        .as_deref()
        .and_then(|name| {
            let mut named = state
                .nodes
                .values()
                .filter(|n| n.user.as_ref().is_some_and(|u| u.long_name == name));
            match (named.next(), named.next()) {
                (Some(node), None) => Some(node.num),
                _ => None,
            }
        })
        .unwrap_or(0);
    vec![Event::Message(TextMessage {
        packet_id: message_id(
            &[message.channel_idx],
            message.sender_timestamp,
            &message.text,
        ),
        from,
        to: BROADCAST,
        channel: u32::from(message.channel_idx),
        text,
        at: now(),
        reply_id: None,
        sender_name: name,
        emoji: false,
        snr: message.snr,
        rssi: None,
        hops_away: hops(message.path_len),
        via_mqtt: false,
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(key_start: u8, name: &str, kind: u8) -> Contact {
        let mut public_key = [0u8; 32];
        public_key[0] = key_start;
        Contact {
            public_key,
            contact_type: kind,
            flags: 0,
            path_len: 2,
            out_path: Vec::new(),
            adv_name: name.into(),
            last_advert: 1_795_000_000,
            adv_lat: 43_331_140,
            adv_lon: -8_463_732,
            last_modification_timestamp: 1,
        }
    }

    #[test]
    fn a_contact_is_a_node() {
        let node = node_from(&contact(0x50, "Faro", 2));
        assert_eq!(node.num, 0x5000_0000);
        assert_eq!(node.id(), "!50000000");
        let user = node.user.unwrap();
        assert_eq!(user.long_name, "Faro");
        assert_eq!(user.role, "REPEATER");
        assert_eq!(node.hops_away, Some(2));
        let position = node.position.unwrap();
        assert!((position.latitude - 43.33114).abs() < 1e-9);
    }

    #[test]
    fn heard_is_by_the_radio_clock_and_never_ahead() {
        let mut heard = contact(1, "Faro", 1);
        heard.last_advert = 4_000_000_000;
        heard.last_modification_timestamp = 1_795_000_000;
        assert_eq!(last_heard(&heard), Some(1_795_000_000));
        heard.last_modification_timestamp = 4_000_000_000;
        assert!(last_heard(&heard).unwrap() <= now());
    }

    #[test]
    fn an_unknown_path_is_no_hops() {
        let mut far = contact(1, "Lejos", 1);
        far.path_len = -1;
        assert_eq!(node_from(&far).hops_away, None);
        assert_eq!(hops(NO_PATH), None);
    }

    #[test]
    fn the_sender_is_read_from_the_text() {
        assert_eq!(
            split_sender("ALICE: hola: qué tal"),
            Some(("ALICE", "hola: qué tal"))
        );
        assert_eq!(split_sender("https://example.org"), None);
        assert_eq!(split_sender("sin nombre"), None);
    }

    fn channel_message(text: &str) -> ChannelMessage {
        ChannelMessage {
            channel_idx: 1,
            path_len: 3,
            txt_type: 0,
            sender_timestamp: 42,
            text: text.into(),
            snr: Some(4.5),
        }
    }

    #[test]
    fn a_channel_message_names_its_sender_and_finds_them() {
        let mut state = MeshState::default();
        let faro = node_from(&contact(0x50, "Faro", 1));
        state.nodes.insert(faro.num, faro);
        let events = channel(&mut state, &channel_message("Faro: buenas"));
        let [Event::Message(message)] = events.as_slice() else {
            panic!("a message");
        };
        assert_eq!(message.text, "buenas");
        assert_eq!(message.sender_name.as_deref(), Some("Faro"));
        assert_eq!(message.from, 0x5000_0000);
        assert_eq!(message.to, BROADCAST);
        assert_eq!(message.channel, 1);
        assert_eq!(message.hops_away, Some(3));
    }

    #[test]
    fn a_name_two_contacts_share_stands_for_neither() {
        let mut state = MeshState::default();
        for start in [1, 2] {
            let node = node_from(&contact(start, "Faro", 1));
            state.nodes.insert(node.num, node);
        }
        let events = channel(&mut state, &channel_message("Faro: buenas"));
        let [Event::Message(message)] = events.as_slice() else {
            panic!("a message");
        };
        assert_eq!(message.from, 0);
        assert_eq!(message.sender_name.as_deref(), Some("Faro"));
    }

    #[test]
    fn the_same_message_heard_twice_is_the_same() {
        let mut state = MeshState::default();
        let once = channel(&mut state, &channel_message("Faro: buenas"));
        let twice = channel(&mut state, &channel_message("Faro: buenas"));
        let id = |events: &[Event]| match events {
            [Event::Message(m)] => m.packet_id,
            _ => panic!("a message"),
        };
        assert_eq!(id(&once), id(&twice));
    }

    #[test]
    fn a_direct_message_comes_from_a_contact() {
        let mut state = MeshState {
            local: Some(LocalNode {
                num: 7,
                ..LocalNode::default()
            }),
            ..MeshState::default()
        };
        let faro = node_from(&contact(0x50, "Faro", 1));
        state.nodes.insert(faro.num, faro);
        let message = ContactMessage {
            sender_prefix: [0x50, 0, 0, 0, 0, 0],
            path_len: NO_PATH,
            txt_type: 0,
            sender_timestamp: 42,
            text: "hola".into(),
            snr: Some(6.0),
            signature: None,
        };
        let events = direct(&mut state, &message);
        assert!(matches!(&events[0], Event::Node(n) if n.snr == Some(6.0)));
        let Event::Message(message) = &events[1] else {
            panic!("a message");
        };
        assert_eq!(message.from, 0x5000_0000);
        assert_eq!(message.to, 7);
        assert_eq!(message.hops_away, None);
    }
}
