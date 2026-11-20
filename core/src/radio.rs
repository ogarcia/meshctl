// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Keeps meshctl connected to the radio: connects, asks for the configuration
//! and node list, keeps the session alive, and reconnects when it drops, for as
//! long as the process runs.
//!
//! The timings are tasticsplitter's, which learned them from real radios.

use crate::link::{self, Link, Transport};
use crate::mesh::{Event, LinkState, MeshState};
use meshctl_proto::meshtastic::{
    AdminMessage, Channel, Config, Data, FromRadio, Heartbeat, MeshPacket, ModuleConfig, PortNum,
    RouteDiscovery, ToRadio, User, admin_message, from_radio, mesh_packet, to_radio,
};
use prost::Message;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// First delay before reconnecting; it doubles on every failure up to
/// [`RECONNECT_MAX`].
const RECONNECT_MIN: Duration = Duration::from_secs(5);
const RECONNECT_MAX: Duration = Duration::from_secs(300);
/// A session that stayed up this long resets the reconnection backoff.
const STABLE_SESSION: Duration = Duration::from_secs(60);
/// A healthy node answers want_config in under a second, but a struggling one
/// has been seen to take a full minute.
const SYNC_TIMEOUT: Duration = Duration::from_secs(120);
/// The official clients send one every five minutes; on serial the firmware
/// drops the API session when it hears nothing for a while.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(300);
/// want_config nonces with a meaning of their own (only the config, only the
/// nodes), which a nonce of ours must not collide with.
const SPECIAL_NONCES: [u32; 2] = [69420, 69421];
/// How many events a slow follower may fall behind before it misses some.
const EVENT_BACKLOG: usize = 1024;

/// The longest text a message may carry, in bytes. The firmware takes a few
/// more, but a margin keeps clear of what other clients and versions accept.
pub const MAX_TEXT_BYTES: usize = 200;

/// A text to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingText {
    /// [`crate::mesh::BROADCAST`] for a channel message, a node for a direct one.
    pub to: u32,
    /// The channel's slot on the radio.
    pub channel: u32,
    pub text: String,
    /// The packet it answers, if any.
    pub reply_id: Option<u32>,
}

/// Why a text could not be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The radio is not connected and in sync.
    NotConnected,
    /// Empty, or longer than [`MAX_TEXT_BYTES`].
    BadText,
}

/// The mesh as meshctl knows it, shared between the radio task, which writes
/// it, and whoever reads it. Cheap to clone.
#[derive(Clone)]
pub struct Mesh {
    state: Arc<RwLock<MeshState>>,
    events: broadcast::Sender<Event>,
    /// Where to write to the radio, while connected.
    outbox: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    /// The id the next packet sent gets.
    next_id: Arc<AtomicU32>,
    /// The want_config nonce the radio's answer is expected with.
    nonce: Arc<AtomicU32>,
    /// Admin requests waiting for their answer, by packet id.
    waiting: Arc<Mutex<HashMap<u32, oneshot::Sender<Vec<u8>>>>>,
    /// When the last sync started, which the radio has [`SYNC_TIMEOUT`] to
    /// answer from.
    sync_started: Arc<Mutex<Instant>>,
}

/// A change to the radio's own settings, as the protobufs carry them, each
/// part whole: whatever is not to change in it goes as it already is.
#[derive(Debug, Clone, Default)]
pub struct SettingsChange {
    pub owner: Option<User>,
    pub configs: Vec<Config>,
    pub module_configs: Vec<ModuleConfig>,
    pub channels: Vec<Channel>,
}

/// Why the radio's settings were not changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminError {
    /// The radio is not connected and in sync.
    NotConnected,
    /// The radio did not answer the request a change starts with.
    NoAnswer,
}

/// How long the radio has to answer an admin request.
const ADMIN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long after changing settings to read them back: the radio applies them
/// after queueing them, and may reboot to.
const ADMIN_RESYNC_DELAY: Duration = Duration::from_secs(3);

impl Default for Mesh {
    fn default() -> Self {
        Self::new(MeshState::default())
    }
}

impl Mesh {
    pub fn new(state: MeshState) -> Self {
        Self {
            state: Arc::new(RwLock::new(state)),
            events: broadcast::channel(EVENT_BACKLOG).0,
            outbox: Arc::default(),
            // Ids that start somewhere else every run, so that a restart does
            // not reuse the ones still being answered.
            next_id: Arc::new(AtomicU32::new(nonce())),
            nonce: Arc::new(AtomicU32::new(0)),
            waiting: Arc::default(),
            sync_started: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Sends a text, asking for an acknowledgement. Returns the packet's id,
    /// which the [`Event::Ack`] that answers it carries.
    pub fn send_text(&self, outgoing: &OutgoingText) -> Result<u32, SendError> {
        if outgoing.text.trim().is_empty() || outgoing.text.len() > MAX_TEXT_BYTES {
            return Err(SendError::BadText);
        }
        self.send_packet(
            outgoing.to,
            outgoing.channel,
            true,
            Data {
                portnum: PortNum::TextMessageApp as i32,
                payload: outgoing.text.as_bytes().to_vec(),
                reply_id: outgoing.reply_id.unwrap_or_default(),
                ..Default::default()
            },
        )
    }

    /// Asks a node for the route to it and back. Returns the request's id,
    /// which the [`crate::mesh::Event::Traceroute`] that answers it carries; a
    /// relay that gives up says so with an [`crate::mesh::Event::Ack`] for it.
    pub fn send_traceroute(&self, to: u32, channel: u32) -> Result<u32, SendError> {
        self.send_packet(
            to,
            channel,
            false,
            Data {
                portnum: PortNum::TracerouteApp as i32,
                payload: RouteDiscovery::default().encode_to_vec(),
                want_response: true,
                ..Default::default()
            },
        )
    }

    /// Hands a packet to the radio, which fills in who it is from and, left
    /// at zero, the hop limit it is configured with.
    fn send_packet(
        &self,
        to: u32,
        channel: u32,
        want_ack: bool,
        data: Data,
    ) -> Result<u32, SendError> {
        if self.read(|state| state.link) != LinkState::Ready {
            return Err(SendError::NotConnected);
        }
        let outbox = self
            .outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let outbox = outbox.ok_or(SendError::NotConnected)?;

        let id = self.next_packet_id();
        let packet = MeshPacket {
            to,
            channel,
            id,
            want_ack,
            payload_variant: Some(mesh_packet::PayloadVariant::Decoded(data)),
            ..Default::default()
        };
        let message = ToRadio {
            payload_variant: Some(to_radio::PayloadVariant::Packet(packet)),
        };
        outbox
            .send(message.encode_to_vec())
            .map_err(|_| SendError::NotConnected)?;
        Ok(id)
    }

    /// Reads the state. Keep it short: the radio waits while it lasts.
    pub fn read<R>(&self, f: impl FnOnce(&MeshState) -> R) -> R {
        f(&self.state.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// Everything that changes from now on. A follower that falls more than
    /// [`EVENT_BACKLOG`] events behind is told it lagged, and should read the
    /// state again.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    fn update(&self, f: impl FnOnce(&mut MeshState) -> Vec<Event>) {
        let events = f(&mut self.state.write().unwrap_or_else(|e| e.into_inner()));
        for event in events {
            if let Event::AdminAnswer {
                request_id,
                passkey,
            } = event
            {
                let waiting = self
                    .waiting
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(passkey);
                }
                continue;
            }
            // Nobody listening is fine.
            let _ = self.events.send(event);
        }
    }

    /// Changes the radio's own settings, all at once: the radio saves them,
    /// and reboots if any of them needs it, once. They are read back a moment
    /// later, and arrive as a sync.
    pub async fn change_settings(&self, change: SettingsChange) -> Result<(), AdminError> {
        let me = self
            .read(|state| state.local.as_ref().map(|l| l.num))
            .ok_or(AdminError::NotConnected)?;

        // Changing settings takes the passkey the radio gives out with what it
        // answers; asking who it is is the cheapest way to have one.
        let (answer, answered) = oneshot::channel();
        let request = self.admin(
            me,
            admin_message::PayloadVariant::GetOwnerRequest(true),
            vec![],
        )?;
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request.0, answer);
        self.send_raw(request.1)?;
        let passkey = match tokio::time::timeout(ADMIN_TIMEOUT, answered).await {
            Ok(Ok(passkey)) => passkey,
            _ => {
                self.waiting
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request.0);
                return Err(AdminError::NoAnswer);
            }
        };

        use admin_message::PayloadVariant as A;
        let mut steps = vec![A::BeginEditSettings(true)];
        steps.extend(change.owner.map(A::SetOwner));
        steps.extend(change.configs.into_iter().map(A::SetConfig));
        steps.extend(change.module_configs.into_iter().map(A::SetModuleConfig));
        steps.extend(change.channels.into_iter().map(A::SetChannel));
        steps.push(A::CommitEditSettings(true));
        for step in steps {
            let (_, packet) = self.admin(me, step, passkey.clone())?;
            self.send_raw(packet)?;
        }

        let mesh = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(ADMIN_RESYNC_DELAY).await;
            // If the radio rebooted, the link dropped and the next one syncs.
            let _ = mesh.resync();
        });
        Ok(())
    }

    /// An admin message to the radio itself, as the packet that carries it,
    /// and that packet's id.
    fn admin(
        &self,
        me: u32,
        variant: admin_message::PayloadVariant,
        passkey: Vec<u8>,
    ) -> Result<(u32, Vec<u8>), AdminError> {
        if self.read(|state| state.link) != LinkState::Ready {
            return Err(AdminError::NotConnected);
        }
        let admin = AdminMessage {
            payload_variant: Some(variant),
            session_passkey: passkey,
        };
        let id = self.next_packet_id();
        let packet = MeshPacket {
            to: me,
            id,
            payload_variant: Some(mesh_packet::PayloadVariant::Decoded(Data {
                portnum: PortNum::AdminApp as i32,
                payload: admin.encode_to_vec(),
                want_response: true,
                ..Default::default()
            })),
            ..Default::default()
        };
        let message = ToRadio {
            payload_variant: Some(to_radio::PayloadVariant::Packet(packet)),
        };
        Ok((id, message.encode_to_vec()))
    }

    /// Writes a ToRadio, already encoded, to the radio.
    fn send_raw(&self, payload: Vec<u8>) -> Result<(), AdminError> {
        let outbox = self
            .outbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(AdminError::NotConnected)?;
        outbox.send(payload).map_err(|_| AdminError::NotConnected)
    }

    /// Asks the radio for its configuration and node list again, as on
    /// connecting.
    fn resync(&self) -> Result<(), AdminError> {
        let nonce = nonce();
        self.nonce.store(nonce, Ordering::Relaxed);
        self.mark_sync_start();
        self.set_link(LinkState::Syncing);
        let message = ToRadio {
            payload_variant: Some(to_radio::PayloadVariant::WantConfigId(nonce)),
        };
        self.send_raw(message.encode_to_vec())
    }

    fn mark_sync_start(&self) {
        *self.sync_started.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn sync_started(&self) -> Instant {
        *self.sync_started.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn next_packet_id(&self) -> u32 {
        // Ids are 32 bits but the firmware keeps the top one clear, and never
        // uses zero.
        (self.next_id.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff).max(1)
    }

    fn set_link(&self, link: LinkState) {
        self.update(|state| {
            if state.link == link {
                return Vec::new();
            }
            state.link = link;
            let mut events = vec![Event::Link(link)];
            if link == LinkState::Ready {
                events.push(Event::Synced);
            }
            events
        });
    }

    /// Connects to the radio and keeps connected, in a task of its own.
    pub fn spawn(&self, transport: Transport) -> JoinHandle<()> {
        let mesh = self.clone();
        tokio::spawn(async move { mesh.run(transport).await })
    }

    async fn run(self, transport: Transport) {
        let mut delay = RECONNECT_MIN;
        loop {
            self.set_link(LinkState::Connecting);
            match link::connect(&transport).await {
                Ok(link) => {
                    tracing::info!(radio = %transport, "connected");
                    let started = Instant::now();
                    let why = self.session(link).await;
                    if started.elapsed() >= STABLE_SESSION {
                        delay = RECONNECT_MIN;
                    }
                    tracing::warn!(
                        radio = %transport,
                        retry_in = delay.as_secs(),
                        "{why}, reconnecting"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        radio = %transport,
                        %error,
                        retry_in = delay.as_secs(),
                        "cannot connect"
                    );
                }
            }
            self.set_link(LinkState::Disconnected);
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(RECONNECT_MAX);
        }
    }

    /// One connection, from asking for the configuration until it drops.
    /// Returns why it ended.
    async fn session(&self, link: Link) -> &'static str {
        *self.outbox.lock().unwrap_or_else(|e| e.into_inner()) = Some(link.tx.clone());
        let why = self.exchange(link).await;
        *self.outbox.lock().unwrap_or_else(|e| e.into_inner()) = None;
        why
    }

    async fn exchange(&self, mut link: Link) -> &'static str {
        let nonce = nonce();
        self.nonce.store(nonce, Ordering::Relaxed);
        self.mark_sync_start();
        self.set_link(LinkState::Syncing);
        if send(&link, to_radio::PayloadVariant::WantConfigId(nonce)).is_err() {
            return "link closed";
        }

        let mut heartbeat =
            tokio::time::interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);

        loop {
            tokio::select! {
                payload = link.rx.recv() => {
                    let Some(payload) = payload else {
                        return "link closed";
                    };
                    let message = match FromRadio::decode(payload.as_slice()) {
                        Ok(message) => message,
                        Err(error) => {
                            tracing::debug!(%error, "undecodable frame from the radio");
                            continue;
                        }
                    };
                    match message.payload_variant {
                        // The nonce of the last request, which a resync renews.
                        Some(from_radio::PayloadVariant::ConfigCompleteId(id))
                            if id == self.nonce.load(Ordering::Relaxed) =>
                        {
                            let nodes = self.read(|state| state.nodes.len());
                            tracing::info!(nodes, "in sync with the radio");
                            self.set_link(LinkState::Ready);
                        }
                        _ => self.update(|state| state.apply(message, now())),
                    }
                }
                _ = heartbeat.tick() => {
                    if send(&link, to_radio::PayloadVariant::Heartbeat(Heartbeat::default())).is_err() {
                        return "link closed";
                    }
                }
                // From when the last sync started: on connecting, or a resync
                // later in the same session.
                () = tokio::time::sleep_until(self.sync_started() + SYNC_TIMEOUT),
                    if self.read(|s| s.link) == LinkState::Syncing =>
                {
                    return "the radio did not send its configuration";
                }
            }
        }
    }
}

fn send(link: &Link, variant: to_radio::PayloadVariant) -> Result<(), ()> {
    let message = ToRadio {
        payload_variant: Some(variant),
    };
    link.tx.send(message.encode_to_vec()).map_err(|_| ())
}

/// A want_config nonce that changes on every connection, so that the answer
/// to an earlier one is never taken for the answer to this one.
fn nonce() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.subsec_nanos());
    if SPECIAL_NONCES.contains(&nanos) || nanos == 0 {
        1
    } else {
        nanos
    }
}

/// Seconds since the epoch, the way the radio counts them.
fn now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_proto::frame::{self, FrameParser};
    use meshctl_proto::meshtastic::{self as pb, MyNodeInfo, NodeInfo, User};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// A radio at the other end of a socket, played by the test.
    struct FakeRadio {
        stream: TcpStream,
        parser: FrameParser,
        /// Frames read but not yet asked for: several can arrive in one read.
        pending: std::collections::VecDeque<Vec<u8>>,
    }

    impl FakeRadio {
        async fn receive(&mut self) -> ToRadio {
            let mut buf = [0u8; 512];
            loop {
                if let Some(payload) = self.pending.pop_front() {
                    return ToRadio::decode(payload.as_slice()).unwrap();
                }
                let n = self.stream.read(&mut buf).await.unwrap();
                assert!(n > 0, "meshctl hung up");
                self.pending.extend(self.parser.feed(&buf[..n]));
            }
        }

        async fn send(&mut self, variant: from_radio::PayloadVariant) {
            let message = FromRadio {
                id: 0,
                payload_variant: Some(variant),
            };
            let framed = frame::encode(&message.encode_to_vec()).unwrap();
            self.stream.write_all(&framed).await.unwrap();
        }
    }

    async fn connected() -> (Mesh, FakeRadio, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mesh = Mesh::default();
        let task = mesh.spawn(Transport::Tcp {
            host: "127.0.0.1".into(),
            port,
        });
        let (stream, _) = listener.accept().await.unwrap();
        let radio = FakeRadio {
            stream,
            parser: FrameParser::new(),
            pending: std::collections::VecDeque::new(),
        };
        (mesh, radio, task)
    }

    /// The next event that is not about getting connected, which depending on
    /// timing may or may not have happened before the test subscribed.
    async fn next_event(events: &mut broadcast::Receiver<Event>) -> Event {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("an event in time")
                .unwrap();
            if !matches!(
                event,
                Event::Link(LinkState::Connecting | LinkState::Syncing)
            ) {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn syncs_and_then_follows_the_mesh() {
        let (mesh, mut radio, task) = connected().await;
        let mut events = mesh.subscribe();

        let Some(to_radio::PayloadVariant::WantConfigId(nonce)) =
            radio.receive().await.payload_variant
        else {
            panic!("the first thing asked is the configuration");
        };

        radio
            .send(from_radio::PayloadVariant::MyInfo(MyNodeInfo {
                my_node_num: 1,
                ..Default::default()
            }))
            .await;
        radio
            .send(from_radio::PayloadVariant::NodeInfo(NodeInfo {
                num: 2,
                user: Some(User {
                    long_name: "Two".into(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await;
        // An answer to somebody else's request does not end ours.
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(
                nonce.wrapping_add(1),
            ))
            .await;
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(nonce))
            .await;

        assert_eq!(next_event(&mut events).await, Event::Link(LinkState::Ready));
        assert_eq!(next_event(&mut events).await, Event::Synced);
        mesh.read(|state| {
            assert_eq!(state.local.as_ref().unwrap().num, 1);
            assert_eq!(state.nodes[&2].user.as_ref().unwrap().long_name, "Two");
        });

        radio
            .send(from_radio::PayloadVariant::Packet(pb::MeshPacket {
                from: 3,
                ..Default::default()
            }))
            .await;
        let Event::Node(node) = next_event(&mut events).await else {
            panic!("a node event");
        };
        assert_eq!(node.num, 3);

        task.abort();
    }

    #[tokio::test]
    async fn a_text_goes_out_once_in_sync() {
        let (mesh, mut radio, task) = connected().await;
        let mut events = mesh.subscribe();
        let text = OutgoingText {
            to: crate::mesh::BROADCAST,
            channel: 4,
            text: "hola".into(),
            reply_id: None,
        };
        assert_eq!(mesh.send_text(&text), Err(SendError::NotConnected));

        let Some(to_radio::PayloadVariant::WantConfigId(nonce)) =
            radio.receive().await.payload_variant
        else {
            panic!("the configuration first");
        };
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(nonce))
            .await;
        assert_eq!(next_event(&mut events).await, Event::Link(LinkState::Ready));

        let id = mesh.send_text(&text).unwrap();
        let Some(to_radio::PayloadVariant::Packet(packet)) = radio.receive().await.payload_variant
        else {
            panic!("a packet");
        };
        assert_eq!(packet.id, id);
        assert_eq!(packet.channel, 4);
        assert!(packet.want_ack);
        let Some(mesh_packet::PayloadVariant::Decoded(data)) = packet.payload_variant else {
            panic!("in the clear");
        };
        assert_eq!(data.payload, b"hola");

        let too_long = OutgoingText {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
            ..text
        };
        assert_eq!(mesh.send_text(&too_long), Err(SendError::BadText));
        task.abort();
    }

    /// The admin message a packet to the radio carries.
    fn admin_of(message: ToRadio) -> (u32, pb::AdminMessage) {
        let Some(to_radio::PayloadVariant::Packet(packet)) = message.payload_variant else {
            panic!("a packet, not {message:?}");
        };
        let Some(mesh_packet::PayloadVariant::Decoded(data)) = packet.payload_variant else {
            panic!("in the clear");
        };
        assert_eq!(data.portnum, PortNum::AdminApp as i32);
        assert_eq!(packet.to, 1, "to the radio itself");
        (
            packet.id,
            pb::AdminMessage::decode(data.payload.as_slice()).unwrap(),
        )
    }

    #[tokio::test]
    async fn settings_change_at_once_with_the_passkey_given() {
        let (mesh, mut radio, task) = connected().await;
        let mut events = mesh.subscribe();
        let Some(to_radio::PayloadVariant::WantConfigId(nonce)) =
            radio.receive().await.payload_variant
        else {
            panic!("the configuration first");
        };
        radio
            .send(from_radio::PayloadVariant::MyInfo(MyNodeInfo {
                my_node_num: 1,
                ..Default::default()
            }))
            .await;
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(nonce))
            .await;
        assert_eq!(next_event(&mut events).await, Event::Link(LinkState::Ready));

        let change = SettingsChange {
            owner: Some(User {
                long_name: "Faro".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let changing = tokio::spawn({
            let mesh = mesh.clone();
            async move { mesh.change_settings(change).await }
        });

        // First the question that gets the passkey...
        let (asked, question) = admin_of(radio.receive().await);
        assert!(matches!(
            question.payload_variant,
            Some(admin_message::PayloadVariant::GetOwnerRequest(true))
        ));
        let answer = pb::AdminMessage {
            session_passkey: vec![7; 8],
            payload_variant: Some(admin_message::PayloadVariant::GetOwnerResponse(
                User::default(),
            )),
        };
        radio
            .send(from_radio::PayloadVariant::Packet(pb::MeshPacket {
                from: 1,
                payload_variant: Some(mesh_packet::PayloadVariant::Decoded(pb::Data {
                    portnum: PortNum::AdminApp as i32,
                    payload: answer.encode_to_vec(),
                    request_id: asked,
                    ..Default::default()
                })),
                ..Default::default()
            }))
            .await;

        // ...then the change, wrapped so that the radio applies it once.
        let mut steps = Vec::new();
        for _ in 0..3 {
            let (_, step) = admin_of(radio.receive().await);
            assert_eq!(step.session_passkey, vec![7; 8]);
            steps.push(step.payload_variant.unwrap());
        }
        assert!(matches!(
            steps[0],
            admin_message::PayloadVariant::BeginEditSettings(true)
        ));
        assert!(
            matches!(&steps[1], admin_message::PayloadVariant::SetOwner(u) if u.long_name == "Faro")
        );
        assert!(matches!(
            steps[2],
            admin_message::PayloadVariant::CommitEditSettings(true)
        ));
        assert_eq!(changing.await.unwrap(), Ok(()));
        task.abort();
    }

    /// What the radio is asked for next, skipping heartbeats.
    async fn next_request(radio: &mut FakeRadio) -> to_radio::PayloadVariant {
        loop {
            match radio.receive().await.payload_variant {
                Some(to_radio::PayloadVariant::Heartbeat(_)) => {}
                Some(other) => return other,
                None => panic!("an empty ToRadio"),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_resync_late_in_a_session_has_its_own_time() {
        let (mesh, mut radio, task) = connected().await;
        let mut events = mesh.subscribe();
        let to_radio::PayloadVariant::WantConfigId(nonce) = next_request(&mut radio).await else {
            panic!("the configuration first");
        };
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(nonce))
            .await;
        assert_eq!(next_event(&mut events).await, Event::Link(LinkState::Ready));

        // Long after the first sync's deadline would have passed.
        tokio::time::advance(SYNC_TIMEOUT * 2).await;
        mesh.resync().unwrap();
        let to_radio::PayloadVariant::WantConfigId(again) = next_request(&mut radio).await else {
            panic!("the configuration again");
        };
        assert_ne!(again, nonce);
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(again))
            .await;
        // In sync again, without having dropped the link on the way.
        loop {
            match next_event(&mut events).await {
                Event::Link(LinkState::Ready) => break,
                Event::Link(LinkState::Disconnected) => panic!("the link dropped"),
                _ => {}
            }
        }
        task.abort();
    }

    #[tokio::test]
    async fn a_dropped_radio_is_noticed() {
        let (mesh, mut radio, task) = connected().await;
        let mut events = mesh.subscribe();
        radio.receive().await;
        drop(radio);
        assert_eq!(
            next_event(&mut events).await,
            Event::Link(LinkState::Disconnected)
        );
        task.abort();
    }
}
