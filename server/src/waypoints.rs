// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Waypoints: kept as each radio hears them, and shared, changed or deleted
//! through it, as the official apps do.
//!
//! The firmware refuses a waypoint sent within ten seconds of the last, so
//! meshctl spaces them itself and says so, rather than send one to be
//! refused.

use crate::radios::{Change, Radios, has_channel};
use crate::store::{Store, WaypointSource, WaypointTaken, now};
use anyhow::Result;
use meshctl_api::LinkState;
use meshctl_api::RadioId;
use meshctl_api::waypoints::{NewWaypoint, Waypoint};
use meshctl_core::radio::SendError;
use meshctl_core::waypoint::{FenceBox, HeardWaypoint, Waypoint as Shared};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// The least time between two waypoints sent through a radio: the
/// firmware's.
pub const SPACING: Duration = Duration::from_secs(10);

/// Why a waypoint could not be shared, changed or deleted.
#[derive(Debug)]
pub enum WaypointFailure {
    NoSuchRadio,
    NoSuchWaypoint,
    NotConnected,
    /// Not one that can be shared, or not through this radio: a MeshCore
    /// one, or a channel it has not.
    Invalid,
    /// Locked to another node.
    Locked,
    /// Another went through the radio too recently.
    TooSoon,
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for WaypointFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Failed(err)
    }
}

impl From<SendError> for WaypointFailure {
    fn from(err: SendError) -> Self {
        match err {
            SendError::NotConnected => Self::NotConnected,
            SendError::BadText | SendError::Refused | SendError::Unsupported => Self::Invalid,
        }
    }
}

/// Tells every client what came of a waypoint.
fn tell(changes: &broadcast::Sender<Change>, radio: RadioId, kept: WaypointTaken) {
    let change = match kept {
        WaypointTaken::Kept(waypoint) => Change::Waypoint(radio, *waypoint),
        WaypointTaken::Gone(id) => Change::WaypointGone(radio, id),
        WaypointTaken::Ignored => return,
    };
    let _ = changes.send(change);
}

/// Keeps a waypoint a radio heard, and tells of it.
pub(crate) async fn keep_heard(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
    heard: &HeardWaypoint,
) -> Result<()> {
    // Nowhere, as an unset position reads, is no place to keep; but a
    // deletion may come so.
    let w = &heard.waypoint;
    if w.latitude == 0.0 && w.longitude == 0.0 && w.expire != meshctl_core::waypoint::DELETED {
        return Ok(());
    }
    let source = WaypointSource {
        from: heard.from,
        channel: heard.channel,
        at: i64::from(heard.at),
        own: false,
        local: false,
        hops: None,
    };
    let kept = store.keep_waypoint(radio, &heard.waypoint, source).await?;
    tell(changes, radio, kept);
    Ok(())
}

/// Forgets a radio's waypoints expired by now, and tells of them.
pub(crate) async fn expire(
    store: &Store,
    changes: &broadcast::Sender<Change>,
    radio: RadioId,
) -> Result<()> {
    for id in store.expire_waypoints(radio, now()).await? {
        let _ = changes.send(Change::WaypointGone(radio, id));
    }
    Ok(())
}

/// A waypoint's emoji as a code point: the first character, past the
/// selectors that ask for one to be drawn as an emoji or as text; 0 for
/// none, which is a 📍.
fn icon(icon: Option<&str>) -> u32 {
    icon.and_then(|icon| icon.chars().find(|c| !matches!(c, '\u{fe0e}' | '\u{fe0f}')))
        .map_or(0, u32::from)
}

/// A waypoint id no other of the radio's has: random, and over 255, as the
/// iOS app makes them.
async fn fresh_id(store: &Store, radio: RadioId) -> Result<u32> {
    loop {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes).expect("the system has randomness to give");
        let id = u32::from_le_bytes(bytes);
        if id > 255 && store.waypoint(radio, id).await?.is_none() {
            return Ok(id);
        }
    }
}

impl Radios {
    /// A radio's waypoints, those not gone, the newest first; none if there
    /// is no such radio.
    pub async fn waypoints(&self, radio: RadioId) -> Result<Option<Vec<Waypoint>>> {
        if self.store.radio(radio).await?.is_none() {
            return Ok(None);
        }
        Ok(Some(self.store.waypoints(radio).await?))
    }

    /// Shares a waypoint through a radio, to everyone on a channel: locked
    /// to it, if asked. It is kept as it went, and every client hears of it.
    pub async fn share_waypoint(
        &self,
        radio: RadioId,
        new: NewWaypoint,
    ) -> Result<Waypoint, WaypointFailure> {
        self.send_waypoint(radio, None, new).await
    }

    /// Changes a waypoint, for everyone: sent again, with its id, as it is
    /// now. Not one locked to another node.
    pub async fn change_waypoint(
        &self,
        radio: RadioId,
        id: u32,
        new: NewWaypoint,
    ) -> Result<Waypoint, WaypointFailure> {
        self.send_waypoint(radio, Some(id), new).await
    }

    async fn send_waypoint(
        &self,
        radio: RadioId,
        id: Option<u32>,
        new: NewWaypoint,
    ) -> Result<Waypoint, WaypointFailure> {
        let mesh = self.mesh(radio).ok_or(WaypointFailure::NoSuchRadio)?;
        if !new.is_valid(now()) {
            return Err(WaypointFailure::Invalid);
        }
        let kept = match id {
            Some(id) => Some(
                self.store
                    .waypoint(radio, id)
                    .await?
                    .ok_or(WaypointFailure::NoSuchWaypoint)?,
            ),
            None => None,
        };
        let was_local = kept.as_ref().is_some_and(|(w, _)| w.local);
        // One on the mesh stays there: it cannot be made meshctl's alone.
        if kept.is_some() && !was_local && new.local {
            return Err(WaypointFailure::Invalid);
        }
        // Sent only through a Meshtastic radio, connected, on a channel it has.
        if !new.local {
            if mesh.read(|state| state.meshcore.is_some()) {
                return Err(WaypointFailure::Invalid);
            }
            if self.link(radio) != LinkState::Ready {
                return Err(WaypointFailure::NotConnected);
            }
            if !has_channel(&mesh, new.channel) {
                return Err(WaypointFailure::Invalid);
            }
        }
        let local_num = mesh.read(|state| state.local.as_ref().map(|l| l.num));
        let local = if new.local {
            local_num.unwrap_or(0)
        } else {
            local_num.ok_or(WaypointFailure::NotConnected)?
        };
        if let Some((_, shared)) = &kept
            && !was_local
            && !shared.changeable_by(local)
        {
            return Err(WaypointFailure::Locked);
        }
        let id = match id {
            Some(id) => id,
            None => fresh_id(&self.store, radio).await?,
        };
        let fence = new.fence.filter(meshctl_api::waypoints::Fence::is_some);
        let shared = Shared {
            id,
            latitude: new.latitude,
            longitude: new.longitude,
            expire: new
                .expire
                .map_or(0, |at| u32::try_from(at).unwrap_or(u32::MAX)),
            locked_to: if new.locked && !new.local { local } else { 0 },
            name: new.name.trim().to_owned(),
            description: new.description.trim().to_owned(),
            icon: icon(new.icon.as_deref()),
            fence_radius: fence.and_then(|f| f.radius).unwrap_or(0),
            fence_box: fence.and_then(|f| f.area).map(|area| FenceBox {
                south: area.south,
                west: area.west,
                north: area.north,
                east: area.east,
            }),
            notify_on_enter: fence.is_some_and(|f| f.notify_on_enter),
            notify_on_exit: fence.is_some_and(|f| f.notify_on_exit),
            notify_favorites_only: fence.is_some_and(|f| f.notify_favorites_only),
        }
        .fitted();
        let channel = if new.local { 0 } else { new.channel };
        if !new.local {
            self.spaced(radio)?;
            mesh.send_waypoint(channel, &shared, new.hops)?;
        }
        let source = WaypointSource {
            from: local,
            channel,
            at: now(),
            own: true,
            local: new.local,
            hops: if new.local { None } else { new.hops },
        };
        match self.store.keep_waypoint(radio, &shared, source).await? {
            WaypointTaken::Kept(waypoint) => {
                self.tell(Change::Waypoint(radio, (*waypoint).clone()));
                Ok(*waypoint)
            }
            other => Err(WaypointFailure::Failed(anyhow::anyhow!(
                "the waypoint shared was not kept: {other:?}"
            ))),
        }
    }

    /// Deletes a waypoint: for everyone, sent again as deleted, which only
    /// whoever may change it can; or in meshctl alone, where any can be.
    /// Every client hears it is gone.
    pub async fn delete_waypoint(
        &self,
        radio: RadioId,
        id: u32,
        everyone: bool,
    ) -> Result<(), WaypointFailure> {
        let mesh = self.mesh(radio).ok_or(WaypointFailure::NoSuchRadio)?;
        let (kept, shared) = self
            .store
            .waypoint(radio, id)
            .await?
            .ok_or(WaypointFailure::NoSuchWaypoint)?;
        // One of meshctl's alone is nobody else's to delete it for.
        if everyone && !kept.local {
            if self.link(radio) != LinkState::Ready {
                return Err(WaypointFailure::NotConnected);
            }
            let local = mesh
                .read(|state| state.local.as_ref().map(|l| l.num))
                .ok_or(WaypointFailure::NotConnected)?;
            if !shared.changeable_by(local) {
                return Err(WaypointFailure::Locked);
            }
            // On the channel it was last on, or the primary if the radio
            // has that no more.
            let channel = if has_channel(&mesh, kept.channel) {
                kept.channel
            } else {
                0
            };
            self.spaced(radio)?;
            // As far as it went.
            mesh.send_waypoint(channel, &shared.deleted(), kept.hops)?;
        }
        if self.store.forget_waypoint(radio, id).await? {
            self.tell(Change::WaypointGone(radio, id));
        }
        Ok(())
    }

    /// Watches a waypoint's fence, telling of nodes going in or out of it,
    /// or no longer; `false` if there is no such waypoint.
    pub async fn watch_waypoint(&self, radio: RadioId, id: u32, watched: bool) -> Result<bool> {
        if !self.store.watch_waypoint(radio, id, watched).await? {
            return Ok(false);
        }
        if let Some((waypoint, _)) = self.store.waypoint(radio, id).await? {
            self.tell(Change::Waypoint(radio, waypoint));
        }
        Ok(true)
    }

    /// Keeps whether a radio's map shows its waypoints, and tells every
    /// client; `false` if there is no such radio.
    pub async fn set_map_waypoints(&self, radio: RadioId, shown: bool) -> Result<bool> {
        if !self.store.set_map_waypoints(radio, shown).await? {
            return Ok(false);
        }
        self.tell(Change::Radios);
        Ok(true)
    }

    /// Takes the radio's turn to send a waypoint, if the last went long
    /// enough ago.
    fn spaced(&self, radio: RadioId) -> Result<(), WaypointFailure> {
        let mut sent = self
            .waypoint_sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sent.get(&radio).is_some_and(|at| at.elapsed() < SPACING) {
            return Err(WaypointFailure::TooSoon);
        }
        sent.insert(radio, Instant::now());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_is_its_first_character_past_the_selectors() {
        assert_eq!(icon(None), 0);
        assert_eq!(icon(Some("")), 0);
        assert_eq!(icon(Some("⛰️")), 0x26F0);
        assert_eq!(icon(Some("\u{fe0f}🚩")), 0x1F6A9);
    }
}
