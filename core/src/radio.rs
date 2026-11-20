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
    Data, FromRadio, Heartbeat, MeshPacket, PortNum, RouteDiscovery, ToRadio, from_radio,
    mesh_packet, to_radio,
};
use prost::Message;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};
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
}

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

        // Ids are 32 bits but the firmware keeps the top one clear, and never
        // uses zero.
        let id = (self.next_id.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff).max(1);
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
            // Nobody listening is fine.
            let _ = self.events.send(event);
        }
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
        self.set_link(LinkState::Syncing);
        if send(&link, to_radio::PayloadVariant::WantConfigId(nonce)).is_err() {
            return "link closed";
        }

        let sync_deadline = tokio::time::sleep(SYNC_TIMEOUT);
        tokio::pin!(sync_deadline);
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
                        Some(from_radio::PayloadVariant::ConfigCompleteId(id)) if id == nonce => {
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
                () = &mut sync_deadline, if self.read(|s| s.link) == LinkState::Syncing => {
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
    }

    impl FakeRadio {
        async fn receive(&mut self) -> ToRadio {
            let mut buf = [0u8; 512];
            loop {
                if let Some(payload) = self.parser.feed(&[]).into_iter().next() {
                    return ToRadio::decode(payload.as_slice()).unwrap();
                }
                let n = self.stream.read(&mut buf).await.unwrap();
                assert!(n > 0, "meshctl hung up");
                if let Some(payload) = self.parser.feed(&buf[..n]).into_iter().next() {
                    return ToRadio::decode(payload.as_slice()).unwrap();
                }
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
