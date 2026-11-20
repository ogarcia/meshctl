// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl keeps between runs, in SQLite: the radios, and what each one
//! last knew of itself and of the mesh.

use anyhow::{Context, Result};
use meshctl_api::{RadioId, RadioKind, RadioSettings};
use meshctl_core::mesh::{LocalNode, Node, Position, User};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The database's file, inside the data directory.
const DATABASE_FILE: &str = "meshctl.db";

/// A radio as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioRecord {
    pub id: RadioId,
    pub settings: RadioSettings,
    /// When it was last connected and in sync, in seconds since the epoch.
    pub last_connected: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct RadioRow {
    id: i64,
    name: String,
    kind: String,
    address: String,
    enabled: bool,
    last_connected: Option<i64>,
}

impl RadioRow {
    /// `None` for a kind this build does not know, which a newer one may have
    /// stored: it is left alone rather than lost.
    fn record(self) -> Option<RadioRecord> {
        Some(RadioRecord {
            id: self.id,
            settings: RadioSettings {
                name: self.name,
                kind: kind_from(&self.kind)?,
                address: self.address,
                enabled: self.enabled,
            },
            last_connected: self.last_connected,
        })
    }
}

#[derive(sqlx::FromRow)]
struct LocalRow {
    local_num: Option<i64>,
    local_firmware_version: Option<String>,
    local_hw_model: Option<String>,
    local_role: Option<String>,
    local_region: Option<String>,
    local_modem_preset: Option<String>,
}

#[derive(sqlx::FromRow)]
struct NodeRow {
    num: i64,
    long_name: Option<String>,
    short_name: Option<String>,
    hw_model: Option<String>,
    role: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    altitude: Option<i32>,
    last_heard: Option<i64>,
    snr: Option<f32>,
    rssi: Option<i32>,
    hops_away: Option<i64>,
    via_mqtt: bool,
    favorite: bool,
    battery_level: Option<i64>,
    voltage: Option<f32>,
}

impl NodeRow {
    fn node(self) -> Node {
        // A user is all four or nothing in the core; what was stored of it
        // came from one, so any of them means there was one.
        let user = (self.long_name.is_some() || self.short_name.is_some()).then(|| User {
            long_name: self.long_name.unwrap_or_default(),
            short_name: self.short_name.unwrap_or_default(),
            hw_model: self.hw_model.unwrap_or_default(),
            role: self.role.unwrap_or_default(),
        });
        let position = match (self.latitude, self.longitude) {
            (Some(latitude), Some(longitude)) => Some(Position {
                latitude,
                longitude,
                altitude: self.altitude,
            }),
            _ => None,
        };
        Node {
            num: u32_from(self.num),
            user,
            position,
            last_heard: self.last_heard.map(u32_from),
            snr: self.snr,
            rssi: self.rssi,
            hops_away: self.hops_away.map(u32_from),
            via_mqtt: self.via_mqtt,
            favorite: self.favorite,
            battery_level: self.battery_level.map(u32_from),
            voltage: self.voltage,
        }
    }
}

/// SQLite has no unsigned integers; node numbers and times are stored as the
/// signed 64-bit ones they fit in, and come back the same way.
fn u32_from(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

const NODE_UPSERT: &str = "INSERT INTO node (radio_id, num, long_name, short_name, hw_model, role,
        latitude, longitude, altitude, last_heard, snr, rssi, hops_away, via_mqtt, favorite,
        battery_level, voltage)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT (radio_id, num) DO UPDATE SET
        long_name = excluded.long_name, short_name = excluded.short_name,
        hw_model = excluded.hw_model, role = excluded.role,
        latitude = excluded.latitude, longitude = excluded.longitude,
        altitude = excluded.altitude, last_heard = excluded.last_heard,
        snr = excluded.snr, rssi = excluded.rssi, hops_away = excluded.hops_away,
        via_mqtt = excluded.via_mqtt, favorite = excluded.favorite,
        battery_level = excluded.battery_level, voltage = excluded.voltage";

async fn upsert_node<'e, E>(executor: E, radio: RadioId, node: &Node) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let user = node.user.as_ref();
    sqlx::query(NODE_UPSERT)
        .bind(radio)
        .bind(i64::from(node.num))
        .bind(user.map(|u| &u.long_name))
        .bind(user.map(|u| &u.short_name))
        .bind(user.map(|u| &u.hw_model))
        .bind(user.map(|u| &u.role))
        .bind(node.position.map(|p| p.latitude))
        .bind(node.position.map(|p| p.longitude))
        .bind(node.position.and_then(|p| p.altitude))
        .bind(node.last_heard.map(i64::from))
        .bind(node.snr)
        .bind(node.rssi)
        .bind(node.hops_away.map(i64::from))
        .bind(node.via_mqtt)
        .bind(node.favorite)
        .bind(node.battery_level.map(i64::from))
        .bind(node.voltage)
        .execute(executor)
        .await?;
    Ok(())
}

fn kind_name(kind: RadioKind) -> &'static str {
    match kind {
        RadioKind::Meshtastic => "meshtastic",
        RadioKind::Meshcore => "meshcore",
    }
}

fn kind_from(name: &str) -> Option<RadioKind> {
    match name {
        "meshtastic" => Some(RadioKind::Meshtastic),
        "meshcore" => Some(RadioKind::Meshcore),
        _ => None,
    }
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens the database in the data directory, creating both if need be,
    /// and brings its schema up to date.
    pub async fn open(data: &Path) -> Result<Self> {
        std::fs::create_dir_all(data)
            .with_context(|| format!("cannot create the data directory {}", data.display()))?;
        let options = SqliteConnectOptions::new()
            .filename(data.join(DATABASE_FILE))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .connect_with(options)
            .await
            .with_context(|| format!("cannot open the database in {}", data.display()))?;
        Self::migrated(pool).await
    }

    /// A database that lives only as long as the process, for the tests.
    #[cfg(test)]
    pub async fn in_memory() -> Self {
        // One connection, or each would be a database of its own.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        Self::migrated(pool).await.unwrap()
    }

    async fn migrated(pool: SqlitePool) -> Result<Self> {
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("cannot bring the database's schema up to date")?;
        Ok(Self { pool })
    }

    /// Closes the database, leaving only its file behind.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn radios(&self) -> Result<Vec<RadioRecord>> {
        let rows: Vec<RadioRow> = sqlx::query_as(
            "SELECT id, name, kind, address, enabled, last_connected FROM radio ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(RadioRow::record).collect())
    }

    pub async fn radio(&self, id: RadioId) -> Result<Option<RadioRecord>> {
        let row: Option<RadioRow> = sqlx::query_as(
            "SELECT id, name, kind, address, enabled, last_connected FROM radio WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(RadioRow::record))
    }

    pub async fn add_radio(&self, settings: &RadioSettings) -> Result<RadioRecord> {
        let id = sqlx::query(
            "INSERT INTO radio (name, kind, address, enabled, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&settings.name)
        .bind(kind_name(settings.kind))
        .bind(&settings.address)
        .bind(settings.enabled)
        .bind(now())
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        Ok(RadioRecord {
            id,
            settings: settings.clone(),
            last_connected: None,
        })
    }

    /// Changes a radio; `false` if there is no such radio.
    pub async fn update_radio(&self, id: RadioId, settings: &RadioSettings) -> Result<bool> {
        let changed = sqlx::query(
            "UPDATE radio SET name = ?, kind = ?, address = ?, enabled = ? WHERE id = ?",
        )
        .bind(&settings.name)
        .bind(kind_name(settings.kind))
        .bind(&settings.address)
        .bind(settings.enabled)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Removes a radio; `false` if there is no such radio.
    pub async fn remove_radio(&self, id: RadioId) -> Result<bool> {
        let removed = sqlx::query("DELETE FROM radio WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(removed > 0)
    }

    /// The radio as it last described itself, if it ever did.
    pub async fn local(&self, radio: RadioId) -> Result<Option<LocalNode>> {
        let row: Option<LocalRow> = sqlx::query_as(
            "SELECT local_num, local_firmware_version, local_hw_model, local_role, local_region,
                local_modem_preset
            FROM radio WHERE id = ?",
        )
        .bind(radio)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|row| {
            Some(LocalNode {
                num: u32_from(row.local_num?),
                firmware_version: row.local_firmware_version,
                hw_model: row.local_hw_model,
                role: row.local_role,
                region: row.local_region,
                modem_preset: row.local_modem_preset,
            })
        }))
    }

    /// Every node the radio has heard.
    pub async fn nodes(&self, radio: RadioId) -> Result<Vec<Node>> {
        let rows: Vec<NodeRow> = sqlx::query_as(
            "SELECT num, long_name, short_name, hw_model, role, latitude, longitude, altitude,
                last_heard, snr, rssi, hops_away, via_mqtt, favorite, battery_level, voltage
            FROM node WHERE radio_id = ?",
        )
        .bind(radio)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(NodeRow::node).collect())
    }

    pub async fn save_node(&self, radio: RadioId, node: &Node) -> Result<()> {
        upsert_node(&self.pool, radio, node).await
    }

    /// Everything a radio said when it synced, at once: itself, every node it
    /// knows, and that it was connected then.
    pub async fn save_sync(
        &self,
        radio: RadioId,
        local: Option<&LocalNode>,
        nodes: &[Node],
        at: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // Another radio behind the same address: the last one's mesh is not
        // this one's.
        let previous: Option<i64> = sqlx::query_scalar("SELECT local_num FROM radio WHERE id = ?")
            .bind(radio)
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
        if let (Some(previous), Some(local)) = (previous, local)
            && previous != i64::from(local.num)
        {
            sqlx::query("DELETE FROM node WHERE radio_id = ?")
                .bind(radio)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "UPDATE radio SET last_connected = ?, local_num = ?, local_firmware_version = ?,
                local_hw_model = ?, local_role = ?, local_region = ?, local_modem_preset = ?
            WHERE id = ?",
        )
        .bind(at)
        .bind(local.map(|l| i64::from(l.num)))
        .bind(local.and_then(|l| l.firmware_version.as_ref()))
        .bind(local.and_then(|l| l.hw_model.as_ref()))
        .bind(local.and_then(|l| l.role.as_ref()))
        .bind(local.and_then(|l| l.region.as_ref()))
        .bind(local.and_then(|l| l.modem_preset.as_ref()))
        .bind(radio)
        .execute(&mut *tx)
        .await?;
        for node in nodes {
            upsert_node(&mut *tx, radio, node).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// That the radio was connected until `at`.
    pub async fn set_last_connected(&self, radio: RadioId, at: i64) -> Result<()> {
        sqlx::query("UPDATE radio SET last_connected = ? WHERE id = ?")
            .bind(at)
            .bind(radio)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> RadioSettings {
        RadioSettings {
            name: "Home".into(),
            kind: RadioKind::Meshtastic,
            address: "192.168.1.10:4403".into(),
            enabled: true,
        }
    }

    #[tokio::test]
    async fn radios_come_and_go() {
        let store = Store::in_memory().await;
        assert!(store.radios().await.unwrap().is_empty());

        let added = store.add_radio(&home()).await.unwrap();
        assert_eq!(store.radios().await.unwrap(), vec![added.clone()]);

        let mut changed = home();
        changed.enabled = false;
        assert!(store.update_radio(added.id, &changed).await.unwrap());
        assert!(
            !store
                .radio(added.id)
                .await
                .unwrap()
                .unwrap()
                .settings
                .enabled
        );

        assert!(store.remove_radio(added.id).await.unwrap());
        assert!(!store.remove_radio(added.id).await.unwrap());
        assert!(store.radio(added.id).await.unwrap().is_none());
    }

    fn node(num: u32, name: &str) -> Node {
        Node {
            num,
            user: Some(User {
                long_name: name.into(),
                short_name: name[..2].into(),
                hw_model: "HELTEC_V3".into(),
                role: "CLIENT".into(),
            }),
            position: Some(Position {
                latitude: 43.36,
                longitude: -8.41,
                altitude: Some(12),
            }),
            last_heard: Some(1_795_000_000),
            snr: Some(6.5),
            rssi: Some(-90),
            hops_away: Some(2),
            via_mqtt: false,
            favorite: true,
            battery_level: Some(101),
            voltage: Some(4.2),
        }
    }

    fn local(num: u32) -> LocalNode {
        LocalNode {
            num,
            firmware_version: Some("2.8.2".into()),
            hw_model: Some("HELTEC_V3".into()),
            role: Some("CLIENT_BASE".into()),
            region: Some("EU_868".into()),
            modem_preset: Some("LONG_FAST".into()),
        }
    }

    #[tokio::test]
    async fn what_a_radio_knew_comes_back() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        // One numbered above i32::MAX, as plenty of nodes are.
        let nodes = [node(0x9000_0001, "Faro"), node(7, "Casa")];

        store
            .save_sync(radio, Some(&local(7)), &nodes, 1234)
            .await
            .unwrap();
        let mut changed = node(7, "Casa");
        changed.battery_level = Some(55);
        store.save_node(radio, &changed).await.unwrap();

        assert_eq!(store.local(radio).await.unwrap(), Some(local(7)));
        assert_eq!(
            store.radio(radio).await.unwrap().unwrap().last_connected,
            Some(1234)
        );
        let mut back = store.nodes(radio).await.unwrap();
        back.sort_by_key(|n| n.num);
        assert_eq!(back, vec![changed, nodes[0].clone()]);
    }

    #[tokio::test]
    async fn another_radio_at_the_same_address_starts_afresh() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        store
            .save_sync(radio, Some(&local(7)), &[node(1, "Faro")], 1)
            .await
            .unwrap();
        store
            .save_sync(radio, Some(&local(8)), &[node(2, "Mar")], 2)
            .await
            .unwrap();
        let nums: Vec<_> = store
            .nodes(radio)
            .await
            .unwrap()
            .iter()
            .map(|n| n.num)
            .collect();
        assert_eq!(nums, [2]);
    }

    #[tokio::test]
    async fn removing_a_radio_forgets_its_mesh() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        store.save_node(radio, &node(1, "Faro")).await.unwrap();
        store.remove_radio(radio).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM node")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn a_kind_from_the_future_is_left_alone() {
        let store = Store::in_memory().await;
        sqlx::query(
            "INSERT INTO radio (name, kind, address, created_at) VALUES ('x', 'lorawan', 'h', 0)",
        )
        .execute(&store.pool)
        .await
        .unwrap();
        assert!(store.radios().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_data_directory_is_made_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("nested");
        let store = Store::open(&data).await.unwrap();
        store.add_radio(&home()).await.unwrap();
        store.close().await;
        assert!(data.join(DATABASE_FILE).exists());
    }
}
