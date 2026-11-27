// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Waypoints: each radio's, as they last were, by their own ids. What is
//! heard of one is taken as the official apps take it: from anyone unless
//! it is locked to a node, and then from that node alone; and a waypoint
//! deleted or expired goes.

use super::{Store, now};
use anyhow::Result;
use meshctl_api::RadioId;
use meshctl_api::waypoints::{CROSSINGS_TOLD, Fence, FenceArea, FenceCrossing, Waypoint};
use meshctl_core::waypoint::{FenceBox, Waypoint as Shared};

/// What came of a waypoint heard or shared.
#[derive(Debug, PartialEq)]
pub enum Taken {
    /// Kept, new or changed.
    Kept(Box<Waypoint>),
    /// Deleted or expired, and gone.
    Gone(u32),
    /// Not taken: locked to another node, or deleted when it was not kept.
    Ignored,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    latitude: f64,
    longitude: f64,
    name: String,
    description: String,
    icon: i64,
    expire: i64,
    locked_to: i64,
    fence_radius: i64,
    fence_south: Option<f64>,
    fence_west: Option<f64>,
    fence_north: Option<f64>,
    fence_east: Option<f64>,
    notify_on_enter: bool,
    notify_on_exit: bool,
    notify_favorites_only: bool,
    watched: bool,
    channel: i64,
    created_by: i64,
    updated_by: i64,
    created_at: i64,
    updated_at: i64,
}

/// A query of whole waypoints, with what follows its columns: fixed as it
/// is built, as sqlx takes none built at run time.
macro_rules! select {
    ($rest:literal) => {
        concat!(
            "SELECT id, latitude, longitude, name, description, icon, expire, locked_to,
                fence_radius, fence_south, fence_west, fence_north, fence_east,
                notify_on_enter, notify_on_exit, notify_favorites_only, watched, channel,
                created_by, updated_by, created_at, updated_at ",
            $rest
        )
    };
}

/// A number kept as an i64 that is a u32.
fn num(n: i64) -> u32 {
    u32::try_from(n).unwrap_or_default()
}

impl Row {
    fn fence_box(&self) -> Option<FenceBox> {
        Some(FenceBox {
            south: self.fence_south?,
            west: self.fence_west?,
            north: self.fence_north?,
            east: self.fence_east?,
        })
    }

    /// As it goes on the air.
    fn shared(&self) -> Shared {
        Shared {
            id: num(self.id),
            latitude: self.latitude,
            longitude: self.longitude,
            expire: num(self.expire),
            locked_to: num(self.locked_to),
            name: self.name.clone(),
            description: self.description.clone(),
            icon: num(self.icon),
            fence_radius: num(self.fence_radius),
            fence_box: self.fence_box(),
            notify_on_enter: self.notify_on_enter,
            notify_on_exit: self.notify_on_exit,
            notify_favorites_only: self.notify_favorites_only,
        }
    }

    /// As the API tells it.
    fn waypoint(&self) -> Waypoint {
        let shared = self.shared();
        let fenced = shared.is_fenced();
        Waypoint {
            id: shared.id,
            latitude: self.latitude,
            longitude: self.longitude,
            name: self.name.clone(),
            description: self.description.clone(),
            icon: shared.emoji().to_string(),
            expire: (self.expire > 0).then_some(self.expire),
            locked_to: (self.locked_to > 0).then_some(num(self.locked_to)),
            fence: fenced.then(|| Fence {
                radius: (self.fence_radius > 0).then_some(num(self.fence_radius)),
                area: self.fence_box().map(|b| FenceArea {
                    south: b.south,
                    west: b.west,
                    north: b.north,
                    east: b.east,
                }),
                notify_on_enter: self.notify_on_enter,
                notify_on_exit: self.notify_on_exit,
                notify_favorites_only: self.notify_favorites_only,
            }),
            watched: self.watched,
            crossings: Vec::new(),
            channel: num(self.channel),
            created_by: num(self.created_by),
            updated_by: num(self.updated_by),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Where a waypoint was heard or shared from, and when.
#[derive(Debug, Clone, Copy)]
pub struct WaypointSource {
    /// The node that sent it.
    pub from: u32,
    /// The slot it came or went on.
    pub channel: u32,
    /// When, in seconds since the epoch.
    pub at: i64,
    /// Whether the radio itself shared it: its fence is then watched as its
    /// author, the radio, asks.
    pub own: bool,
}

impl Store {
    /// A radio's waypoints, those not gone, the newest first.
    pub async fn waypoints(&self, radio: RadioId) -> Result<Vec<Waypoint>> {
        let rows: Vec<Row> = sqlx::query_as(select!(
            "FROM waypoint WHERE radio_id = ? AND (expire = 0 OR expire > ?)
            ORDER BY updated_at DESC, id"
        ))
        .bind(radio)
        .bind(now())
        .fetch_all(&self.pool)
        .await?;
        let mut waypoints: Vec<Waypoint> = rows.iter().map(Row::waypoint).collect();
        for waypoint in &mut waypoints {
            waypoint.crossings = self.crossings(radio, waypoint.id).await?;
        }
        Ok(waypoints)
    }

    /// The latest nodes gone in or out of a waypoint's fence, the newest
    /// first.
    async fn crossings(&self, radio: RadioId, id: u32) -> Result<Vec<FenceCrossing>> {
        let rows: Vec<(i64, bool, i64)> = sqlx::query_as(
            "SELECT node, entered, at FROM waypoint_crossing
            WHERE radio_id = ? AND waypoint_id = ? ORDER BY at DESC, id DESC LIMIT ?",
        )
        .bind(radio)
        .bind(i64::from(id))
        .bind(i64::try_from(CROSSINGS_TOLD).unwrap_or(5))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(node, entered, at)| FenceCrossing {
                node: num(node),
                entered,
                at,
            })
            .collect())
    }

    /// Keeps a node gone in or out of a waypoint's fence, and forgets the
    /// waypoint's oldest past the latest twenty.
    pub async fn add_crossing(
        &self,
        radio: RadioId,
        id: u32,
        crossing: FenceCrossing,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO waypoint_crossing (radio_id, waypoint_id, node, entered, at)
            VALUES (?, ?, ?, ?, ?)",
        )
        .bind(radio)
        .bind(i64::from(id))
        .bind(i64::from(crossing.node))
        .bind(crossing.entered)
        .bind(crossing.at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM waypoint_crossing WHERE radio_id = ? AND waypoint_id = ? AND id NOT IN
                (SELECT id FROM waypoint_crossing WHERE radio_id = ? AND waypoint_id = ?
                ORDER BY at DESC, id DESC LIMIT 20)",
        )
        .bind(radio)
        .bind(i64::from(id))
        .bind(radio)
        .bind(i64::from(id))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// One of a radio's waypoints, as the API tells it and as it goes on the
    /// air.
    pub async fn waypoint(&self, radio: RadioId, id: u32) -> Result<Option<(Waypoint, Shared)>> {
        let row: Option<Row> =
            sqlx::query_as(select!("FROM waypoint WHERE radio_id = ? AND id = ?"))
                .bind(radio)
                .bind(i64::from(id))
                .fetch_optional(&self.pool)
                .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut waypoint = row.waypoint();
        waypoint.crossings = self.crossings(radio, id).await?;
        Ok(Some((waypoint, row.shared())))
    }

    /// The waypoints whose fences a radio watches, those not gone, as they
    /// go on the air.
    pub async fn watched_waypoints(&self, radio: RadioId) -> Result<Vec<Shared>> {
        let rows: Vec<Row> = sqlx::query_as(select!(
            "FROM waypoint WHERE radio_id = ? AND watched AND (expire = 0 OR expire > ?)"
        ))
        .bind(radio)
        .bind(now())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(Row::shared).collect())
    }

    /// Takes a waypoint heard, or shared by the radio: kept, new or in place
    /// of the one with its id; or gone, deleted or expired; or ignored, from
    /// a node it is not open to.
    pub async fn keep_waypoint(
        &self,
        radio: RadioId,
        waypoint: &Shared,
        source: WaypointSource,
    ) -> Result<Taken> {
        let stored = self.waypoint(radio, waypoint.id).await?;
        // Locked, as kept or as sent, to a node that is not who sends it.
        let locked = stored
            .as_ref()
            .is_some_and(|(_, kept)| !kept.changeable_by(source.from));
        if locked || !waypoint.changeable_by(source.from) {
            return Ok(Taken::Ignored);
        }
        if waypoint.is_gone(u32::try_from(now()).unwrap_or(u32::MAX)) {
            if stored.is_none() {
                return Ok(Taken::Ignored);
            }
            self.forget_waypoint(radio, waypoint.id).await?;
            return Ok(Taken::Gone(waypoint.id));
        }
        // The radio's own, watched as it asks; another's, as it was.
        let watched = source
            .own
            .then(|| waypoint.is_fenced() && (waypoint.notify_on_enter || waypoint.notify_on_exit));
        let fence = waypoint.fence_box;
        sqlx::query(
            "INSERT INTO waypoint (radio_id, id, latitude, longitude, name, description, icon,
                expire, locked_to, fence_radius, fence_south, fence_west, fence_north,
                fence_east, notify_on_enter, notify_on_exit, notify_favorites_only, watched,
                channel, created_by, updated_by, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, 0), ?, ?, ?, ?, ?)
            ON CONFLICT (radio_id, id) DO UPDATE SET
                latitude = excluded.latitude, longitude = excluded.longitude,
                name = excluded.name, description = excluded.description, icon = excluded.icon,
                expire = excluded.expire, locked_to = excluded.locked_to,
                fence_radius = excluded.fence_radius, fence_south = excluded.fence_south,
                fence_west = excluded.fence_west, fence_north = excluded.fence_north,
                fence_east = excluded.fence_east, notify_on_enter = excluded.notify_on_enter,
                notify_on_exit = excluded.notify_on_exit,
                notify_favorites_only = excluded.notify_favorites_only,
                watched = COALESCE(?, watched), channel = excluded.channel,
                updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        )
        .bind(radio)
        .bind(i64::from(waypoint.id))
        .bind(waypoint.latitude)
        .bind(waypoint.longitude)
        .bind(&waypoint.name)
        .bind(&waypoint.description)
        .bind(i64::from(waypoint.icon))
        .bind(i64::from(waypoint.expire))
        .bind(i64::from(waypoint.locked_to))
        .bind(i64::from(waypoint.fence_radius))
        .bind(fence.map(|b| b.south))
        .bind(fence.map(|b| b.west))
        .bind(fence.map(|b| b.north))
        .bind(fence.map(|b| b.east))
        .bind(waypoint.notify_on_enter)
        .bind(waypoint.notify_on_exit)
        .bind(waypoint.notify_favorites_only)
        .bind(watched)
        .bind(i64::from(source.channel))
        .bind(i64::from(source.from))
        .bind(i64::from(source.from))
        .bind(source.at)
        .bind(source.at)
        .bind(watched)
        .execute(&self.pool)
        .await?;
        let (kept, _) = self
            .waypoint(radio, waypoint.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the waypoint just kept is not there"))?;
        Ok(Taken::Kept(Box::new(kept)))
    }

    /// Forgets a waypoint in meshctl alone; `false` if there was none.
    pub async fn forget_waypoint(&self, radio: RadioId, id: u32) -> Result<bool> {
        let gone = sqlx::query("DELETE FROM waypoint WHERE radio_id = ? AND id = ?")
            .bind(radio)
            .bind(i64::from(id))
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(gone > 0)
    }

    /// Watches a waypoint's fence, or no longer; `false` if there is none.
    pub async fn watch_waypoint(&self, radio: RadioId, id: u32, watched: bool) -> Result<bool> {
        let changed = sqlx::query("UPDATE waypoint SET watched = ? WHERE radio_id = ? AND id = ?")
            .bind(watched)
            .bind(radio)
            .bind(i64::from(id))
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(changed > 0)
    }

    /// Forgets a radio's waypoints expired by `at`; their ids.
    pub async fn expire_waypoints(&self, radio: RadioId, at: i64) -> Result<Vec<u32>> {
        let gone: Vec<(i64,)> = sqlx::query_as(
            "DELETE FROM waypoint WHERE radio_id = ? AND expire > 1 AND expire <= ? RETURNING id",
        )
        .bind(radio)
        .bind(at)
        .fetch_all(&self.pool)
        .await?;
        Ok(gone.into_iter().map(|(id,)| num(id)).collect())
    }

    /// Keeps whether a radio's map shows its waypoints; `false` if there is
    /// no such radio.
    pub async fn set_map_waypoints(&self, radio: RadioId, shown: bool) -> Result<bool> {
        let changed = sqlx::query("UPDATE radio SET map_waypoints = ? WHERE id = ?")
            .bind(shown)
            .bind(radio)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_api::{RadioKind, RadioSettings};

    async fn store_with_radio() -> (Store, RadioId) {
        let store = Store::in_memory().await;
        let radio = store
            .add_radio(&RadioSettings {
                name: "Home".into(),
                kind: RadioKind::Meshtastic,
                address: "192.0.2.1".into(),
                enabled: false,
                shown: true,
                badge: Default::default(),
            })
            .await
            .unwrap()
            .id;
        (store, radio)
    }

    async fn watched(store: &Store, radio: RadioId) -> bool {
        store.waypoint(radio, 77).await.unwrap().unwrap().0.watched
    }

    fn summit() -> Shared {
        Shared {
            id: 77,
            latitude: 43.36,
            longitude: -8.41,
            name: "Cumbre".into(),
            icon: 0x26F0,
            fence_radius: 250,
            notify_on_enter: true,
            ..Default::default()
        }
    }

    fn from(node: u32, own: bool) -> WaypointSource {
        WaypointSource {
            from: node,
            channel: 0,
            at: 1000,
            own,
        }
    }

    #[tokio::test]
    async fn a_waypoint_is_kept_changed_and_deleted() {
        let (store, radio) = store_with_radio().await;
        let Taken::Kept(kept) = store
            .keep_waypoint(radio, &summit(), from(5, false))
            .await
            .unwrap()
        else {
            panic!("not kept");
        };
        assert_eq!((kept.id, kept.icon.as_str(), kept.created_by), (77, "⛰", 5));
        assert_eq!(kept.fence.and_then(|f| f.radius), Some(250));
        // Another's, not watched until asked.
        assert!(!watched(&store, radio).await);

        // Changed by someone else: open to anyone.
        let moved = Shared {
            latitude: 43.4,
            ..summit()
        };
        let Taken::Kept(kept) = store
            .keep_waypoint(radio, &moved, from(6, false))
            .await
            .unwrap()
        else {
            panic!("not changed");
        };
        assert_eq!((kept.created_by, kept.updated_by), (5, 6));
        assert!((kept.latitude - 43.4).abs() < 1e-9);

        assert_eq!(
            store
                .keep_waypoint(radio, &summit().deleted(), from(6, false))
                .await
                .unwrap(),
            Taken::Gone(77)
        );
        assert!(store.waypoints(radio).await.unwrap().is_empty());
        // Deleted again, there is nothing left to delete.
        assert_eq!(
            store
                .keep_waypoint(radio, &summit().deleted(), from(6, false))
                .await
                .unwrap(),
            Taken::Ignored
        );
    }

    #[tokio::test]
    async fn locked_it_is_its_owners_alone() {
        let (store, radio) = store_with_radio().await;
        let locked = Shared {
            locked_to: 5,
            ..summit()
        };
        // Sent locked to another than who sends it: not taken.
        assert_eq!(
            store
                .keep_waypoint(radio, &locked, from(6, false))
                .await
                .unwrap(),
            Taken::Ignored
        );
        assert!(matches!(
            store
                .keep_waypoint(radio, &locked, from(5, false))
                .await
                .unwrap(),
            Taken::Kept(_)
        ));
        // Neither changed nor deleted, nor unlocked, by another.
        let unlocked = Shared {
            locked_to: 0,
            name: "Mine now".into(),
            ..summit()
        };
        assert_eq!(
            store
                .keep_waypoint(radio, &unlocked, from(6, false))
                .await
                .unwrap(),
            Taken::Ignored
        );
        assert_eq!(
            store
                .keep_waypoint(radio, &locked.deleted(), from(6, false))
                .await
                .unwrap(),
            Taken::Ignored
        );
        assert_eq!(
            store
                .keep_waypoint(radio, &locked.deleted(), from(5, false))
                .await
                .unwrap(),
            Taken::Gone(77)
        );
    }

    #[tokio::test]
    async fn the_radios_own_fences_are_watched_as_it_asks() {
        let (store, radio) = store_with_radio().await;
        store
            .keep_waypoint(radio, &summit(), from(1, true))
            .await
            .unwrap();
        assert!(watched(&store, radio).await);
        // Changed by another, still watched.
        store
            .keep_waypoint(radio, &summit(), from(2, false))
            .await
            .unwrap();
        assert!(watched(&store, radio).await);
        // Asked not to be told of anything any more.
        let quiet = Shared {
            notify_on_enter: false,
            ..summit()
        };
        store
            .keep_waypoint(radio, &quiet, from(1, true))
            .await
            .unwrap();
        assert!(!watched(&store, radio).await);
        assert!(store.watch_waypoint(radio, 77, true).await.unwrap());
        assert!(watched(&store, radio).await);
    }

    #[tokio::test]
    async fn the_latest_crossings_go_with_their_waypoint() {
        let (store, radio) = store_with_radio().await;
        store
            .keep_waypoint(radio, &summit(), from(5, false))
            .await
            .unwrap();
        for at in 0..25 {
            let crossing = FenceCrossing {
                node: 9,
                entered: at % 2 == 0,
                at: 1000 + at,
            };
            store.add_crossing(radio, 77, crossing).await.unwrap();
        }
        let (kept, _) = store.waypoint(radio, 77).await.unwrap().unwrap();
        assert_eq!(kept.crossings.len(), CROSSINGS_TOLD);
        assert_eq!(kept.crossings[0].at, 1024);
        assert!(kept.crossings[0].entered);
        let left: (i64,) = sqlx::query_as("SELECT count(*) FROM waypoint_crossing")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(left.0, 20);
        // Forgotten, its crossings with it.
        store.forget_waypoint(radio, 77).await.unwrap();
        let left: (i64,) = sqlx::query_as("SELECT count(*) FROM waypoint_crossing")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(left.0, 0);
    }

    #[tokio::test]
    async fn expired_waypoints_go() {
        let (store, radio) = store_with_radio().await;
        let soon = Shared {
            expire: u32::try_from(now()).unwrap() + 100,
            ..summit()
        };
        store
            .keep_waypoint(radio, &soon, from(5, false))
            .await
            .unwrap();
        let never = Shared { id: 78, ..summit() };
        store
            .keep_waypoint(radio, &never, from(5, false))
            .await
            .unwrap();
        assert_eq!(store.waypoints(radio).await.unwrap().len(), 2);
        assert_eq!(
            store.expire_waypoints(radio, now() + 200).await.unwrap(),
            vec![77]
        );
        assert_eq!(store.waypoints(radio).await.unwrap().len(), 1);
        // One already expired as it comes is no waypoint to keep.
        let past = Shared {
            id: 79,
            expire: 500,
            ..summit()
        };
        assert_eq!(
            store
                .keep_waypoint(radio, &past, from(5, false))
                .await
                .unwrap(),
            Taken::Ignored
        );
    }
}
