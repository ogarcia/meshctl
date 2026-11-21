// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios meshctl knows of. Each one has a mesh, started from what was
//! stored of it and kept stored as it changes; an enabled one also has a link
//! that keeps that mesh current. They are started, restarted and stopped as
//! radios are added, changed and removed.

use crate::store::{NewMessageRow, RadioRecord, Store, now};
use anyhow::Result;
use meshctl_api::{
    BADGE_COLORS, Badge, BadgeContent, ChannelSecret, Conversation, ErrorCode, Hop, LinkState,
    MapView, Message, MessageId, MessageStatus, NewMessage, NewRequest, NodeSettings,
    NodeSettingsChange, Packet, Peer, Preferences, Radio, RadioId, RadioKind, RadioSettings,
    Request, RequestKind, SectionSecret, SettingsApplied, SettingsSection, SightingId, Status,
    Traceroute,
};
use meshctl_core::link::Transport;
use meshctl_core::mesh::{self, MeshState};
use meshctl_core::radio::{AdminError, Ask, Mesh, OutgoingText, Protocol, SendError};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

/// The longest name a radio may have, in characters.
const NAME_MAX: usize = 64;

/// How many events a slow follower may fall behind before it misses some.
const EVENT_BACKLOG: usize = 4096;

/// What changed, across every radio.
#[derive(Debug, Clone)]
pub enum Change {
    /// A radio was added, changed or removed.
    Radios,
    /// Something changed on one radio: its link, or a node. Messages and
    /// their answers come as [`Change::Message`] and [`Change::MessageStatus`]
    /// instead, once kept.
    Mesh(RadioId, mesh::Event),
    /// A message heard or sent, as kept.
    Message(RadioId, Message),
    /// What became of a message sent.
    MessageStatus {
        radio: RadioId,
        id: MessageId,
        status: MessageStatus,
        error: Option<String>,
    },
    /// A traceroute sent, or what became of it.
    Traceroute(RadioId, Traceroute),
    /// A packet a radio heard, as kept.
    Packet(RadioId, Packet),
    /// Something a node was asked for, or what became of it.
    Request(RadioId, Request),
    /// The interface's preferences changed.
    Preferences(Preferences),
    /// Events of a radio were lost on their way here.
    Lagged,
}

/// Why a message was not sent.
#[derive(Debug)]
pub enum SendFailure {
    NoSuchRadio,
    NotConnected,
    Invalid,
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for SendFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(err)
    }
}

/// How long a radio waits between traceroutes, as the official clients do:
/// each one floods the mesh both ways.
pub const TRACEROUTE_INTERVAL: i64 = 30;

/// How long a traceroute is waited for before it is given up on.
const TRACEROUTE_TIMEOUT: i64 = 120;

/// How often traceroutes waited for too long are looked for.
const TRACEROUTE_EXPIRY_TICK: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a request is waited for before it is given up on.
const REQUEST_TIMEOUT: i64 = 120;

/// Why a request was not sent.
#[derive(Debug)]
pub enum AskFailure {
    NoSuchRadio,
    NotConnected,
    /// Not something meshctl does with this kind of radio yet.
    Unsupported,
    /// Everyone, or the radio itself.
    InvalidNode,
    /// The radio has no such channel, or it is disabled.
    InvalidChannel,
    /// The same was asked of the same node, and is still waiting.
    Pending,
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for AskFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(err)
    }
}

/// Why a radio's settings were not shown or changed.
#[derive(Debug)]
pub enum SettingsFailure {
    NoSuchRadio,
    /// Not connected, or not yet in sync: its settings are known only then.
    NotConnected,
    /// Which setting it is that the radio cannot have.
    Invalid(String),
    /// A channel the very same as another the radio has.
    Duplicate,
    /// A position more precise than a public channel takes.
    PublicPrecision,
    NoAnswer,
    /// Not something meshctl does with this kind of radio yet.
    Unsupported,
}

/// Why a traceroute was not sent.
#[derive(Debug)]
pub enum TraceFailure {
    NoSuchRadio,
    NotConnected,
    /// Not something meshctl does with this kind of radio yet.
    Unsupported,
    /// Everyone, or the radio itself.
    InvalidNode,
    /// The radio has no such channel, or it is disabled.
    InvalidChannel,
    /// The last one was this many seconds ago; wait the rest.
    TooSoon {
        wait: i64,
    },
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for TraceFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(err)
    }
}

/// The routes a traceroute answer tells, each from its first node to its last:
/// from the radio to the node traced, and back. Each hop with the SNR it was
/// heard at, which the firmware gives one for each hop, in order.
pub fn routes(origin: u32, answer: &mesh::TracerouteAnswer) -> (Vec<Hop>, Vec<Hop>) {
    let path = |first: u32, relays: &[u32], last: u32, snrs: &[Option<f32>]| -> Vec<Hop> {
        let first = Hop {
            node: Some(first),
            snr: None,
        };
        let rest = relays
            .iter()
            .copied()
            .chain([last])
            .enumerate()
            .map(|(i, node)| Hop {
                node: (node != mesh::UNKNOWN_HOP).then_some(node),
                snr: snrs.get(i).copied().flatten(),
            });
        std::iter::once(first).chain(rest).collect()
    };
    (
        path(origin, &answer.route, answer.from, &answer.snr_towards),
        path(answer.from, &answer.route_back, origin, &answer.snr_back),
    )
}

async fn answer_traceroute(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    answer: &mesh::TracerouteAnswer,
) -> Result<()> {
    let Some((id, origin, _)) = store.pending_traceroute(radio, answer.request_id).await? else {
        return Ok(());
    };
    let (towards, back) = routes(origin, answer);
    if let Some(traceroute) = store.settle_traceroute(id, Ok((&towards, &back))).await? {
        let _ = changes.send(Change::Traceroute(radio, traceroute));
    }
    Ok(())
}

async fn fail_traceroute(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    packet_id: u32,
    error: &str,
) -> Result<()> {
    let Some((id, _, _)) = store.pending_traceroute(radio, packet_id).await? else {
        return Ok(());
    };
    if let Some(traceroute) = store.settle_traceroute(id, Err(error)).await? {
        let _ = changes.send(Change::Traceroute(radio, traceroute));
    }
    Ok(())
}

/// Keeps a packet a radio heard, tells of it, and takes it as the answer to a
/// request when it is one.
async fn keep_packet(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    heard: &mesh::Heard,
) -> Result<()> {
    let Some(row) = store.add_packet(radio, heard).await? else {
        return Ok(());
    };
    let sighting = row.id;
    let _ = changes.send(Change::Packet(radio, crate::packets::packet(row)));
    if let Some(request_id) = heard.request_id
        && let Some(request) = store
            .answer_request(
                radio,
                request_id,
                heard.from,
                heard.portnum,
                sighting,
                i64::from(heard.at),
            )
            .await?
    {
        let _ = changes.send(Change::Request(radio, request));
    }
    Ok(())
}

/// Gives up on the traceroutes and requests waited for too long.
async fn expire(store: &Store, changes: &broadcast::Sender<Change>, radio: RadioId) -> Result<()> {
    for traceroute in store
        .expire_traceroutes(radio, now() - TRACEROUTE_TIMEOUT)
        .await?
    {
        let _ = changes.send(Change::Traceroute(radio, traceroute));
    }
    for request in store
        .expire_requests(radio, now() - REQUEST_TIMEOUT)
        .await?
    {
        let _ = changes.send(Change::Request(radio, request));
    }
    Ok(())
}

/// Whether a radio has a channel in that slot that it uses.
fn has_channel(mesh: &Mesh, channel: u32) -> bool {
    mesh.read(|state| {
        state
            .channels
            .get(&channel)
            .is_some_and(|c| c.role != "DISABLED")
    })
}

/// Checks what a client asks a radio to be, and says where it is.
pub fn validate(settings: &RadioSettings) -> Result<Transport, ErrorCode> {
    let name = settings.name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(ErrorCode::InvalidName);
    }
    // Each kind serves on a port of its own, taken when none is given.
    let port = match settings.kind {
        RadioKind::Meshtastic => meshctl_core::link::DEFAULT_TCP_PORT,
        RadioKind::Meshcore => meshctl_core::meshcore::DEFAULT_TCP_PORT,
    };
    let transport =
        Transport::parse(settings.address.trim(), port).map_err(|_| ErrorCode::InvalidAddress)?;
    if !valid_badge(&settings.badge) {
        return Err(ErrorCode::InvalidBadge);
    }
    Ok(transport)
}

/// The most a button's text takes, in bytes.
const TEXT_MAX: usize = 4;

/// The most an emoji takes, in characters: enough for those joined of
/// several, such as families or flags of regions.
const EMOJI_MAX: usize = 16;

/// Whether a radio's button can be shown: a colour of the palette, or one as
/// `#rrggbb`; up to four bytes of text, as a node's short name; or an emoji,
/// which has no letters, digits or spaces.
fn valid_badge(badge: &Badge) -> bool {
    let color = badge.color.as_deref().is_none_or(|color| {
        BADGE_COLORS.contains(&color)
            || (color.len() == 7
                && color.starts_with('#')
                && color[1..].chars().all(|c| c.is_ascii_hexdigit()))
    });
    let shown = |text: &str| text.chars().all(|c| !c.is_whitespace() && !c.is_control());
    let content = match &badge.content {
        BadgeContent::Initials | BadgeContent::Icon { .. } => true,
        BadgeContent::Text { text } => {
            let text = text.trim();
            // As Meshtastic's short names: four bytes, which is four letters,
            // fewer with accents, or one emoji.
            (1..=TEXT_MAX).contains(&text.len()) && shown(text)
        }
        BadgeContent::Emoji { emoji } => {
            let emoji = emoji.trim();
            (1..=EMOJI_MAX).contains(&emoji.chars().count())
                && shown(emoji)
                && !emoji.chars().any(|c| c.is_alphanumeric())
        }
    };
    color && content
}

/// Settings as they are kept: trimmed.
pub fn tidy(settings: RadioSettings) -> RadioSettings {
    RadioSettings {
        name: settings.name.trim().to_owned(),
        address: settings.address.trim().to_owned(),
        badge: Badge {
            color: settings.badge.color.map(|color| color.to_ascii_lowercase()),
            content: match settings.badge.content {
                BadgeContent::Text { text } => BadgeContent::Text {
                    text: text.trim().to_owned(),
                },
                BadgeContent::Emoji { emoji } => BadgeContent::Emoji {
                    emoji: emoji.trim().to_owned(),
                },
                content => content,
            },
        },
        ..settings
    }
}

/// A radio's mesh, and the tasks that keep it: its link when it is enabled,
/// and always one that passes its events on and one that stores them.
struct Running {
    mesh: Mesh,
    protocol: Protocol,
    enabled: bool,
    tasks: Vec<JoinHandle<()>>,
}

/// The driver a kind of radio takes.
fn protocol_of(kind: RadioKind) -> Protocol {
    match kind {
        RadioKind::Meshtastic => Protocol::Meshtastic,
        RadioKind::Meshcore => Protocol::Meshcore,
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // The link closes with its task.
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[derive(Clone)]
pub struct Radios {
    store: Store,
    running: Arc<Mutex<HashMap<RadioId, Running>>>,
    changes: broadcast::Sender<Change>,
    stopped: watch::Sender<bool>,
}

impl Radios {
    /// Starts every radio in the store, and connects to the enabled ones.
    pub async fn start(store: Store) -> Result<Self> {
        let radios = Self {
            store,
            running: Arc::default(),
            changes: broadcast::channel(EVENT_BACKLOG).0,
            stopped: watch::channel(false).0,
        };
        for record in radios.store.radios().await? {
            let state = radios.stored_state(record.id).await?;
            radios.run(&record, state);
        }
        Ok(radios)
    }

    /// Everything that changes from now on, on every radio.
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// What a radio knows, live if it is connected and as last stored if not.
    pub fn mesh(&self, id: RadioId) -> Option<Mesh> {
        self.lock().get(&id).map(|running| running.mesh.clone())
    }

    /// Which mesh a radio speaks, if there is such a radio.
    fn protocol(&self, id: RadioId) -> Option<Protocol> {
        self.lock().get(&id).map(|running| running.protocol)
    }

    /// How a radio's link is doing; [`LinkState::Disabled`] for one that is
    /// not meant to be connected, or not running at all.
    pub fn link(&self, id: RadioId) -> LinkState {
        match self.lock().get(&id) {
            Some(running) if running.enabled => {
                crate::web::convert::link(running.mesh.read(|state| state.link))
            }
            _ => LinkState::Disabled,
        }
    }

    /// A radio's link and what it said of itself, if there is such a radio.
    pub fn status(&self, id: RadioId) -> Option<Status> {
        let local = self
            .mesh(id)?
            .read(|state| state.local.as_ref().map(crate::web::convert::local));
        Some(Status {
            link: self.link(id),
            local,
        })
    }

    /// Sends a message through a radio, and keeps it as sent.
    pub async fn send(&self, id: RadioId, message: NewMessage) -> Result<Message, SendFailure> {
        let mesh = self.mesh(id).ok_or(SendFailure::NoSuchRadio)?;
        let (to, channel) = match (message.channel, message.node) {
            (Some(channel), None) => {
                let usable = mesh.read(|state| {
                    state
                        .channels
                        .get(&channel)
                        .is_some_and(|c| c.role != "DISABLED")
                });
                if !usable {
                    return Err(SendFailure::Invalid);
                }
                (mesh::BROADCAST, channel)
            }
            // A direct message goes on the primary channel; with the
            // recipient's key, when the firmware has it, it is private anyway.
            (None, Some(node)) if node != mesh::BROADCAST => (node, 0),
            _ => return Err(SendFailure::Invalid),
        };
        let outgoing = OutgoingText {
            to,
            channel,
            text: message.text.clone(),
            reply_id: message.reply_id,
        };
        if self.link(id) != LinkState::Ready {
            return Err(SendFailure::NotConnected);
        }
        let sent = mesh
            .send_text(&outgoing)
            .await
            .map_err(|error| match error {
                SendError::NotConnected => SendFailure::NotConnected,
                SendError::BadText | SendError::Refused | SendError::Unsupported => {
                    SendFailure::Invalid
                }
            })?;
        let from = mesh.read(|state| state.local.as_ref().map_or(0, |l| l.num));
        let row = NewMessageRow {
            packet_id: sent.packet_id,
            from,
            sender_name: None,
            to,
            channel,
            text: message.text,
            at: now(),
            outgoing: true,
            // What cannot be acknowledged is as sent as it will ever be.
            status: if sent.acknowledged {
                MessageStatus::Sending
            } else {
                MessageStatus::Sent
            },
            reply_id: message.reply_id,
            emoji: false,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
        };
        let kept = self
            .store
            .add_message(id, &row)
            .await?
            .ok_or_else(|| anyhow::anyhow!("a packet id was used twice"))?;
        let _ = self.changes.send(Change::Message(id, kept.clone()));
        Ok(kept)
    }

    /// Every conversation a radio has: its channels, whether or not anything
    /// was said in them, and every node it exchanged messages with, the most
    /// recent first.
    pub async fn conversations(&self, id: RadioId) -> Result<Option<Vec<Conversation>>> {
        let Some(mesh) = self.mesh(id) else {
            return Ok(None);
        };
        let kept = self.store.conversations(id).await?;
        let channels: Vec<u32> = mesh.read(|state| {
            state
                .channels
                .values()
                .filter(|c| c.role != "DISABLED")
                .map(|c| c.index)
                .collect()
        });
        let mut conversations: Vec<Conversation> = channels
            .into_iter()
            .map(|channel| {
                let peer = Peer::Channel { channel };
                let last = kept
                    .iter()
                    .find(|c| c.peer == peer)
                    .and_then(|c| c.last.clone());
                Conversation { peer, last }
            })
            .collect();
        // Channels no longer on the radio keep what was said in them.
        let others: Vec<Conversation> = kept
            .into_iter()
            .filter(|c| !conversations.iter().any(|known| known.peer == c.peer))
            .collect();
        conversations.extend(others);
        Ok(Some(conversations))
    }

    /// The messages exchanged with a peer through a radio, if there is such a
    /// radio.
    pub async fn messages(
        &self,
        id: RadioId,
        peer: Peer,
        before: Option<MessageId>,
        limit: u32,
    ) -> Result<Option<Vec<Message>>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        Ok(Some(self.store.messages(id, peer, before, limit).await?))
    }

    /// Traces the route from a radio to a node.
    pub async fn traceroute(
        &self,
        id: RadioId,
        node: u32,
        channel: u32,
    ) -> Result<Traceroute, TraceFailure> {
        let mesh = self.mesh(id).ok_or(TraceFailure::NoSuchRadio)?;
        let origin = mesh.read(|state| state.local.as_ref().map(|l| l.num));
        if node == mesh::BROADCAST || origin == Some(node) {
            return Err(TraceFailure::InvalidNode);
        }
        if !has_channel(&mesh, channel) {
            return Err(TraceFailure::InvalidChannel);
        }
        if self.protocol(id) != Some(Protocol::Meshtastic) {
            return Err(TraceFailure::Unsupported);
        }
        if self.link(id) != LinkState::Ready {
            return Err(TraceFailure::NotConnected);
        }
        let at = now();
        if let Some(last) = self.store.last_traceroute_at(id).await? {
            let since = at - last;
            if since < TRACEROUTE_INTERVAL {
                return Err(TraceFailure::TooSoon {
                    wait: TRACEROUTE_INTERVAL - since,
                });
            }
        }
        let packet_id = mesh
            .send_traceroute(node, channel)
            .map_err(|error| match error {
                SendError::Unsupported => TraceFailure::Unsupported,
                SendError::NotConnected | SendError::BadText | SendError::Refused => {
                    TraceFailure::NotConnected
                }
            })?;
        let traceroute = self
            .store
            .add_traceroute(id, packet_id, origin.unwrap_or_default(), node, at)
            .await?;
        let _ = self
            .changes
            .send(Change::Traceroute(id, traceroute.clone()));
        Ok(traceroute)
    }

    /// Asks a node for something it tells on request; the answer, or the lack
    /// of one, comes on the event stream as the request changes.
    pub async fn request(&self, id: RadioId, new: NewRequest) -> Result<Request, AskFailure> {
        let mesh = self.mesh(id).ok_or(AskFailure::NoSuchRadio)?;
        let origin = mesh.read(|state| state.local.as_ref().map(|l| l.num));
        if new.node == mesh::BROADCAST || origin == Some(new.node) {
            return Err(AskFailure::InvalidNode);
        }
        if self.protocol(id) != Some(Protocol::Meshtastic) {
            return Err(AskFailure::Unsupported);
        }
        if self.link(id) != LinkState::Ready {
            return Err(AskFailure::NotConnected);
        }
        if !has_channel(&mesh, new.channel) {
            return Err(AskFailure::InvalidChannel);
        }
        if self.store.is_asking(id, new.node, new.kind).await? {
            return Err(AskFailure::Pending);
        }
        let ask = match new.kind {
            RequestKind::NodeInfo => Ask::NodeInfo,
            RequestKind::Position => Ask::Position,
            RequestKind::DeviceTelemetry => Ask::DeviceTelemetry,
            RequestKind::EnvironmentTelemetry => Ask::EnvironmentTelemetry,
            RequestKind::PowerTelemetry => Ask::PowerTelemetry,
            RequestKind::LocalStats => Ask::LocalStats,
            RequestKind::Neighbors => Ask::Neighbors,
        };
        let packet_id =
            mesh.send_request(new.node, new.channel, ask)
                .map_err(|error| match error {
                    SendError::Unsupported => AskFailure::Unsupported,
                    SendError::NotConnected | SendError::BadText | SendError::Refused => {
                        AskFailure::NotConnected
                    }
                })?;
        let request = self
            .store
            .add_request(id, new.node, new.kind, new.channel, packet_id, now())
            .await?;
        let _ = self.changes.send(Change::Request(id, request.clone()));
        Ok(request)
    }

    /// The requests sent through a radio, the newest first, or `None` if
    /// there is no such radio.
    pub async fn requests(
        &self,
        id: RadioId,
        node: Option<u32>,
        limit: u32,
    ) -> Result<Option<Vec<Request>>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        Ok(Some(self.store.requests(id, node, limit).await?))
    }

    /// A radio's own settings, as it sent them on syncing. Only while it is
    /// connected: they are not kept, and a radio that is not there cannot be
    /// changed anyway.
    pub fn settings(&self, id: RadioId) -> Result<NodeSettings, SettingsFailure> {
        self.configurable(id)?
            .read(|state| crate::settings::view(&state.settings))
            .ok_or(SettingsFailure::NotConnected)
    }

    /// A channel's key, for the one who asks for it, and only then: nothing
    /// else hands it out. Each time is logged.
    pub fn channel_secret(
        &self,
        id: RadioId,
        index: u32,
    ) -> Result<Option<ChannelSecret>, SettingsFailure> {
        let (sent, secret) = self.configurable(id)?.read(|state| {
            (
                crate::settings::view(&state.settings).is_some(),
                crate::settings::secret(&state.settings, index),
            )
        });
        // As for its settings: not before the radio has sent them.
        if !sent {
            return Err(SettingsFailure::NotConnected);
        }
        if secret.is_some() {
            tracing::info!(radio = id, channel = index, "channel key handed out");
        }
        Ok(secret)
    }

    /// Puts a radio's channels in `order`, by the slots they have now: the
    /// first becomes the primary, the rest follow it with no gap, and any
    /// left out is taken off the radio. Each slot is given its conversation at
    /// once, not only once the radio is read back; that of one left out
    /// waits for it, in no slot, unless `forget`.
    async fn rearrange_channels(
        &self,
        id: RadioId,
        order: &[u32],
        forget: bool,
    ) -> Result<SettingsApplied, SettingsFailure> {
        use crate::channels::{rearranged, slots};
        let mesh = self.configurable(id)?;
        let (sent, before, after) = mesh
            .read(|state| {
                let sent = rearranged(&state.settings, order)?;
                let mut settings = state.settings.clone();
                for channel in &sent {
                    settings
                        .channels
                        .insert(channel.index as u32, channel.clone());
                }
                Ok((sent, slots(&state.settings), slots(&settings)))
            })
            .map_err(|why: crate::settings::Invalid| SettingsFailure::Invalid(why.0.to_owned()))?;
        if !sent.is_empty() {
            mesh.change_settings(meshctl_core::radio::SettingsChange {
                channels: sent,
                ..Default::default()
            })
            .await
            .map_err(admin_failure)?;
        }
        // Should the radio not take it, reading it back puts each
        // conversation back where it was.
        let followed = async {
            // Before the slots are taken anew: it goes by the one it has.
            if forget {
                for gone in before.iter().filter(|b| !order.contains(&b.index)) {
                    self.store.forget_channel(id, gone.index).await?;
                }
            }
            self.store.follow_channels(id, &after, now()).await?;
            anyhow::Ok(())
        };
        if let Err(err) = followed.await {
            tracing::warn!(radio = id, error = format_args!("{err:#}"), "cannot store");
        }
        tracing::info!(radio = id, ?order, forget, "channels rearranged");
        Ok(SettingsApplied { reboots: false })
    }

    /// Puts a radio's channels in another order: every one it has, by its
    /// slot now.
    pub async fn reorder_channels(
        &self,
        id: RadioId,
        order: &[u32],
    ) -> Result<SettingsApplied, SettingsFailure> {
        let have = self
            .configurable(id)?
            .read(|state| crate::channels::slots(&state.settings).len());
        if order.len() != have {
            return Err(SettingsFailure::Invalid("order".to_owned()));
        }
        self.rearrange_channels(id, order, false).await
    }

    /// Takes a channel off a radio, but its primary: those after it move up
    /// a slot. Its conversation waits for it, in no slot, unless `forget`.
    pub async fn remove_channel(
        &self,
        id: RadioId,
        index: u32,
        forget: bool,
    ) -> Result<SettingsApplied, SettingsFailure> {
        let have: Vec<u32> = self
            .configurable(id)?
            .read(|state| crate::channels::slots(&state.settings))
            .iter()
            .map(|s| s.index)
            .collect();
        if index == 0 || !have.contains(&index) {
            return Err(SettingsFailure::Invalid("channel".to_owned()));
        }
        let order: Vec<u32> = have.into_iter().filter(|&i| i != index).collect();
        self.rearrange_channels(id, &order, forget).await
    }

    /// A secret of one of a radio's sections, for the one who asks for it, and
    /// only then. Each time is logged.
    pub fn section_secret(
        &self,
        id: RadioId,
        section: &str,
        field: &str,
    ) -> Result<Option<SectionSecret>, SettingsFailure> {
        let value = self
            .configurable(id)?
            .read(|state| crate::sections::secret(&state.settings, section, field));
        if value.is_some() {
            tracing::info!(radio = id, section, field, "setting's secret handed out");
        }
        Ok(value.map(|value| SectionSecret {
            section: section.to_owned(),
            field: field.to_owned(),
            value,
        }))
    }

    /// A radio whose settings can be read and changed now: there is such a
    /// radio, meshctl knows its settings, and it is connected.
    fn configurable(&self, id: RadioId) -> Result<Mesh, SettingsFailure> {
        let mesh = self.mesh(id).ok_or(SettingsFailure::NoSuchRadio)?;
        if self.protocol(id) != Some(Protocol::Meshtastic) {
            return Err(SettingsFailure::Unsupported);
        }
        if self.link(id) != LinkState::Ready {
            return Err(SettingsFailure::NotConnected);
        }
        Ok(mesh)
    }

    /// Changes a radio's own settings. Returns whether it reboots to apply
    /// them.
    pub async fn change_settings(
        &self,
        id: RadioId,
        wanted: &NodeSettingsChange,
    ) -> Result<SettingsApplied, SettingsFailure> {
        let mesh = self.configurable(id)?;
        let (change, reboots) = mesh
            .read(|state| {
                crate::settings::change(&state.settings, wanted, |key| {
                    getrandom::fill(key).expect("the system has randomness to give");
                })
            })
            .map_err(|why| match why.0 {
                crate::settings::DUPLICATE => SettingsFailure::Duplicate,
                crate::settings::PUBLIC_PRECISION => SettingsFailure::PublicPrecision,
                why => SettingsFailure::Invalid(why.to_owned()),
            })?;
        mesh.change_settings(change).await.map_err(admin_failure)?;
        tracing::info!(radio = id, reboots, "settings changed");
        Ok(SettingsApplied { reboots })
    }

    /// The rest of a radio's configuration and its modules, section by
    /// section; like its settings, only while it is connected.
    pub fn sections(&self, id: RadioId) -> Result<Vec<SettingsSection>, SettingsFailure> {
        Ok(self
            .configurable(id)?
            .read(|state| crate::sections::sections(&state.settings)))
    }

    /// Changes some settings of one section. The firmware reboots for nearly
    /// any of them, so it is always said that it does.
    pub async fn change_section(
        &self,
        id: RadioId,
        section: &str,
        values: &BTreeMap<String, serde_json::Value>,
    ) -> Result<SettingsApplied, SettingsFailure> {
        let mesh = self.configurable(id)?;
        if values.is_empty() {
            return Err(SettingsFailure::Invalid(section.to_owned()));
        }
        let change = mesh
            .read(|state| crate::sections::change(&state.settings, section, values))
            .map_err(|why| SettingsFailure::Invalid(why.0))?;
        mesh.change_settings(change).await.map_err(admin_failure)?;
        tracing::info!(radio = id, section, "settings changed");
        Ok(SettingsApplied { reboots: true })
    }

    /// A radio's traceroutes, to one node or to any, the newest first, if there
    /// is such a radio.
    pub async fn traceroutes(
        &self,
        id: RadioId,
        node: Option<u32>,
        limit: u32,
    ) -> Result<Option<Vec<Traceroute>>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        Ok(Some(self.store.traceroutes(id, node, limit).await?))
    }

    /// The packets a radio heard, the newest first, or `None` if there is no
    /// such radio.
    pub async fn packets(
        &self,
        id: RadioId,
        before: Option<SightingId>,
        node: Option<u32>,
        limit: u32,
    ) -> Result<Option<Vec<Packet>>> {
        if self.mesh(id).is_none() {
            return Ok(None);
        }
        let rows = self.store.packets(id, before, node, limit).await?;
        Ok(Some(rows.into_iter().map(crate::packets::packet).collect()))
    }

    pub async fn list(&self) -> Result<Vec<Radio>> {
        let records = self.store.radios().await?;
        Ok(records
            .into_iter()
            .map(|record| self.radio(record))
            .collect())
    }

    pub async fn get(&self, id: RadioId) -> Result<Option<Radio>> {
        Ok(self.store.radio(id).await?.map(|record| self.radio(record)))
    }

    /// Adds a radio, already validated, and connects to it if it is enabled.
    pub async fn add(&self, settings: RadioSettings) -> Result<Radio> {
        let record = self.store.add_radio(&settings).await?;
        self.run(&record, MeshState::default());
        self.changed();
        Ok(self.radio(record))
    }

    /// Changes a radio, already validated, reconnecting to it as it is now
    /// when what it connects to, or whether it does, changed.
    pub async fn update(&self, id: RadioId, settings: RadioSettings) -> Result<Option<Radio>> {
        let before = self.store.radio(id).await?;
        if !self.store.update_radio(id, &settings).await? {
            return Ok(None);
        }
        let Some(record) = self.store.radio(id).await? else {
            return Ok(None);
        };
        // How it shows alone changes nothing of its link.
        let link = |settings: &RadioSettings| {
            (
                settings.name.clone(),
                settings.kind,
                settings.address.clone(),
                settings.enabled,
            )
        };
        if before.is_some_and(|before| link(&before.settings) == link(&record.settings))
            && self.lock().contains_key(&id)
        {
            self.changed();
            return Ok(Some(self.radio(record)));
        }
        // What it knew carries over, more current than what was stored.
        let previous = self.lock().remove(&id);
        let state = match previous {
            Some(running) => running.mesh.read(MeshState::clone),
            None => self.stored_state(id).await?,
        };
        // Its link goes with the old settings: it was connected until now.
        let record = if state.link == mesh::LinkState::Ready {
            let at = now();
            self.store.set_last_connected(id, at).await?;
            RadioRecord {
                last_connected: Some(at),
                ..record
            }
        } else {
            record
        };
        self.run(&record, state);
        self.changed();
        Ok(Some(self.radio(record)))
    }

    /// Puts the radios in the order given; `false`, changing nothing, unless
    /// it names each radio there is once.
    pub async fn reorder(&self, radios: &[RadioId]) -> Result<bool> {
        let mut given = radios.to_vec();
        given.sort_unstable();
        let mut there: Vec<RadioId> = self.store.radios().await?.iter().map(|r| r.id).collect();
        there.sort_unstable();
        if given != there {
            return Ok(false);
        }
        self.store.set_radio_order(radios).await?;
        self.changed();
        Ok(true)
    }

    /// Where the map was left for a radio: `None` if there is no such radio.
    /// Nobody is told when it moves: each browser goes on where it is.
    pub async fn map_view(&self, id: RadioId) -> Result<Option<Option<MapView>>> {
        self.store.map_view(id).await
    }

    /// Keeps where the map was left for a radio; `false` if there is none.
    pub async fn set_map_view(&self, id: RadioId, view: &MapView) -> Result<bool> {
        self.store.set_map_view(id, view).await
    }

    /// Removes a radio and disconnects from it; `false` if there was none.
    pub async fn remove(&self, id: RadioId) -> Result<bool> {
        if !self.store.remove_radio(id).await? {
            return Ok(false);
        }
        self.lock().remove(&id);
        self.changed();
        Ok(true)
    }

    /// Disconnects from every radio, for good: the daemon is stopping. Those
    /// that were connected until now are stored as such.
    pub async fn stop(&self) {
        self.stopped.send_replace(true);
        let connected: Vec<RadioId> = self
            .lock()
            .iter()
            .filter(|(_, running)| running.mesh.read(|s| s.link) == mesh::LinkState::Ready)
            .map(|(id, _)| *id)
            .collect();
        self.lock().clear();
        let at = now();
        for id in connected {
            if let Err(err) = self.store.set_last_connected(id, at).await {
                tracing::warn!(radio = id, error = format_args!("{err:#}"), "cannot store");
            }
        }
    }

    /// The interface's preferences.
    pub async fn preferences(&self) -> Result<Preferences> {
        self.store.preferences().await
    }

    /// Keeps the interface's preferences, and tells every client.
    pub async fn set_preferences(&self, preferences: Preferences) -> Result<()> {
        self.store.set_preferences(&preferences).await?;
        let _ = self.changes.send(Change::Preferences(preferences));
        Ok(())
    }

    /// Resolves once [`Self::stop`] has been called, for whoever follows the
    /// changes to stop following them.
    pub async fn stopped(&self) {
        let mut stopped = self.stopped.subscribe();
        let _ = stopped.wait_for(|stopped| *stopped).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RadioId, Running>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self) {
        let _ = self.changes.send(Change::Radios);
    }

    fn radio(&self, record: RadioRecord) -> Radio {
        Radio {
            id: record.id,
            link: self.link(record.id),
            name: record.settings.name,
            kind: record.settings.kind,
            address: record.settings.address,
            enabled: record.settings.enabled,
            shown: record.settings.shown,
            short_name: (record.settings.kind == RadioKind::Meshtastic)
                .then(|| self.short_name(record.id))
                .flatten(),
            badge: record.settings.badge,
            last_connected: record.last_connected,
        }
    }

    /// The short name of a radio's own node, as it last said it.
    fn short_name(&self, id: RadioId) -> Option<String> {
        let lock = self.lock();
        let running = lock.get(&id)?;
        running.mesh.read(|state| {
            let local = state.local.as_ref()?.num;
            let short = &state.nodes.get(&local)?.user.as_ref()?.short_name;
            (!short.trim().is_empty()).then(|| short.clone())
        })
    }

    /// What was stored of a radio, to start its mesh from.
    async fn stored_state(&self, id: RadioId) -> Result<MeshState> {
        let local = self.store.local(id).await?;
        let nodes = self.store.nodes(id).await?;
        Ok(MeshState {
            local,
            nodes: nodes.into_iter().map(|node| (node.num, node)).collect(),
            ..MeshState::default()
        })
    }

    /// Starts a radio's mesh from `state`, and connects to it if it is
    /// enabled and can be.
    fn run(&self, record: &RadioRecord, state: MeshState) {
        let id = record.id;
        let mesh = Mesh::new(MeshState {
            link: mesh::LinkState::Disconnected,
            ..state
        });
        let mut tasks = vec![
            self.forward(id, &mesh),
            self.persist(id, &mesh, protocol_of(record.settings.kind)),
        ];
        let mut enabled = record.settings.enabled;
        if enabled {
            match validate(&record.settings) {
                Ok(transport) => {
                    tracing::info!(radio = id, name = %record.settings.name, %transport, "connecting");
                    tasks.push(mesh.spawn(protocol_of(record.settings.kind), transport));
                }
                Err(error) => {
                    tracing::warn!(radio = id, ?error, "cannot connect to this radio");
                    enabled = false;
                }
            }
        }
        self.lock().insert(
            id,
            Running {
                mesh,
                protocol: protocol_of(record.settings.kind),
                enabled,
                tasks,
            },
        );
    }

    /// Passes a radio's events on, saying whose they are.
    fn forward(&self, id: RadioId, mesh: &Mesh) -> JoinHandle<()> {
        let mut events = mesh.subscribe();
        let changes = self.changes.clone();
        tokio::spawn(async move {
            loop {
                let change = match events.recv().await {
                    // These go out once kept, from `persist`.
                    Ok(
                        mesh::Event::Message(_)
                        | mesh::Event::Packet(_)
                        | mesh::Event::Ack(_)
                        | mesh::Event::Traceroute(_)
                        | mesh::Event::AdminAnswer { .. },
                    ) => continue,
                    Ok(event) => Change::Mesh(id, event),
                    Err(RecvError::Lagged(_)) => Change::Lagged,
                    Err(RecvError::Closed) => return,
                };
                let _ = changes.send(change);
            }
        })
    }

    /// Stores what a radio learns as it learns it: each node as it changes,
    /// everything at once when the radio syncs, and when it was last connected
    /// as it drops.
    fn persist(&self, id: RadioId, mesh: &Mesh, protocol: Protocol) -> JoinHandle<()> {
        let mut events = mesh.subscribe();
        let mesh = mesh.clone();
        let store = self.store.clone();
        let changes = self.changes.clone();
        tokio::spawn(async move {
            let mut connected = false;
            let mut expiry = tokio::time::interval(TRACEROUTE_EXPIRY_TICK);
            loop {
                let event = tokio::select! {
                    event = events.recv() => event,
                    _ = expiry.tick() => {
                        if let Err(err) = expire(&store, &changes, id).await {
                            tracing::warn!(
                                radio = id,
                                error = format_args!("{err:#}"),
                                "cannot store"
                            );
                        }
                        continue;
                    }
                };
                let stored = match event {
                    Ok(mesh::Event::Node(node)) => store.save_node(id, &node).await,
                    Ok(mesh::Event::Packet(heard)) => {
                        keep_packet(&store, &changes, id, &heard).await
                    }
                    Ok(mesh::Event::Traceroute(answer)) => {
                        answer_traceroute(&store, &changes, id, &answer).await
                    }
                    Ok(mesh::Event::Message(heard)) => {
                        let local = mesh.read(|s| s.local.as_ref().map(|l| l.num));
                        let kept = store.add_message(id, &heard_row(heard, local)).await;
                        kept.map(|kept| {
                            if let Some(message) = kept {
                                let _ = changes.send(Change::Message(id, message));
                            }
                        })
                    }
                    Ok(mesh::Event::Ack(ack)) => {
                        // A relay giving up on a traceroute or a request, or a
                        // node with nothing to tell, says so this way.
                        if let Some(error) = &ack.error {
                            let failed =
                                match fail_traceroute(&store, &changes, id, ack.packet_id, error)
                                    .await
                                {
                                    Ok(()) => store.fail_request(id, ack.packet_id, error).await,
                                    Err(err) => Err(err),
                                };
                            match failed {
                                Ok(Some(request)) => {
                                    let _ = changes.send(Change::Request(id, request));
                                }
                                Ok(None) => {}
                                Err(err) => tracing::warn!(
                                    radio = id,
                                    error = format_args!("{err:#}"),
                                    "cannot store"
                                ),
                            }
                        }
                        let answered = store
                            .ack(id, ack.packet_id, ack.from, ack.error.as_deref())
                            .await;
                        answered.map(|answered| {
                            if let Some((message, status, error)) = answered {
                                let _ = changes.send(Change::MessageStatus {
                                    radio: id,
                                    id: message,
                                    status,
                                    error,
                                });
                            }
                        })
                    }
                    Ok(mesh::Event::Synced) => {
                        connected = true;
                        match save_all(&store, id, &mesh).await {
                            Ok(()) if protocol == Protocol::Meshtastic => {
                                follow_channels(&store, &changes, id, &mesh).await
                            }
                            other => other,
                        }
                    }
                    // Something was missed: store it all again, if it is current.
                    Err(RecvError::Lagged(_)) if connected => save_all(&store, id, &mesh).await,
                    Ok(mesh::Event::Link(link)) if connected && link != mesh::LinkState::Ready => {
                        connected = false;
                        store.set_last_connected(id, now()).await
                    }
                    Err(RecvError::Closed) => return,
                    _ => Ok(()),
                };
                if let Err(err) = stored {
                    tracing::warn!(radio = id, error = format_args!("{err:#}"), "cannot store");
                }
            }
        })
    }

    /// The store, for the tests to put things in it.
    #[cfg(test)]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Puts a mesh in place of a radio's link, for the tests.
    #[cfg(test)]
    pub fn insert_mesh(&self, id: RadioId, mesh: Mesh) {
        let forward = self.forward(id, &mesh);
        self.lock().insert(
            id,
            Running {
                mesh,
                protocol: Protocol::Meshtastic,
                enabled: true,
                tasks: vec![forward],
            },
        );
    }
}

/// What a radio refusing a change of settings means for the API.
fn admin_failure(error: AdminError) -> SettingsFailure {
    match error {
        AdminError::NotConnected => SettingsFailure::NotConnected,
        AdminError::NoAnswer => SettingsFailure::NoAnswer,
        AdminError::Unsupported => SettingsFailure::Unsupported,
    }
}

/// A message heard, as kept.
///
/// One from the radio itself was sent through it by another client (a phone,
/// or another one behind the same tasticsplitter): it is ours as much as one
/// sent from meshctl, and as far as anyone here can tell, it went out.
fn heard_row(heard: mesh::TextMessage, local: Option<u32>) -> NewMessageRow {
    let ours = local == Some(heard.from);
    NewMessageRow {
        packet_id: heard.packet_id,
        from: heard.from,
        sender_name: heard.sender_name,
        to: heard.to,
        channel: heard.channel,
        text: heard.text,
        at: i64::from(heard.at),
        outgoing: ours,
        status: if ours {
            MessageStatus::Sent
        } else {
            MessageStatus::Received
        },
        reply_id: heard.reply_id,
        emoji: heard.emoji,
        snr: heard.snr,
        rssi: heard.rssi,
        hops_away: heard.hops_away,
        via_mqtt: heard.via_mqtt,
    }
}

/// Takes the channels a radio was just read with: each slot's conversation
/// is its channel's. If any slot has another, the radio is said to be in
/// sync again, for whoever shows it to read it anew.
async fn follow_channels(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    id: RadioId,
    mesh: &Mesh,
) -> Result<()> {
    let now = mesh.read(|state| crate::channels::slots(&state.settings));
    // A radio always has its primary: none read is nothing read, not every
    // channel gone.
    if now.is_empty() {
        return Ok(());
    }
    if store.follow_channels(id, &now, crate::store::now()).await? {
        tracing::info!(radio = id, "channels moved");
        let _ = changes.send(Change::Mesh(id, mesh::Event::Synced));
    }
    Ok(())
}

async fn save_all(store: &Store, id: RadioId, mesh: &Mesh) -> Result<()> {
    let (local, nodes) = mesh.read(|state| {
        (
            state.local.clone(),
            state.nodes.values().cloned().collect::<Vec<_>>(),
        )
    });
    store.save_sync(id, local.as_ref(), &nodes, now()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(name: &str, kind: RadioKind, address: &str) -> RadioSettings {
        RadioSettings {
            name: name.into(),
            kind,
            address: address.into(),
            enabled: true,
            shown: true,
            badge: Default::default(),
        }
    }

    #[test]
    fn an_answer_reads_as_two_routes_from_end_to_end() {
        let answer = mesh::TracerouteAnswer {
            request_id: 1,
            from: 9,
            route: vec![5, mesh::UNKNOWN_HOP],
            snr_towards: vec![Some(6.0), None, Some(2.5)],
            route_back: vec![5],
            snr_back: vec![Some(-1.5), Some(10.0)],
        };
        let (towards, back) = routes(7, &answer);
        let hop = |node, snr| Hop { node, snr };
        assert_eq!(
            towards,
            [
                hop(Some(7), None),
                hop(Some(5), Some(6.0)),
                hop(None, None),
                hop(Some(9), Some(2.5)),
            ]
        );
        assert_eq!(
            back,
            [
                hop(Some(9), None),
                hop(Some(5), Some(-1.5)),
                hop(Some(7), Some(10.0))
            ]
        );
    }

    #[test]
    fn a_direct_answer_is_one_hop_each_way() {
        let answer = mesh::TracerouteAnswer {
            request_id: 1,
            from: 9,
            route: vec![],
            snr_towards: vec![Some(8.0)],
            route_back: vec![],
            snr_back: vec![Some(7.0)],
        };
        let (towards, back) = routes(7, &answer);
        assert_eq!(towards.len(), 2);
        assert_eq!(back[1].snr, Some(7.0));
    }

    #[test]
    fn what_another_client_sent_through_the_radio_is_ours() {
        let heard = mesh::TextMessage {
            packet_id: 1,
            from: 7,
            to: 9,
            channel: 0,
            text: "hola".into(),
            at: 1,
            reply_id: None,
            sender_name: None,
            emoji: false,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
        };
        let ours = heard_row(heard.clone(), Some(7));
        assert!(ours.outgoing);
        assert_eq!(ours.status, MessageStatus::Sent);
        let theirs = heard_row(heard, Some(9));
        assert!(!theirs.outgoing);
        assert_eq!(theirs.status, MessageStatus::Received);
    }

    #[test]
    fn what_a_radio_may_be() {
        let ok = settings("Home", RadioKind::Meshtastic, "192.168.1.10");
        assert!(validate(&ok).is_ok());
        // Each kind takes its own port when none is given.
        let port = |kind| match validate(&settings("Home", kind, "radio.lan")) {
            Ok(Transport::Tcp { port, .. }) => port,
            other => panic!("{other:?}"),
        };
        assert_eq!(port(RadioKind::Meshtastic), 4403);
        assert_eq!(port(RadioKind::Meshcore), 5000);
        let cases = [
            (
                settings("  ", RadioKind::Meshtastic, "h"),
                ErrorCode::InvalidName,
            ),
            (
                settings(&"x".repeat(65), RadioKind::Meshtastic, "h"),
                ErrorCode::InvalidName,
            ),
            (
                settings("Home", RadioKind::Meshtastic, "h:port"),
                ErrorCode::InvalidAddress,
            ),
        ];
        for (settings, error) in cases {
            assert_eq!(validate(&settings).unwrap_err(), error, "{settings:?}");
        }
    }

    #[test]
    fn what_a_radio_button_may_show() {
        let with = |color: Option<&str>, content| {
            let mut radio = settings("Home", RadioKind::Meshtastic, "192.168.1.10");
            radio.badge = Badge {
                color: color.map(Into::into),
                content,
            };
            radio
        };
        let text = |text: &str| BadgeContent::Text { text: text.into() };
        let emoji = |emoji: &str| BadgeContent::Emoji {
            emoji: emoji.into(),
        };
        let good = [
            with(Some("#F0a63a"), BadgeContent::Initials),
            with(Some("teal"), BadgeContent::Initials),
            with(None, text("Ñ")),
            with(None, text(" TM ")),
            with(None, text("G3L0")),
            with(None, text("ÑU")),
            with(None, text("🦊")),
            with(None, emoji("📡")),
            with(None, emoji("👨‍👩‍👧")),
            with(None, emoji("🇪🇸")),
            with(
                Some("#000000"),
                BadgeContent::Icon {
                    icon: meshctl_api::BadgeIcon::Meshcore,
                },
            ),
        ];
        for radio in good {
            assert!(validate(&radio).is_ok(), "{radio:?}");
        }
        let bad = [
            with(Some("f0a63a"), BadgeContent::Initials),
            with(Some("red"), BadgeContent::Initials),
            with(Some("#f0a63"), BadgeContent::Initials),
            with(Some("#g0a63a"), BadgeContent::Initials),
            with(Some("#f0a63a00"), BadgeContent::Initials),
            with(None, text("")),
            with(None, text("ABCDE")),
            with(None, text("ÑAND")),
            with(None, text("🦊M")),
            with(None, text("A B")),
            with(None, emoji("")),
            with(None, emoji("ok")),
            with(None, emoji("📡 📡")),
            with(None, emoji(&"📡".repeat(17))),
        ];
        for radio in bad {
            assert_eq!(
                validate(&radio).unwrap_err(),
                ErrorCode::InvalidBadge,
                "{radio:?}"
            );
        }
        // Kept trimmed, and the colour in lower case.
        let kept = tidy(with(Some("#F0A63A"), text(" TM ")));
        assert_eq!(kept.badge.color.as_deref(), Some("#f0a63a"));
        assert_eq!(kept.badge.content, text("TM"));
    }

    #[tokio::test]
    async fn radios_keep_the_order_given() {
        let radios = Radios::start(Store::in_memory().await).await.unwrap();
        let mut ids = Vec::new();
        for name in ["A", "B", "C"] {
            let mut radio = settings(name, RadioKind::Meshtastic, "127.0.0.1:1");
            radio.enabled = false;
            ids.push(radios.add(radio).await.unwrap().id);
        }
        let names = async || {
            let list = radios.list().await.unwrap();
            list.into_iter().map(|r| r.name).collect::<Vec<_>>()
        };
        assert_eq!(names().await, ["A", "B", "C"]);

        let mut changes = radios.subscribe();
        assert!(radios.reorder(&[ids[2], ids[0], ids[1]]).await.unwrap());
        assert!(matches!(changes.recv().await.unwrap(), Change::Radios));
        assert_eq!(names().await, ["C", "A", "B"]);

        // Every radio, once: else nothing changes.
        for wrong in [
            vec![ids[0], ids[1]],
            vec![ids[0], ids[1], ids[1]],
            vec![ids[0], ids[1], ids[2], 99],
        ] {
            assert!(!radios.reorder(&wrong).await.unwrap(), "{wrong:?}");
        }
        assert_eq!(names().await, ["C", "A", "B"]);

        // One added later goes last.
        let mut last = settings("D", RadioKind::Meshtastic, "127.0.0.1:1");
        last.enabled = false;
        radios.add(last).await.unwrap();
        assert_eq!(names().await, ["C", "A", "B", "D"]);
    }

    #[tokio::test]
    async fn a_new_look_keeps_the_link() {
        let radios = Radios::start(Store::in_memory().await).await.unwrap();
        let radio = radios
            .add(settings("Home", RadioKind::Meshtastic, "127.0.0.1:1"))
            .await
            .unwrap();
        let running = |radios: &Radios| {
            let lock = radios.lock();
            let tasks = &lock[&radio.id].tasks;
            tasks.iter().map(|task| task.id()).collect::<Vec<_>>()
        };
        let before = running(&radios);

        let mut look = settings("Home", RadioKind::Meshtastic, "127.0.0.1:1");
        look.shown = false;
        look.badge.color = Some("#3a86f0".into());
        let changed = radios.update(radio.id, look).await.unwrap().unwrap();
        assert!(!changed.shown);
        assert_eq!(changed.badge.color.as_deref(), Some("#3a86f0"));
        assert_eq!(running(&radios), before);

        // Another address is another link.
        let moved = settings("Home", RadioKind::Meshtastic, "127.0.0.1:2");
        radios.update(radio.id, moved).await.unwrap();
        assert_ne!(running(&radios), before);
    }

    #[tokio::test]
    async fn a_disabled_radio_keeps_its_mesh_but_no_link() {
        let radios = Radios::start(Store::in_memory().await).await.unwrap();
        let mut changes = radios.subscribe();

        // Nothing listens there: the link keeps trying, which is all we need.
        let radio = radios
            .add(settings("Home", RadioKind::Meshtastic, "127.0.0.1:1"))
            .await
            .unwrap();
        assert!(matches!(changes.recv().await.unwrap(), Change::Radios));
        assert_ne!(radios.link(radio.id), LinkState::Disabled);

        let mut off = settings("Home", RadioKind::Meshtastic, "127.0.0.1:1");
        off.enabled = false;
        let radio = radios.update(radio.id, off).await.unwrap().unwrap();
        assert_eq!(radio.link, LinkState::Disabled);
        assert!(radios.mesh(radio.id).is_some());

        assert!(radios.remove(radio.id).await.unwrap());
        assert!(radios.mesh(radio.id).is_none());
        assert!(radios.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stored_radios_start_with_what_they_knew() {
        let store = Store::in_memory().await;
        let mut off = settings("Car", RadioKind::Meshtastic, "/dev/ttyACM0");
        off.enabled = false;
        let added = store.add_radio(&off).await.unwrap();
        let faro = mesh::Node {
            num: 9,
            user: None,
            position: None,
            last_heard: Some(5),
            snr: None,
            rssi: None,
            hops_away: Some(1),
            via_mqtt: false,
            favorite: false,
            battery_level: None,
            voltage: None,
            channel_utilization: None,
        };
        store.save_node(added.id, &faro).await.unwrap();

        let radios = Radios::start(store).await.unwrap();
        let known = radios.mesh(added.id).unwrap().read(|s| s.nodes.clone());
        assert_eq!(known.get(&9), Some(&faro));
        assert_eq!(radios.link(added.id), LinkState::Disabled);

        radios.stop().await;
        assert!(radios.mesh(added.id).is_none());
    }

    #[tokio::test]
    async fn a_meshtastic_radio_goes_by_its_short_name() {
        let store = Store::in_memory().await;
        let mut off = settings("Car", RadioKind::Meshtastic, "/dev/ttyACM0");
        off.enabled = false;
        let car = store.add_radio(&off).await.unwrap();
        let mut core = settings("Core", RadioKind::Meshcore, "/dev/ttyACM1");
        core.enabled = false;
        let core = store.add_radio(&core).await.unwrap();
        let own = |num| mesh::Node {
            num,
            user: Some(mesh::User {
                long_name: "G3L0 M0V1L".into(),
                short_name: "G3L0".into(),
                hw_model: "HELTEC_V3".into(),
                role: "CLIENT".into(),
            }),
            position: None,
            last_heard: None,
            snr: None,
            rssi: None,
            hops_away: Some(0),
            via_mqtt: false,
            favorite: false,
            battery_level: None,
            voltage: None,
            channel_utilization: None,
        };
        let local = |num| mesh::LocalNode {
            num,
            firmware_version: None,
            hw_model: None,
            role: None,
            region: None,
            modem_preset: None,
        };
        for (radio, num) in [(car.id, 7), (core.id, 8)] {
            store
                .save_sync(radio, Some(&local(num)), &[own(num)], 1)
                .await
                .unwrap();
        }

        let radios = Radios::start(store).await.unwrap();
        let short = |id| radios.get(id);
        assert_eq!(
            short(car.id).await.unwrap().unwrap().short_name.as_deref(),
            Some("G3L0")
        );
        // MeshCore's nodes have no short names.
        assert_eq!(short(core.id).await.unwrap().unwrap().short_name, None);
        radios.stop().await;
    }
}
