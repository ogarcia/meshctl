// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios meshctl keeps connected to: what is stored, and a running link
//! for each one that is enabled, started, restarted and stopped as they are
//! added, changed and removed.

use crate::store::{RadioRecord, Store};
use anyhow::Result;
use meshctl_api::{ErrorCode, LinkState, Radio, RadioId, RadioKind, RadioSettings};
use meshctl_core::link::Transport;
use meshctl_core::mesh;
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

/// A radio meshctl is connected to, or trying to be.
struct Running {
    mesh: Mesh,
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
    /// Starts every enabled radio in the store.
    pub async fn start(store: Store) -> Result<Self> {
        let radios = Self {
            store,
            running: Arc::default(),
            changes: broadcast::channel(EVENT_BACKLOG).0,
            stopped: watch::channel(false).0,
        };
        for record in radios.store.radios().await? {
            radios.run(&record);
        }
        Ok(radios)
    }

    /// Everything that changes from now on, on every radio.
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// The live state of a radio, if it is running.
    pub fn mesh(&self, id: RadioId) -> Option<Mesh> {
        self.lock().get(&id).map(|running| running.mesh.clone())
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
        self.run(&record);
        self.changed();
        Ok(self.radio(record))
    }

    /// Changes a radio, already validated, reconnecting to it as it is now.
    pub async fn update(&self, id: RadioId, settings: RadioSettings) -> Result<Option<Radio>> {
        if !self.store.update_radio(id, &settings).await? {
            return Ok(None);
        }
        let record = RadioRecord { id, settings };
        self.lock().remove(&id);
        self.run(&record);
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

    /// Disconnects from every radio, for good: the daemon is stopping.
    pub fn stop(&self) {
        self.stopped.send_replace(true);
        self.lock().clear();
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
        let link = self.mesh(record.id).map_or(LinkState::Disabled, |mesh| {
            crate::web::convert::link(mesh.read(|state| state.link))
        });
        Radio {
            id: record.id,
            name: record.settings.name,
            kind: record.settings.kind,
            address: record.settings.address,
            enabled: record.settings.enabled,
            link,
        }
    }

    /// Connects to a radio, if it is enabled and can be.
    fn run(&self, record: &RadioRecord) {
        if !record.settings.enabled {
            return;
        }
        let transport = match validate(&record.settings) {
            Ok(transport) => transport,
            Err(error) => {
                tracing::warn!(radio = record.id, ?error, "not starting radio");
                return;
            }
        };
        tracing::info!(radio = record.id, name = %record.settings.name, %transport, "starting radio");
        let mesh = Mesh::default();
        let link = mesh.spawn(transport);
        let forward = self.forward(record.id, &mesh);
        self.lock().insert(
            record.id,
            Running {
                mesh,
                tasks: vec![link, forward],
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

    /// Puts a mesh in place of a radio's link, for the tests.
    #[cfg(test)]
    pub fn insert_mesh(&self, id: RadioId, mesh: Mesh) {
        let forward = self.forward(id, &mesh);
        self.lock().insert(
            id,
            Running {
                mesh,
                tasks: vec![forward],
            },
        );
    }
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
    async fn an_enabled_radio_runs_and_a_disabled_one_does_not() {
        let radios = Radios::start(Store::in_memory().await).await.unwrap();
        let mut changes = radios.subscribe();

        // Nothing listens there: the link keeps trying, which is all we need.
        let radio = radios
            .add(settings("Home", RadioKind::Meshtastic, "127.0.0.1:1"))
            .await
            .unwrap();
        assert!(matches!(changes.recv().await.unwrap(), Change::Radios));
        assert!(radios.mesh(radio.id).is_some());

        let mut off = settings("Home", RadioKind::Meshtastic, "127.0.0.1:1");
        off.enabled = false;
        let radio = radios.update(radio.id, off).await.unwrap().unwrap();
        assert_eq!(radio.link, LinkState::Disabled);
        assert!(radios.mesh(radio.id).is_none());

        assert!(radios.remove(radio.id).await.unwrap());
        assert!(radios.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stored_radios_start_with_the_daemon() {
        let store = Store::in_memory().await;
        let added = store
            .add_radio(&settings("Home", RadioKind::Meshtastic, "127.0.0.1:1"))
            .await
            .unwrap();
        let radios = Radios::start(store).await.unwrap();
        assert!(radios.mesh(added.id).is_some());
        radios.stop();
        assert!(radios.mesh(added.id).is_none());
    }
}
