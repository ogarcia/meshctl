// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios meshctl knows of. Each one has a mesh, started from what was
//! stored of it and kept stored as it changes; an enabled one also has a link
//! that keeps that mesh current. They are started, restarted and stopped as
//! radios are added, changed and removed.

use crate::store::{NewMessageRow, RadioRecord, Store, now};
use anyhow::Result;
use meshctl_api::{
    Conversation, ErrorCode, Hop, LinkState, Message, MessageId, MessageStatus, NewMessage,
    NodeSettings, NodeSettingsChange, Peer, Radio, RadioId, RadioKind, RadioSettings,
    SettingsApplied, Status, Traceroute,
};
use meshctl_core::link::Transport;
use meshctl_core::mesh::{self, MeshState};
use meshctl_core::radio::{AdminError, Mesh, OutgoingText, SendError};
use std::collections::HashMap;
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

/// Why a radio's settings were not shown or changed.
#[derive(Debug)]
pub enum SettingsFailure {
    NoSuchRadio,
    /// Not connected, or not yet in sync: its settings are known only then.
    NotConnected,
    Invalid(crate::settings::Invalid),
    NoAnswer,
}

/// Why a traceroute was not sent.
#[derive(Debug)]
pub enum TraceFailure {
    NoSuchRadio,
    NotConnected,
    /// Everyone, or the radio itself.
    InvalidNode,
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

/// Checks what a client asks a radio to be, and says where it is.
pub fn validate(settings: &RadioSettings) -> Result<Transport, ErrorCode> {
    let name = settings.name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(ErrorCode::InvalidName);
    }
    if settings.kind != RadioKind::Meshtastic {
        return Err(ErrorCode::UnsupportedKind);
    }
    settings
        .address
        .trim()
        .parse()
        .map_err(|_| ErrorCode::InvalidAddress)
}

/// Settings as they are kept: trimmed.
pub fn tidy(settings: RadioSettings) -> RadioSettings {
    RadioSettings {
        name: settings.name.trim().to_owned(),
        address: settings.address.trim().to_owned(),
        ..settings
    }
}

/// A radio's mesh, and the tasks that keep it: its link when it is enabled,
/// and always one that passes its events on and one that stores them.
struct Running {
    mesh: Mesh,
    enabled: bool,
    tasks: Vec<JoinHandle<()>>,
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
        let packet_id = mesh.send_text(&outgoing).map_err(|error| match error {
            SendError::NotConnected => SendFailure::NotConnected,
            SendError::BadText => SendFailure::Invalid,
        })?;
        let from = mesh.read(|state| state.local.as_ref().map_or(0, |l| l.num));
        let row = NewMessageRow {
            packet_id,
            from,
            to,
            channel,
            text: message.text,
            at: now(),
            outgoing: true,
            status: MessageStatus::Sending,
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
    pub async fn traceroute(&self, id: RadioId, node: u32) -> Result<Traceroute, TraceFailure> {
        let mesh = self.mesh(id).ok_or(TraceFailure::NoSuchRadio)?;
        let origin = mesh.read(|state| state.local.as_ref().map(|l| l.num));
        if node == mesh::BROADCAST || origin == Some(node) {
            return Err(TraceFailure::InvalidNode);
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
        let packet_id = mesh.send_traceroute(node, 0).map_err(|error| match error {
            SendError::NotConnected | SendError::BadText => TraceFailure::NotConnected,
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

    /// A radio's own settings, as it sent them on syncing. Only while it is
    /// connected: they are not kept, and a radio that is not there cannot be
    /// changed anyway.
    pub fn settings(&self, id: RadioId) -> Result<NodeSettings, SettingsFailure> {
        let mesh = self.mesh(id).ok_or(SettingsFailure::NoSuchRadio)?;
        if self.link(id) != LinkState::Ready {
            return Err(SettingsFailure::NotConnected);
        }
        mesh.read(|state| crate::settings::view(&state.settings))
            .ok_or(SettingsFailure::NotConnected)
    }

    /// Changes a radio's own settings. Returns whether it reboots to apply
    /// them.
    pub async fn change_settings(
        &self,
        id: RadioId,
        wanted: &NodeSettingsChange,
    ) -> Result<SettingsApplied, SettingsFailure> {
        let mesh = self.mesh(id).ok_or(SettingsFailure::NoSuchRadio)?;
        if self.link(id) != LinkState::Ready {
            return Err(SettingsFailure::NotConnected);
        }
        let (change, reboots) = mesh
            .read(|state| {
                crate::settings::change(&state.settings, wanted, |key| {
                    getrandom::fill(key).expect("the system has randomness to give");
                })
            })
            .map_err(SettingsFailure::Invalid)?;
        mesh.change_settings(change)
            .await
            .map_err(|error| match error {
                AdminError::NotConnected => SettingsFailure::NotConnected,
                AdminError::NoAnswer => SettingsFailure::NoAnswer,
            })?;
        tracing::info!(radio = id, reboots, "settings changed");
        Ok(SettingsApplied { reboots })
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

    /// Changes a radio, already validated, reconnecting to it as it is now.
    pub async fn update(&self, id: RadioId, settings: RadioSettings) -> Result<Option<Radio>> {
        if !self.store.update_radio(id, &settings).await? {
            return Ok(None);
        }
        let Some(record) = self.store.radio(id).await? else {
            return Ok(None);
        };
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
            last_connected: record.last_connected,
        }
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
        let mut tasks = vec![self.forward(id, &mesh), self.persist(id, &mesh)];
        let mut enabled = record.settings.enabled;
        if enabled {
            match validate(&record.settings) {
                Ok(transport) => {
                    tracing::info!(radio = id, name = %record.settings.name, %transport, "connecting");
                    tasks.push(mesh.spawn(transport));
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
    fn persist(&self, id: RadioId, mesh: &Mesh) -> JoinHandle<()> {
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
                        let before = now() - TRACEROUTE_TIMEOUT;
                        match store.expire_traceroutes(id, before).await {
                            Ok(expired) => {
                                for traceroute in expired {
                                    let _ = changes.send(Change::Traceroute(id, traceroute));
                                }
                            }
                            Err(err) => tracing::warn!(
                                radio = id,
                                error = format_args!("{err:#}"),
                                "cannot store"
                            ),
                        }
                        continue;
                    }
                };
                let stored = match event {
                    Ok(mesh::Event::Node(node)) => store.save_node(id, &node).await,
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
                        // A relay giving up on a traceroute says so this way.
                        if let Some(error) = &ack.error
                            && let Err(err) =
                                fail_traceroute(&store, &changes, id, ack.packet_id, error).await
                        {
                            tracing::warn!(
                                radio = id,
                                error = format_args!("{err:#}"),
                                "cannot store"
                            );
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
                        save_all(&store, id, &mesh).await
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

    /// Puts a mesh in place of a radio's link, for the tests.
    #[cfg(test)]
    pub fn insert_mesh(&self, id: RadioId, mesh: Mesh) {
        let forward = self.forward(id, &mesh);
        self.lock().insert(
            id,
            Running {
                mesh,
                enabled: true,
                tasks: vec![forward],
            },
        );
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
                settings("Home", RadioKind::Meshcore, "h"),
                ErrorCode::UnsupportedKind,
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
        };
        store.save_node(added.id, &faro).await.unwrap();

        let radios = Radios::start(store).await.unwrap();
        let known = radios.mesh(added.id).unwrap().read(|s| s.nodes.clone());
        assert_eq!(known.get(&9), Some(&faro));
        assert_eq!(radios.link(added.id), LinkState::Disabled);

        radios.stop().await;
        assert!(radios.mesh(added.id).is_none());
    }
}
