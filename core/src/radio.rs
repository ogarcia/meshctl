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
    AdminMessage, Channel, Config, Data, DeviceMetrics, EnvironmentMetrics, FromRadio, Heartbeat,
    LocalStats, MeshPacket, ModuleConfig, NeighborInfo, PortNum, Position, PowerMetrics,
    RouteDiscovery, SharedContact, Telemetry, ToRadio, User, admin_message, from_radio,
    mesh_packet, telemetry, to_radio,
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
pub(crate) const RECONNECT_MIN: Duration = Duration::from_secs(5);
pub(crate) const RECONNECT_MAX: Duration = Duration::from_secs(300);
/// A session that stayed up this long resets the reconnection backoff.
pub(crate) const STABLE_SESSION: Duration = Duration::from_secs(60);
/// A healthy node answers want_config in under a second, but a struggling one
/// has been seen to take a full minute.
const SYNC_TIMEOUT: Duration = Duration::from_secs(120);
/// How often a radio in sync is read again, quietly: to know which nodes it
/// has, as it drops some once its list is full, or another client removes
/// them, without telling.
pub(crate) const REREAD_INTERVAL: Duration = Duration::from_secs(3600);
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
    /// Whether it is a reaction to `reply_id`: its text an emoji.
    pub emoji: bool,
    /// The region it floods in, for this message alone; none for what the
    /// radio would. Only MeshCore radios have regions.
    pub scope: Option<crate::mesh::Scope>,
    /// The clock it goes by, for a MeshCore channel message sent again: the
    /// same packet, so that what repeaters pass on is told as the first;
    /// none for now.
    pub sent_at: Option<u32>,
}

/// Why a text could not be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The radio is not connected and in sync.
    NotConnected,
    /// Empty, or longer than [`MAX_TEXT_BYTES`].
    BadText,
    /// Not something this kind of radio does.
    Unsupported,
    /// The radio would not take it.
    Refused,
}

/// What a node can be asked for, and answers on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Who it is.
    NodeInfo,
    /// Where it is.
    Position,
    /// Its battery, voltage and how busy the channel is.
    DeviceTelemetry,
    /// What its environment sensors read, if it has any.
    EnvironmentTelemetry,
    /// What its power sensors read, if it has any.
    PowerTelemetry,
    /// How it sees the mesh: nodes online, packets, noise.
    LocalStats,
    /// The nodes it hears directly.
    Neighbors,
}

/// Which mesh a radio speaks, and so which driver keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Meshtastic,
    Meshcore,
}

/// A text handed to the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// The id its acknowledgement, if any, comes back with.
    pub packet_id: u32,
    /// Whether an acknowledgement can come at all. A MeshCore channel message
    /// has none: once the radio took it, that is all there is to know.
    pub acknowledged: bool,
    /// When it went, by the clock it was sent by: on MeshCore, where replies
    /// and reactions name a message by it.
    pub sent_at: Option<u32>,
}

/// The mesh as meshctl knows it, shared between the radio task, which writes
/// it, and whoever reads it. Cheap to clone.
#[derive(Clone)]
pub struct Mesh {
    pub(crate) state: Arc<RwLock<MeshState>>,
    pub(crate) events: broadcast::Sender<Event>,
    /// The MeshCore session, while one is open; what sending goes through.
    pub(crate) meshcore: Arc<Mutex<Option<crate::meshcore::Session>>>,
    /// Where to write to the radio, while connected.
    outbox: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    /// The id the next packet sent gets.
    next_id: Arc<AtomicU32>,
    /// The want_config nonce the radio's answer is expected with.
    nonce: Arc<AtomicU32>,
    /// Admin requests waiting for their answer, by packet id.
    waiting: Arc<Mutex<HashMap<u32, oneshot::Sender<AdminMessage>>>>,
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
    /// Where the radio is, as set by hand, or no longer: sent after the
    /// configuration, which would otherwise undo it.
    pub fixed_position: Option<FixedPosition>,
}

/// What can be done to a node as its admin, the radio itself or another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeAction {
    /// It reboots, in a few seconds.
    Reboot,
    /// It shuts down, in a few seconds, until someone switches it on.
    Shutdown,
    /// It forgets the nodes it knows, but its favourites.
    ForgetNodes,
    /// Its configuration goes back to the factory's, keeping its Bluetooth
    /// pairings.
    FactoryReset,
}

impl NodeAction {
    fn request(self) -> admin_message::PayloadVariant {
        use admin_message::PayloadVariant as A;
        match self {
            Self::Reboot => A::RebootSeconds(ACTION_DELAY),
            Self::Shutdown => A::ShutdownSeconds(ACTION_DELAY),
            Self::ForgetNodes => A::NodedbReset(true),
            Self::FactoryReset => A::FactoryResetConfig(1),
        }
    }
}

/// How many seconds a node waits to reboot or shut down: time enough to
/// say it took it.
const ACTION_DELAY: i32 = 5;

/// A position set by hand on the radio, or let go.
#[derive(Debug, Clone, PartialEq)]
pub enum FixedPosition {
    /// The radio says it is there, instead of what its GPS reads.
    Set(Position),
    /// The radio goes back to its GPS, if it has one.
    Remove,
}

/// Why the radio's settings were not changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    /// The radio is not connected and in sync.
    NotConnected,
    /// The radio did not answer the request a change starts with.
    NoAnswer,
    /// Not something this kind of radio does yet.
    Unsupported,
    /// A node the radio does not know of.
    UnknownNode,
    /// The radio's own node, which it cannot forget.
    OwnNode,
    /// A node the radio knows no path to, for what goes to it only direct.
    NoPath,
    /// The radio would not do it, and said why, as the protobufs name it.
    Refused(String),
}

/// The admin messages a change of settings takes, in their order: within
/// one edit, so that the radio saves them, and reboots, once.
fn steps(change: SettingsChange) -> Vec<admin_message::PayloadVariant> {
    use admin_message::PayloadVariant as A;
    let mut steps = vec![A::BeginEditSettings(true)];
    steps.extend(change.owner.map(A::SetOwner));
    steps.extend(change.configs.into_iter().map(A::SetConfig));
    steps.extend(change.module_configs.into_iter().map(A::SetModuleConfig));
    steps.extend(change.channels.into_iter().map(A::SetChannel));
    steps.extend(change.fixed_position.map(|fixed| match fixed {
        FixedPosition::Set(position) => A::SetFixedPosition(position),
        FixedPosition::Remove => A::RemoveFixedPosition(true),
    }));
    steps.push(A::CommitEditSettings(true));
    steps
}

/// How long the radio has to answer an admin request.
const ADMIN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long another node has to answer one over the mesh, a few hops each
/// way, and resent if lost.
const REMOTE_ADMIN_TIMEOUT: Duration = Duration::from_secs(60);

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
            meshcore: Arc::default(),
            outbox: Arc::default(),
            // Ids that start somewhere else every run, so that a restart does
            // not reuse the ones still being answered.
            next_id: Arc::new(AtomicU32::new(nonce())),
            nonce: Arc::new(AtomicU32::new(0)),
            waiting: Arc::default(),
            sync_started: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Sends a text, asking for an acknowledgement where there can be one,
    /// which comes back as an [`Event::Ack`] with the id returned.
    pub async fn send_text(&self, outgoing: &OutgoingText) -> Result<Sent, SendError> {
        if outgoing.text.trim().is_empty() || outgoing.text.len() > MAX_TEXT_BYTES {
            return Err(SendError::BadText);
        }
        if self.read(|state| state.link) != LinkState::Ready {
            return Err(SendError::NotConnected);
        }
        let meshcore = self
            .meshcore
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(session) = meshcore {
            // MeshCore has neither replies nor reactions.
            if outgoing.emoji {
                return Err(SendError::Unsupported);
            }
            return session.send_text(self, outgoing).await;
        }
        if outgoing.emoji && outgoing.reply_id.is_none() {
            return Err(SendError::BadText);
        }
        let packet_id = self.send_packet(
            outgoing.to,
            outgoing.channel,
            true,
            Data {
                portnum: PortNum::TextMessageApp as i32,
                payload: outgoing.text.as_bytes().to_vec(),
                reply_id: outgoing.reply_id.unwrap_or_default(),
                emoji: u32::from(outgoing.emoji),
                ..Default::default()
            },
        )?;
        Ok(Sent {
            packet_id,
            acknowledged: true,
            sent_at: None,
        })
    }

    /// Sends where the radio is to a channel or a node, as a position of
    /// its own: the firmware rounds it as the channel it goes on shares
    /// positions. To a node, it asks for the node's back, which comes as a
    /// position with the id returned as its `request_id`.
    pub fn send_position(
        &self,
        to: u32,
        channel: u32,
        position: Position,
    ) -> Result<u32, SendError> {
        if self.is_meshcore() {
            return Err(SendError::Unsupported);
        }
        self.send_packet(
            to,
            channel,
            true,
            Data {
                portnum: PortNum::PositionApp as i32,
                payload: position.encode_to_vec(),
                want_response: to != crate::mesh::BROADCAST,
                ..Default::default()
            },
        )
    }

    /// Whether the radio is a MeshCore one, as far as its open session says.
    fn is_meshcore(&self) -> bool {
        self.meshcore
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Asks a node for the route to it and back. Returns the request's id,
    /// which the [`crate::mesh::Event::Traceroute`] that answers it carries; a
    /// relay that gives up says so with an [`crate::mesh::Event::Ack`] for it.
    pub fn send_traceroute(&self, to: u32, channel: u32) -> Result<u32, SendError> {
        if self.is_meshcore() {
            return Err(SendError::Unsupported);
        }
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

    /// Asks a node for something it tells on request. Returns the request's
    /// id, which the answer carries as its `request_id`, as a
    /// [`crate::mesh::Event::Packet`]; a node that has nothing to tell, or a
    /// relay that gives up, says so with an [`crate::mesh::Event::Ack`] for it.
    ///
    /// The node answers on the channel asked on, if it has it: a position, at
    /// the precision that channel gives.
    pub fn send_request(&self, to: u32, channel: u32, ask: Ask) -> Result<u32, SendError> {
        if self.is_meshcore() {
            return Err(SendError::Unsupported);
        }
        let telemetry = |variant: telemetry::Variant| Telemetry {
            variant: Some(variant),
            ..Default::default()
        };
        let (portnum, payload) = match ask {
            // Who we are, as the official clients send it: the node answers
            // with who it is.
            Ask::NodeInfo => (
                PortNum::NodeinfoApp,
                self.read(|state| state.settings.owner.clone())
                    .unwrap_or_default()
                    .encode_to_vec(),
            ),
            // An empty position, which tells nothing of ours.
            Ask::Position => (PortNum::PositionApp, Vec::new()),
            Ask::DeviceTelemetry => (
                PortNum::TelemetryApp,
                telemetry(telemetry::Variant::DeviceMetrics(DeviceMetrics::default()))
                    .encode_to_vec(),
            ),
            Ask::EnvironmentTelemetry => (
                PortNum::TelemetryApp,
                telemetry(telemetry::Variant::EnvironmentMetrics(
                    EnvironmentMetrics::default(),
                ))
                .encode_to_vec(),
            ),
            Ask::PowerTelemetry => (
                PortNum::TelemetryApp,
                telemetry(telemetry::Variant::PowerMetrics(PowerMetrics::default()))
                    .encode_to_vec(),
            ),
            Ask::LocalStats => (
                PortNum::TelemetryApp,
                telemetry(telemetry::Variant::LocalStats(LocalStats::default())).encode_to_vec(),
            ),
            Ask::Neighbors => (
                PortNum::NeighborinfoApp,
                NeighborInfo::default().encode_to_vec(),
            ),
        };
        self.send_packet(
            to,
            channel,
            false,
            Data {
                portnum: portnum as i32,
                payload,
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

    pub(crate) fn update(&self, f: impl FnOnce(&mut MeshState) -> Vec<Event>) {
        let events = f(&mut self.state.write().unwrap_or_else(|e| e.into_inner()));
        for event in events {
            if let Event::AdminAnswer {
                request_id, admin, ..
            } = event
            {
                let waiting = self
                    .waiting
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(*admin);
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
        if self.is_meshcore() {
            return Err(AdminError::Unsupported);
        }
        let (me, passkey) = self.passkey().await?;
        for step in steps(change) {
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

    /// Has a node do something as its admin: the radio itself, or another
    /// node over the mesh, sealed to it with the passkey it gives out. Once
    /// it acknowledges it.
    pub async fn act(&self, node: Option<u32>, action: NodeAction) -> Result<(), AdminError> {
        if self.is_meshcore() {
            return Err(AdminError::Unsupported);
        }
        match node {
            None => {
                let (me, passkey) = self.passkey().await?;
                self.acknowledged(me, action.request(), passkey).await
            }
            Some(to) => {
                let passkey = self
                    .ask_node(to, admin_message::PayloadVariant::GetOwnerRequest(true))
                    .await?
                    .session_passkey;
                self.delivered(to, action.request(), passkey).await
            }
        }
    }

    /// Changes another node's settings over the mesh, as its admin, all at
    /// once as the radio's own are: sealed to it, with the passkey it gives
    /// out when asked who it is, each step waited for until the node says
    /// it has it. It saves them, and reboots if any of them needs it.
    pub async fn change_node(&self, to: u32, change: SettingsChange) -> Result<(), AdminError> {
        let passkey = self
            .ask_node(to, admin_message::PayloadVariant::GetOwnerRequest(true))
            .await?
            .session_passkey;
        for step in steps(change) {
            self.delivered(to, step, passkey.clone()).await?;
        }
        Ok(())
    }

    /// Sends an admin message to another node, sealed to it, and waits until
    /// the node itself acknowledges it: a relay heard passing it on is not
    /// enough. Or why not.
    async fn delivered(
        &self,
        to: u32,
        variant: admin_message::PayloadVariant,
        passkey: Vec<u8>,
    ) -> Result<(), AdminError> {
        let (id, packet) = self.admin_to(to, variant, passkey, true)?;
        let mut events = self.subscribe();
        self.send_raw(packet)?;
        let outcome = tokio::time::timeout(REMOTE_ADMIN_TIMEOUT, async {
            loop {
                match events.recv().await {
                    Ok(Event::Ack(ack)) if ack.packet_id == id => match ack.error {
                        Some(reason) => return Some(Err(AdminError::Refused(reason))),
                        None if ack.from == to => return Some(Ok(())),
                        None => {}
                    },
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .await;
        outcome.ok().flatten().unwrap_or(Err(AdminError::NoAnswer))
    }

    /// Marks a node a favourite of the radio, or no longer: the radio keeps
    /// it in its node list however long it goes unheard. Nothing goes on the
    /// air; the node changes here at once, as the radio does not tell.
    pub async fn set_favorite(&self, num: u32, favorite: bool) -> Result<(), AdminError> {
        if self.is_meshcore() {
            return Err(AdminError::Unsupported);
        }
        if !self.read(|state| state.nodes.contains_key(&num)) {
            return Err(AdminError::UnknownNode);
        }
        let (me, passkey) = self.passkey().await?;
        let variant = if favorite {
            admin_message::PayloadVariant::SetFavoriteNode(num)
        } else {
            admin_message::PayloadVariant::RemoveFavoriteNode(num)
        };
        let (_, packet) = self.admin(me, variant, passkey)?;
        self.send_raw(packet)?;
        self.update(|state| {
            state.nodes.get_mut(&num).map_or_else(Vec::new, |node| {
                node.favorite = favorite;
                vec![Event::Node(node.clone())]
            })
        });
        Ok(())
    }

    /// Has the radio forget a node: out of its node list, until it is heard
    /// again. Here it stays, as not in the radio's list. Nothing goes on the
    /// air; nothing is asked of the radio if it is known not to have it.
    pub async fn remove_node(&self, num: u32) -> Result<(), AdminError> {
        let Some(on_radio) = self.read(|state| state.nodes.get(&num).map(|n| n.on_radio)) else {
            return Err(AdminError::UnknownNode);
        };
        if on_radio == Some(false) {
            return Ok(());
        }
        // A MeshCore radio forgets it as one of its contacts.
        if self.is_meshcore() {
            return self.meshcore_remove_contact(num).await;
        }
        let (me, passkey) = self.passkey().await?;
        if num == me {
            return Err(AdminError::OwnNode);
        }
        // Forgotten once the radio says so. Until then, nothing here changes,
        // nor anything kept of it.
        self.acknowledged(
            me,
            admin_message::PayloadVariant::RemoveByNodenum(num),
            passkey,
        )
        .await?;
        self.update(|state| match state.nodes.get_mut(&num) {
            Some(node) => {
                node.on_radio = Some(false);
                vec![Event::Node(node.clone())]
            }
            None => Vec::new(),
        });
        Ok(())
    }

    /// Lets go of a node here too, as meshctl forgets it: out of the list
    /// until it is heard again.
    pub fn drop_node(&self, num: u32) {
        self.update(|state| {
            if state.nodes.remove(&num).is_some() {
                vec![Event::NodeRemoved(num)]
            } else {
                Vec::new()
            }
        });
    }

    /// Has the radio keep a node from a shared contact: who it is and its
    /// key above all, so that it can be written to privately before it is
    /// heard. Nothing goes on the air; the node is here once the radio says
    /// it keeps it.
    pub async fn add_contact(&self, contact: SharedContact) -> Result<(), AdminError> {
        if self.is_meshcore() {
            return Err(AdminError::Unsupported);
        }
        let Some(user) = contact.user.clone() else {
            return Err(AdminError::Refused("NO_USER".into()));
        };
        let (me, passkey) = self.passkey().await?;
        if contact.node_num == me {
            return Err(AdminError::OwnNode);
        }
        let num = contact.node_num;
        self.acknowledged(
            me,
            admin_message::PayloadVariant::AddContact(contact),
            passkey,
        )
        .await?;
        self.update(|state| {
            let node = state
                .nodes
                .entry(num)
                .or_insert_with(|| crate::mesh::Node::new(num));
            node.user = Some(crate::mesh::user_from(user));
            node.on_radio = Some(true);
            vec![Event::Node(node.clone())]
        });
        Ok(())
    }

    /// Sends an admin message to the radio and waits for what it says of it:
    /// it answers what it does on its own with an acknowledgement, or why
    /// not.
    async fn acknowledged(
        &self,
        me: u32,
        variant: admin_message::PayloadVariant,
        passkey: Vec<u8>,
    ) -> Result<(), AdminError> {
        let (id, packet) = self.admin(me, variant, passkey)?;
        let mut events = self.subscribe();
        self.send_raw(packet)?;
        let acknowledged = tokio::time::timeout(ADMIN_TIMEOUT, async {
            loop {
                match events.recv().await {
                    Ok(Event::Ack(ack)) if ack.packet_id == id => return Some(ack.error),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .await;
        match acknowledged {
            Ok(Some(None)) => Ok(()),
            Ok(Some(Some(reason))) => Err(AdminError::Refused(reason)),
            Ok(None) | Err(_) => Err(AdminError::NoAnswer),
        }
    }

    /// The radio's own number, and the passkey changing it takes, which it
    /// gives out with what it answers: asking who it is is the cheapest way
    /// to have one.
    async fn passkey(&self) -> Result<(u32, Vec<u8>), AdminError> {
        let me = self
            .read(|state| state.local.as_ref().map(|l| l.num))
            .ok_or(AdminError::NotConnected)?;
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
        match tokio::time::timeout(ADMIN_TIMEOUT, answered).await {
            Ok(Ok(answer)) => Ok((me, answer.session_passkey)),
            _ => {
                self.waiting
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request.0);
                Err(AdminError::NoAnswer)
            }
        }
    }

    /// Asks another node for something of its own, part of its settings,
    /// over the mesh: sealed to its key, as the firmware takes admin
    /// requests only from a key it is told to, and answers so. Its answer,
    /// which carries the passkey a change to it takes; or why none.
    pub async fn ask_node(
        &self,
        to: u32,
        request: admin_message::PayloadVariant,
    ) -> Result<AdminMessage, AdminError> {
        if self.is_meshcore() {
            return Err(AdminError::Unsupported);
        }
        let (me, known) = self.read(|state| {
            (
                state.local.as_ref().map(|l| l.num),
                state.nodes.contains_key(&to),
            )
        });
        let me = me.ok_or(AdminError::NotConnected)?;
        if to == me {
            return Err(AdminError::OwnNode);
        }
        if !known {
            return Err(AdminError::UnknownNode);
        }
        let (id, packet) = self.admin_to(to, request, vec![], true)?;
        let (answer, answered) = oneshot::channel();
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, answer);
        let mut events = self.subscribe();
        let sent = self.send_raw(packet);
        // Refused, by the node or by the radio on its way: as the routing
        // error that answers the packet.
        let refused = async move {
            loop {
                match events.recv().await {
                    Ok(Event::Ack(ack)) if ack.packet_id == id && ack.error.is_some() => {
                        return ack.error;
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        };
        let outcome = match sent {
            Ok(()) => tokio::time::timeout(REMOTE_ADMIN_TIMEOUT, async {
                tokio::select! {
                    answer = answered => answer.ok().map(Ok),
                    reason = refused => reason.map(|r| Err(AdminError::Refused(r))),
                }
            })
            .await
            .ok()
            .flatten(),
            Err(error) => Some(Err(error)),
        };
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        outcome.unwrap_or(Err(AdminError::NoAnswer))
    }

    /// An admin message to the radio itself, as the packet that carries it,
    /// and that packet's id.
    fn admin(
        &self,
        me: u32,
        variant: admin_message::PayloadVariant,
        passkey: Vec<u8>,
    ) -> Result<(u32, Vec<u8>), AdminError> {
        self.admin_to(me, variant, passkey, false)
    }

    /// An admin message to a node, the radio itself or another, sealed to
    /// its key if said, as the packet that carries it, and that packet's id.
    fn admin_to(
        &self,
        to: u32,
        variant: admin_message::PayloadVariant,
        passkey: Vec<u8>,
        sealed: bool,
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
            to,
            id,
            pki_encrypted: sealed,
            want_ack: sealed,
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

    pub(crate) fn set_link(&self, link: LinkState) {
        self.update(|state| {
            if state.link == link {
                return Vec::new();
            }
            state.link_to(link);
            state.link_since = Some(now());
            let mut events = vec![Event::Link(link)];
            if link == LinkState::Ready {
                events.push(Event::Synced);
            }
            events
        });
    }

    /// Connects to the radio and keeps connected, in a task of its own, with
    /// the driver for the mesh it speaks.
    pub fn spawn(&self, protocol: Protocol, transport: Transport) -> JoinHandle<()> {
        let mesh = self.clone();
        match protocol {
            Protocol::Meshtastic => tokio::spawn(async move { mesh.run(transport).await }),
            Protocol::Meshcore => {
                tokio::spawn(async move { crate::meshcore::run(mesh, transport).await })
            }
        }
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
        let mut reread =
            tokio::time::interval_at(Instant::now() + REREAD_INTERVAL, REREAD_INTERVAL);

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
                            if self.read(|state| state.rereading) {
                                tracing::debug!(nodes, "read the radio again");
                                self.update(MeshState::reread_done);
                            } else {
                                tracing::info!(nodes, "in sync with the radio");
                                self.set_link(LinkState::Ready);
                            }
                        }
                        _ => self.update(|state| state.apply(message, now())),
                    }
                }
                _ = heartbeat.tick() => {
                    if send(&link, to_radio::PayloadVariant::Heartbeat(Heartbeat::default())).is_err() {
                        return "link closed";
                    }
                }
                // In sync, read it all again, as on connecting but quietly:
                // its link stays as it is.
                _ = reread.tick(),
                    if self.read(|s| s.link == LinkState::Ready && !s.rereading) =>
                {
                    let again = self::nonce();
                    self.nonce.store(again, Ordering::Relaxed);
                    self.mark_sync_start();
                    self.update(|state| {
                        state.reread();
                        Vec::new()
                    });
                    if send(&link, to_radio::PayloadVariant::WantConfigId(again)).is_err() {
                        return "link closed";
                    }
                }
                // Not read again in time: left as it was known, the link
                // kept, as the radio still answers all else.
                () = tokio::time::sleep_until(self.sync_started() + SYNC_TIMEOUT),
                    if self.read(|s| s.rereading) =>
                {
                    tracing::debug!("the radio was not read again in time");
                    self.update(|state| {
                        state.reread_failed();
                        Vec::new()
                    });
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
pub(crate) fn now() -> u32 {
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
        let task = mesh.spawn(
            Protocol::Meshtastic,
            Transport::Tcp {
                host: "127.0.0.1".into(),
                port,
            },
        );
        let (stream, _) = listener.accept().await.unwrap();
        // Each frame out at once: with Nagle's, one written after another
        // waits for an acknowledgement that a paused clock outruns.
        stream.set_nodelay(true).unwrap();
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
        let Event::Packet(heard) = next_event(&mut events).await else {
            panic!("the packet, first");
        };
        assert_eq!(heard.from, 3);
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
            emoji: false,
            scope: None,
            sent_at: None,
        };
        assert_eq!(mesh.send_text(&text).await, Err(SendError::NotConnected));

        let Some(to_radio::PayloadVariant::WantConfigId(nonce)) =
            radio.receive().await.payload_variant
        else {
            panic!("the configuration first");
        };
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(nonce))
            .await;
        assert_eq!(next_event(&mut events).await, Event::Link(LinkState::Ready));

        let id = mesh.send_text(&text).await.unwrap().packet_id;
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
        assert_eq!(data.emoji, 0);

        // A reaction: the emoji, marked as one, to the packet it answers.
        let reaction = OutgoingText {
            text: "👍".into(),
            reply_id: Some(id),
            emoji: true,
            ..text.clone()
        };
        mesh.send_text(&reaction).await.unwrap();
        let Some(to_radio::PayloadVariant::Packet(packet)) = radio.receive().await.payload_variant
        else {
            panic!("a packet");
        };
        let Some(mesh_packet::PayloadVariant::Decoded(data)) = packet.payload_variant else {
            panic!("in the clear");
        };
        assert_eq!((data.emoji, data.reply_id), (1, id));
        let to_nothing = OutgoingText {
            reply_id: None,
            ..reaction
        };
        assert_eq!(mesh.send_text(&to_nothing).await, Err(SendError::BadText));

        let too_long = OutgoingText {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
            ..text
        };
        assert_eq!(mesh.send_text(&too_long).await, Err(SendError::BadText));
        task.abort();
    }

    #[tokio::test]
    async fn a_request_asks_for_an_answer_on_its_channel() {
        let (mesh, mut radio, task) = connected().await;
        let mut events = mesh.subscribe();
        let Some(to_radio::PayloadVariant::WantConfigId(nonce)) =
            radio.receive().await.payload_variant
        else {
            panic!("the configuration first");
        };
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(nonce))
            .await;
        assert_eq!(next_event(&mut events).await, Event::Link(LinkState::Ready));

        let id = mesh.send_request(0x5568_b843, 4, Ask::LocalStats).unwrap();
        let Some(to_radio::PayloadVariant::Packet(packet)) = radio.receive().await.payload_variant
        else {
            panic!("a packet");
        };
        assert_eq!((packet.id, packet.to, packet.channel), (id, 0x5568_b843, 4));
        let Some(mesh_packet::PayloadVariant::Decoded(data)) = packet.payload_variant else {
            panic!("in the clear");
        };
        assert!(data.want_response);
        assert_eq!(data.portnum, PortNum::TelemetryApp as i32);
        let asked = Telemetry::decode(data.payload.as_slice()).unwrap();
        assert!(matches!(
            asked.variant,
            Some(telemetry::Variant::LocalStats(_))
        ));
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
            fixed_position: Some(FixedPosition::Set(Position {
                latitude_i: Some(433_623_000),
                longitude_i: Some(-84_115_000),
                ..Default::default()
            })),
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

        // ...then the change, wrapped so that the radio applies it once, the
        // position last.
        let mut steps = Vec::new();
        for _ in 0..4 {
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
            &steps[2],
            admin_message::PayloadVariant::SetFixedPosition(p) if p.latitude_i == Some(433_623_000)
        ));
        assert!(matches!(
            steps[3],
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

    /// Connected, and in sync with a radio that names nodes 5 and 7.
    async fn in_sync() -> (Mesh, FakeRadio, JoinHandle<()>, broadcast::Receiver<Event>) {
        let (mesh, mut radio, task) = connected().await;
        let mut events = mesh.subscribe();
        let to_radio::PayloadVariant::WantConfigId(nonce) = next_request(&mut radio).await else {
            panic!("the configuration first");
        };
        for num in [5, 7] {
            radio
                .send(from_radio::PayloadVariant::NodeInfo(pb::NodeInfo {
                    num,
                    ..Default::default()
                }))
                .await;
        }
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(nonce))
            .await;
        assert_eq!(next_event(&mut events).await, Event::Link(LinkState::Ready));
        assert_eq!(next_event(&mut events).await, Event::Synced);
        (mesh, radio, task, events)
    }

    fn on_radio(mesh: &Mesh, num: u32) -> Option<bool> {
        mesh.read(|state| state.nodes[&num].on_radio)
    }

    #[tokio::test(start_paused = true)]
    async fn the_radio_is_read_again_quietly_every_so_often() {
        let (mesh, mut radio, task, mut events) = in_sync().await;
        assert_eq!([on_radio(&mesh, 5), on_radio(&mesh, 7)], [Some(true); 2]);

        tokio::time::advance(REREAD_INTERVAL).await;
        let to_radio::PayloadVariant::WantConfigId(again) = next_request(&mut radio).await else {
            panic!("the configuration again");
        };
        // 5 dropped from the radio's list since.
        radio
            .send(from_radio::PayloadVariant::NodeInfo(pb::NodeInfo {
                num: 7,
                ..Default::default()
            }))
            .await;
        radio
            .send(from_radio::PayloadVariant::ConfigCompleteId(again))
            .await;
        // Told at once at the end, the link never changed on the way.
        assert_eq!(next_event(&mut events).await, Event::Synced);
        assert_eq!(mesh.read(|s| s.link), LinkState::Ready);
        assert_eq!(
            [on_radio(&mesh, 5), on_radio(&mesh, 7)],
            [Some(false), Some(true)]
        );
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_reading_again_unanswered_leaves_all_as_it_was() {
        let (mesh, mut radio, task, _events) = in_sync().await;
        tokio::time::advance(REREAD_INTERVAL).await;
        let to_radio::PayloadVariant::WantConfigId(_) = next_request(&mut radio).await else {
            panic!("the configuration again");
        };
        // Only 7 named, and then nothing more.
        radio
            .send(from_radio::PayloadVariant::NodeInfo(pb::NodeInfo {
                num: 7,
                ..Default::default()
            }))
            .await;
        tokio::time::sleep(SYNC_TIMEOUT + Duration::from_secs(1)).await;
        assert!(!mesh.read(|s| s.rereading));
        assert_eq!(mesh.read(|s| s.link), LinkState::Ready);
        assert_eq!([on_radio(&mesh, 5), on_radio(&mesh, 7)], [Some(true); 2]);
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
