// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios meshctl knows of. Each one has a mesh, started from what was
//! stored of it and kept stored as it changes; an enabled one also has a link
//! that keeps that mesh current. They are started, restarted and stopped as
//! radios are added, changed and removed.

use crate::store::{RadioRecord, Store, now};
use anyhow::Result;
use meshctl_api::{ErrorCode, LinkState, Radio, RadioId, RadioKind, RadioSettings, Status};
use meshctl_core::link::Transport;
use meshctl_core::mesh::{self, MeshState};
use meshctl_core::radio::Mesh;
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
    /// Something changed on one radio.
    Mesh(RadioId, mesh::Event),
    /// Events of a radio were lost on their way here.
    Lagged,
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
        tokio::spawn(async move {
            let mut connected = false;
            loop {
                let stored = match events.recv().await {
                    Ok(mesh::Event::Node(node)) => store.save_node(id, &node).await,
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
