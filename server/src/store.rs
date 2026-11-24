// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl keeps between runs, in SQLite: the radios, and what each one
//! last knew of itself and of the mesh.

use crate::channels::{Chat, Fingerprint, Slot, assign, provisional};
use anyhow::{Context, Result};
use meshctl_api::{
    BROADCAST, Badge, Conversation, Hop, MapView, Message, MessageId, MessageStatus, PacketFilter,
    PacketKind, PacketReach, Peer, Preferences, RadioId, RadioKind, RadioSettings, SightingId,
    Traceroute, TracerouteId, TracerouteStatus,
};
use meshctl_core::mesh::{Heard, LocalNode, Node, Position, PositionSource, User};
use sqlx::SqliteConnection;
use sqlx::sqlite::{
    SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

mod requests;
mod retention;

/// A radio's button as stored: NULL for the one every radio has by default.
fn badge_json(badge: &Badge) -> Result<Option<String>> {
    if *badge == Badge::default() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(badge)?))
}

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
    shown: bool,
    badge: Option<String>,
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
                shown: self.shown,
                // A button this build cannot read is shown as it is by default.
                badge: self
                    .badge
                    .and_then(|badge| serde_json::from_str(&badge).ok())
                    .unwrap_or_default(),
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
    public_key: Option<Vec<u8>>,
    licensed: bool,
    unmessagable: bool,
    latitude: Option<f64>,
    longitude: Option<f64>,
    altitude: Option<i32>,
    position_radius: Option<i64>,
    position_source: Option<String>,
    last_heard: Option<i64>,
    snr: Option<f32>,
    rssi: Option<i32>,
    hops_away: Option<i64>,
    via_mqtt: bool,
    favorite: bool,
    battery_level: Option<i64>,
    voltage: Option<f32>,
    channel_utilization: Option<f32>,
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
            public_key: self.public_key.unwrap_or_default(),
            licensed: self.licensed,
            unmessagable: self.unmessagable,
        });
        let position = match (self.latitude, self.longitude) {
            (Some(latitude), Some(longitude)) => Some(Position {
                latitude,
                longitude,
                altitude: self.altitude,
                radius: self.position_radius.map(u32_from),
                source: self.position_source.as_deref().and_then(source_from),
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
            channel_utilization: self.channel_utilization,
            on_radio: None,
            status: None,
            status_at: None,
        }
    }
}

fn source_name(source: PositionSource) -> &'static str {
    match source {
        PositionSource::Manual => "manual",
        PositionSource::Internal => "internal",
        PositionSource::External => "external",
    }
}

fn source_from(name: &str) -> Option<PositionSource> {
    match name {
        "manual" => Some(PositionSource::Manual),
        "internal" => Some(PositionSource::Internal),
        "external" => Some(PositionSource::External),
        _ => None,
    }
}

/// SQLite has no unsigned integers; node numbers and times are stored as the
/// signed 64-bit ones they fit in, and come back the same way.
fn u32_from(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

const NODE_UPSERT: &str = "INSERT INTO node (radio_id, num, long_name, short_name, hw_model, role,
        public_key, licensed, unmessagable,
        latitude, longitude, altitude, position_radius, position_source, last_heard, snr, rssi,
        hops_away,
        via_mqtt, favorite,
        battery_level, voltage, channel_utilization)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT (radio_id, num) DO UPDATE SET
        long_name = excluded.long_name, short_name = excluded.short_name,
        hw_model = excluded.hw_model, role = excluded.role,
        public_key = excluded.public_key, licensed = excluded.licensed,
        unmessagable = excluded.unmessagable,
        latitude = excluded.latitude, longitude = excluded.longitude,
        altitude = excluded.altitude, position_radius = excluded.position_radius,
        position_source = excluded.position_source,
        last_heard = excluded.last_heard,
        snr = excluded.snr, rssi = excluded.rssi, hops_away = excluded.hops_away,
        via_mqtt = excluded.via_mqtt, favorite = excluded.favorite,
        battery_level = excluded.battery_level, voltage = excluded.voltage,
        channel_utilization = excluded.channel_utilization";

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
        .bind(user.map(|u| &u.public_key).filter(|key| !key.is_empty()))
        .bind(user.is_some_and(|u| u.licensed))
        .bind(user.is_some_and(|u| u.unmessagable))
        .bind(node.position.map(|p| p.latitude))
        .bind(node.position.map(|p| p.longitude))
        .bind(node.position.and_then(|p| p.altitude))
        .bind(node.position.and_then(|p| p.radius).map(i64::from))
        .bind(node.position.and_then(|p| p.source).map(source_name))
        .bind(node.last_heard.map(i64::from))
        .bind(node.snr)
        .bind(node.rssi)
        .bind(node.hops_away.map(i64::from))
        .bind(node.via_mqtt)
        .bind(node.favorite)
        .bind(node.battery_level.map(i64::from))
        .bind(node.voltage)
        .bind(node.channel_utilization)
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
            // What is cleaned away given back to the disk, a little at a
            // time: set as the database is made.
            .auto_vacuum(SqliteAutoVacuum::Incremental)
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

    /// The interface's preferences, or the defaults when none were set. A row
    /// that does not read, written by a newer meshctl, counts as none.
    pub async fn preferences(&self) -> Result<Preferences> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM preference WHERE key = 'interface'")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|(value,)| {
                serde_json::from_str(&value)
                    .inspect_err(|err| tracing::warn!(%err, "ignoring the stored preferences"))
                    .ok()
            })
            .unwrap_or_default())
    }

    pub async fn set_preferences(&self, preferences: &Preferences) -> Result<()> {
        sqlx::query(
            "INSERT INTO preference (key, value) VALUES ('interface', ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(serde_json::to_string(preferences)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The emojis picked in the interface, the most used first and, among
    /// as used, the latest: under 'emojis' in the preferences, as JSON.
    pub async fn emojis(&self) -> Result<Vec<String>> {
        Ok(ranked(self.emoji_uses().await?))
    }

    /// Counts an emoji picked at `at` (seconds since the epoch), and gives the
    /// emojis as [`Self::emojis`] does. Only the most used are kept.
    pub async fn use_emoji(&self, emoji: &str, at: i64) -> Result<Vec<String>> {
        let mut uses = self.emoji_uses().await?;
        match uses.iter_mut().find(|u| u.emoji == emoji) {
            Some(used) => {
                used.count += 1;
                used.last = at;
            }
            None => uses.push(EmojiUse {
                emoji: emoji.to_owned(),
                count: 1,
                last: at,
            }),
        }
        sort_uses(&mut uses);
        uses.truncate(EMOJIS_KEPT);
        sqlx::query(
            "INSERT INTO preference (key, value) VALUES ('emojis', ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(serde_json::to_string(&uses)?)
        .execute(&self.pool)
        .await?;
        Ok(ranked(uses))
    }

    async fn emoji_uses(&self) -> Result<Vec<EmojiUse>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM preference WHERE key = 'emojis'")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|(value,)| serde_json::from_str(&value).ok())
            .unwrap_or_default())
    }

    pub async fn radios(&self) -> Result<Vec<RadioRecord>> {
        let rows: Vec<RadioRow> = sqlx::query_as(
            "SELECT id, name, kind, address, enabled, shown, badge, last_connected FROM radio
            ORDER BY position, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(RadioRow::record).collect())
    }

    pub async fn radio(&self, id: RadioId) -> Result<Option<RadioRecord>> {
        let row: Option<RadioRow> = sqlx::query_as(
            "SELECT id, name, kind, address, enabled, shown, badge, last_connected FROM radio
            WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(RadioRow::record))
    }

    pub async fn add_radio(&self, settings: &RadioSettings) -> Result<RadioRecord> {
        let id = sqlx::query(
            "INSERT INTO radio (name, kind, address, enabled, shown, badge, position, created_at)
            VALUES (?, ?, ?, ?, ?, ?, (SELECT COALESCE(MAX(position), 0) + 1 FROM radio), ?)",
        )
        .bind(&settings.name)
        .bind(kind_name(settings.kind))
        .bind(&settings.address)
        .bind(settings.enabled)
        .bind(settings.shown)
        .bind(badge_json(&settings.badge)?)
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
            "UPDATE radio SET name = ?, kind = ?, address = ?, enabled = ?, shown = ?, badge = ?
            WHERE id = ?",
        )
        .bind(&settings.name)
        .bind(kind_name(settings.kind))
        .bind(&settings.address)
        .bind(settings.enabled)
        .bind(settings.shown)
        .bind(badge_json(&settings.badge)?)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Where the map was left for a radio: `None` if there is no such radio,
    /// `Some(None)` if it was never moved.
    pub async fn map_view(&self, radio: RadioId) -> Result<Option<Option<MapView>>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT map_view FROM radio WHERE id = ?")
                .bind(radio)
                .fetch_optional(&self.pool)
                .await?;
        // One this build cannot read is as none.
        Ok(row.map(|(view,)| view.and_then(|view| serde_json::from_str(&view).ok())))
    }

    /// Keeps where the map was left for a radio; `false` if there is none.
    pub async fn set_map_view(&self, radio: RadioId, view: &MapView) -> Result<bool> {
        let changed = sqlx::query("UPDATE radio SET map_view = ? WHERE id = ?")
            .bind(serde_json::to_string(view)?)
            .bind(radio)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(changed > 0)
    }

    /// Puts the radios in the order given, which names each of them once.
    pub async fn set_radio_order(&self, radios: &[RadioId]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (position, id) in radios.iter().enumerate() {
            sqlx::query("UPDATE radio SET position = ? WHERE id = ?")
                .bind(position as i64 + 1)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
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
            "SELECT num, long_name, short_name, hw_model, role, public_key, licensed, unmessagable,
                latitude, longitude, altitude, position_radius, position_source,
                last_heard, snr, rssi, hops_away, via_mqtt, favorite, battery_level, voltage,
                channel_utilization
            FROM node WHERE radio_id = ?",
        )
        .bind(radio)
        .fetch_all(&self.pool)
        .await?;
        let mut nodes: Vec<Node> = rows.into_iter().map(NodeRow::node).collect();
        // What each said of itself last: always kept, as each node's latest
        // of each kind is.
        let said: Vec<(i64, Vec<u8>, i64)> = sqlx::query_as(
            "SELECT p.from_num, p.payload, p.at FROM packet p JOIN packet_seen s ON s.packet = p.id
            WHERE s.radio_id = ? AND p.portnum = ? ORDER BY p.at, p.id",
        )
        .bind(radio)
        .bind(meshctl_proto::meshtastic::PortNum::NodeStatusApp as i32)
        .fetch_all(&self.pool)
        .await?;
        let mut statuses = HashMap::new();
        for (from, payload, at) in said {
            use prost::Message as _;
            if let Ok(status) = meshctl_proto::meshtastic::StatusMessage::decode(payload.as_slice())
            {
                statuses.insert(from, (status.status, at));
            }
        }
        for node in &mut nodes {
            if let Some((status, at)) = statuses.remove(&i64::from(node.num)) {
                node.status = meshctl_core::mesh::status_of(&status);
                node.status_at = u32::try_from(at).ok();
            }
        }
        Ok(nodes)
    }

    /// The latest packets a radio heard from a node on a port, each as its
    /// hearing and what it carries, the latest first, up to so many.
    pub async fn latest_payloads(
        &self,
        radio: RadioId,
        from: u32,
        portnum: i32,
        limit: u32,
    ) -> Result<Vec<(SightingId, Vec<u8>)>> {
        Ok(sqlx::query_as(
            "SELECT s.id, p.payload FROM packet p JOIN packet_seen s ON s.packet = p.id
            WHERE s.radio_id = ? AND p.from_num = ? AND p.portnum = ?
            ORDER BY p.at DESC, p.id DESC LIMIT ?",
        )
        .bind(radio)
        .bind(i64::from(from))
        .bind(portnum)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// The telemetry a radio heard from a node since a time, each as when it
    /// was heard and what it carries, the oldest first.
    pub async fn telemetry_since(
        &self,
        radio: RadioId,
        from: u32,
        since: i64,
    ) -> Result<Vec<(i64, Vec<u8>)>> {
        Ok(sqlx::query_as(
            "SELECT s.at, p.payload FROM packet p JOIN packet_seen s ON s.packet = p.id
            WHERE s.radio_id = ? AND p.from_num = ? AND p.portnum = ? AND p.at >= ?
            ORDER BY s.at",
        )
        .bind(radio)
        .bind(i64::from(from))
        .bind(meshctl_proto::meshtastic::PortNum::TelemetryApp as i32)
        .bind(since)
        .fetch_all(&self.pool)
        .await?)
    }

    /// How a radio heard a node directly since a time, each hearing as when,
    /// its SNR and its RSSI, the oldest first: not relayed, nor through MQTT.
    pub async fn signal_since(
        &self,
        radio: RadioId,
        from: u32,
        since: i64,
    ) -> Result<Vec<(i64, f64, Option<i64>)>> {
        Ok(sqlx::query_as(
            "SELECT s.at, s.snr, s.rssi FROM packet p JOIN packet_seen s ON s.packet = p.id
            WHERE s.radio_id = ? AND p.from_num = ? AND p.at >= ?
                AND s.snr IS NOT NULL AND s.hop_limit = s.hop_start AND s.via_mqtt = 0
            ORDER BY s.at",
        )
        .bind(radio)
        .bind(i64::from(from))
        .bind(since)
        .fetch_all(&self.pool)
        .await?)
    }

    /// How many hops each packet a radio heard from a node since a time took,
    /// as when it was heard and the hops, the oldest first: not through MQTT,
    /// and only when the packet says how many it started with, as firmware
    /// before 2.3 does not.
    pub async fn hops_since(
        &self,
        radio: RadioId,
        from: u32,
        since: i64,
    ) -> Result<Vec<(i64, i64)>> {
        Ok(sqlx::query_as(
            "SELECT s.at, s.hop_start - s.hop_limit FROM packet p JOIN packet_seen s ON s.packet = p.id
            WHERE s.radio_id = ? AND p.from_num = ? AND p.at >= ?
                AND s.hop_start > 0 AND s.hop_start >= s.hop_limit AND s.via_mqtt = 0
            ORDER BY s.at",
        )
        .bind(radio)
        .bind(i64::from(from))
        .bind(since)
        .fetch_all(&self.pool)
        .await?)
    }

    /// When a radio first heard a node's telemetry, or the node directly,
    /// if it ever did: how far back its charts can go.
    pub async fn oldest_telemetry(&self, radio: RadioId, from: u32) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT min(s.at) FROM packet p JOIN packet_seen s ON s.packet = p.id
            WHERE s.radio_id = ? AND p.from_num = ? AND (p.portnum = ?
                OR (s.snr IS NOT NULL AND s.hop_limit = s.hop_start AND s.via_mqtt = 0))",
        )
        .bind(radio)
        .bind(i64::from(from))
        .bind(meshctl_proto::meshtastic::PortNum::TelemetryApp as i32)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn save_node(&self, radio: RadioId, node: &Node) -> Result<()> {
        upsert_node(&self.pool, radio, node).await
    }

    /// Forgets a node a radio forgot: out of its node list, what was heard
    /// of it kept.
    pub async fn remove_node(&self, radio: RadioId, num: u32) -> Result<()> {
        sqlx::query("DELETE FROM node WHERE radio_id = ? AND num = ?")
            .bind(radio)
            .bind(i64::from(num))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Forgets all a radio has of a node: what it was asked and the routes
    /// traced to it, the conversation with it, and what it was heard to send,
    /// its telemetry with it. What it wrote in channels stays: it is theirs.
    pub async fn forget_node(&self, radio: RadioId, num: u32) -> Result<()> {
        let num = i64::from(num);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM request WHERE radio_id = ? AND node = ?")
            .bind(radio)
            .bind(num)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM traceroute WHERE radio_id = ? AND target = ?")
            .bind(radio)
            .bind(num)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM conversation_read WHERE radio_id = ? AND node = ?")
            .bind(radio)
            .bind(num)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "DELETE FROM message WHERE radio_id = ? AND to_num != ?
                AND (from_num = ? OR to_num = ?)",
        )
        .bind(radio)
        .bind(i64::from(BROADCAST))
        .bind(num)
        .bind(num)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM packet_seen WHERE radio_id = ?
                AND packet IN (SELECT id FROM packet WHERE from_num = ?)",
        )
        .bind(radio)
        .bind(num)
        .execute(&mut *tx)
        .await?;
        // What no radio heard any longer.
        sqlx::query(
            "DELETE FROM packet WHERE from_num = ?
                AND NOT EXISTS (SELECT 1 FROM packet_seen s WHERE s.packet = packet.id)",
        )
        .bind(num)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
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

    /// Keeps a message; `None` if it was already kept, as a message replayed
    /// on reconnecting is.
    /// Keeps a packet a radio heard: the packet, once whoever hears it, and
    /// this radio's hearing of it. `None` when this radio had already heard
    /// it, as when it replays packets on reconnecting.
    pub async fn add_packet(&self, radio: RadioId, heard: &Heard) -> Result<Option<PacketRow>> {
        let mut tx = self.pool.begin().await?;
        // A packet first heard encrypted is read in full once a radio that can
        // decrypt it hears it too.
        sqlx::query(
            "INSERT INTO packet (packet_id, from_num, to_num, portnum, payload, request_id,
                reply_id, want_response, pki, kind, at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (from_num, packet_id) DO UPDATE SET
                portnum = excluded.portnum,
                payload = excluded.payload,
                kind = excluded.kind,
                request_id = excluded.request_id,
                reply_id = excluded.reply_id,
                want_response = excluded.want_response
            WHERE packet.portnum IS NULL AND excluded.portnum IS NOT NULL",
        )
        .bind(i64::from(heard.packet_id))
        .bind(i64::from(heard.from))
        .bind(i64::from(heard.to))
        .bind(heard.portnum)
        .bind(&heard.payload)
        .bind(heard.request_id.map(i64::from))
        .bind(heard.reply_id.map(i64::from))
        .bind(heard.want_response)
        .bind(heard.pki)
        .bind(retention::telemetry_kind(heard.portnum, &heard.payload))
        .bind(i64::from(heard.at))
        .execute(&mut *tx)
        .await?;
        let (packet,): (i64,) =
            sqlx::query_as("SELECT id FROM packet WHERE from_num = ? AND packet_id = ?")
                .bind(i64::from(heard.from))
                .bind(i64::from(heard.packet_id))
                .fetch_one(&mut *tx)
                .await?;
        let channel = chat_of_slot(&mut tx, radio, heard.channel, i64::from(heard.at)).await?;
        let seen = sqlx::query(
            "INSERT INTO packet_seen (packet, radio_id, channel, channel_id, at, snr, rssi,
                hop_limit, hop_start, relay_node, via_mqtt)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (packet, radio_id) DO NOTHING",
        )
        .bind(packet)
        .bind(radio)
        .bind(i64::from(heard.channel))
        .bind(channel)
        .bind(i64::from(heard.at))
        .bind(heard.snr)
        .bind(heard.rssi)
        .bind(i64::from(heard.hop_limit))
        .bind(i64::from(heard.hop_start))
        .bind(heard.relay_node.map(i64::from))
        .bind(heard.via_mqtt)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if seen.rows_affected() == 0 {
            return Ok(None);
        }
        self.sighting(seen.last_insert_rowid()).await
    }

    /// How a radio heard, or sent, the packet that carried one of its
    /// messages: none if it kept no such hearing.
    pub async fn message_sighting(
        &self,
        radio: RadioId,
        id: MessageId,
    ) -> Result<Option<PacketRow>> {
        let seen: Option<(i64,)> = sqlx::query_as(
            "SELECT s.id FROM message m
                JOIN packet p ON p.packet_id = m.packet_id AND p.from_num = m.from_num
                JOIN packet_seen s ON s.packet = p.id AND s.radio_id = m.radio_id
            WHERE m.radio_id = ? AND m.id = ?",
        )
        .bind(radio)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        match seen {
            Some((seen,)) => self.sighting(seen).await,
            None => Ok(None),
        }
    }

    /// A radio's hearing of a packet, with the packet.
    pub async fn sighting(&self, id: SightingId) -> Result<Option<PacketRow>> {
        let row = sqlx::query_as(
            "SELECT s.id, p.packet_id, p.from_num, p.to_num,
                coalesce(
                    (SELECT min(c.slot) FROM channel_slot c WHERE c.channel_id = s.channel_id),
                    s.channel
                ) AS channel,
                p.portnum, p.payload,
                p.request_id, p.reply_id, p.want_response, p.pki, s.at, s.snr, s.rssi,
                s.hop_limit, s.hop_start, s.relay_node, s.via_mqtt
            FROM packet_seen s JOIN packet p ON p.id = s.packet
            WHERE s.id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// The packets a radio heard, the newest first: those heard before a
    /// hearing, with `before`, and those from or to a node, with `node`.
    pub async fn packets(
        &self,
        radio: RadioId,
        before: Option<SightingId>,
        filter: &PacketFilter,
        limit: u32,
    ) -> Result<Vec<PacketRow>> {
        // The channel's slot now, where it has one, the slot it was heard on
        // otherwise: as the packet shows it, and as it is filtered by.
        const SLOT: &str = "coalesce(
                (SELECT min(c.slot) FROM channel_slot c WHERE c.channel_id = s.channel_id),
                s.channel
            )";
        let mut query = sqlx::QueryBuilder::new(format!(
            "SELECT s.id, p.packet_id, p.from_num, p.to_num, {SLOT} AS channel,
                p.portnum, p.payload,
                p.request_id, p.reply_id, p.want_response, p.pki, s.at, s.snr, s.rssi,
                s.hop_limit, s.hop_start, s.relay_node, s.via_mqtt
            FROM packet_seen s JOIN packet p ON p.id = s.packet
            WHERE s.radio_id = "
        ));
        query.push_bind(radio);
        if let Some(before) = before {
            query.push(" AND s.id < ").push_bind(before);
        }
        if let Some(kind) = filter.kind {
            let ports = crate::packets::portnums(kind);
            match kind {
                PacketKind::Encrypted => {
                    query.push(" AND p.portnum IS NULL");
                }
                PacketKind::Other => {
                    query.push(" AND p.portnum IS NOT NULL AND p.portnum NOT IN (");
                    let mut list = query.separated(", ");
                    for port in ports {
                        list.push_bind(port);
                    }
                    query.push(")");
                }
                _ => {
                    query.push(" AND p.portnum IN (");
                    let mut list = query.separated(", ");
                    for port in ports {
                        list.push_bind(port);
                    }
                    query.push(")");
                }
            }
        }
        if let Some(channel) = filter.channel {
            query
                .push(format!(" AND {SLOT} = "))
                .push_bind(i64::from(channel));
        }
        match filter.reach {
            Some(PacketReach::Direct) => {
                query.push(" AND s.via_mqtt = 0 AND s.hop_start > 0 AND s.hop_start = s.hop_limit");
            }
            Some(PacketReach::Mesh) => {
                query.push(" AND s.via_mqtt = 0 AND s.hop_start > 0 AND s.hop_start > s.hop_limit");
            }
            Some(PacketReach::Mqtt) => {
                query.push(" AND s.via_mqtt = 1");
            }
            None => {}
        }
        if let Some(node) = filter.node.map(i64::from) {
            query
                .push(" AND (p.from_num = ")
                .push_bind(node)
                .push(" OR p.to_num = ")
                .push_bind(node)
                .push(")");
        }
        if let Some(num) = filter.not_from {
            query.push(" AND p.from_num != ").push_bind(i64::from(num));
        }
        query
            .push(" ORDER BY s.id DESC LIMIT ")
            .push_bind(i64::from(limit));
        let rows = query.build_query_as().fetch_all(&self.pool).await?;
        Ok(rows)
    }

    /// Keeps a message: a channel message in the conversation of the
    /// channel in its slot now.
    pub async fn add_message(
        &self,
        radio: RadioId,
        message: &NewMessageRow,
    ) -> Result<Option<Message>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let channel = if message.to == BROADCAST {
            Some(chat_of_slot(&mut tx, radio, message.channel, message.at).await?)
        } else {
            None
        };
        let id = sqlx::query(
            "INSERT INTO message (radio_id, packet_id, from_num, sender_name, to_num, channel,
                channel_id, text, at, outgoing, status, reply_id, emoji, snr, rssi, hops_away,
                via_mqtt, scope, sent_at, author_num)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (radio_id, from_num, packet_id) DO NOTHING",
        )
        .bind(radio)
        .bind(i64::from(message.packet_id))
        .bind(i64::from(message.from))
        .bind(&message.sender_name)
        .bind(i64::from(message.to))
        .bind(i64::from(message.channel))
        .bind(channel)
        .bind(&message.text)
        .bind(message.at)
        .bind(message.outgoing)
        .bind(status_name(message.status))
        .bind(message.reply_id.map(i64::from))
        .bind(message.emoji)
        .bind(message.snr)
        .bind(message.rssi)
        .bind(message.hops_away.map(i64::from))
        .bind(message.via_mqtt)
        .bind(&message.scope)
        .bind(message.sent_at)
        .bind(message.author.map(i64::from))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if id.rows_affected() == 0 {
            return Ok(None);
        }
        self.message(id.last_insert_rowid()).await
    }

    async fn message(&self, id: MessageId) -> Result<Option<Message>> {
        let row: Option<MessageRow> = sqlx::query_as("SELECT * FROM message_view WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(MessageRow::message))
    }

    /// Takes in the answer to a message sent: `from` is who answered. Returns
    /// the message's new status, if there is such a message.
    pub async fn ack(
        &self,
        radio: RadioId,
        packet_id: u32,
        from: u32,
        error: Option<&str>,
    ) -> Result<Option<(MessageId, MessageStatus, Option<String>)>> {
        let row: Option<(i64, i64, String)> = sqlx::query_as(
            "SELECT id, to_num, status FROM message
            WHERE radio_id = ? AND outgoing = 1 AND packet_id = ?
            ORDER BY id DESC LIMIT 1",
        )
        .bind(radio)
        .bind(i64::from(packet_id))
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, to, status)) = row else {
            return Ok(None);
        };
        let status = match (error, status_from(&status)) {
            // Once delivered, a late answer from a relay changes nothing.
            (_, MessageStatus::Delivered) => MessageStatus::Delivered,
            (Some(_), _) => MessageStatus::Failed,
            (None, _) if i64::from(from) == to => MessageStatus::Delivered,
            (None, _) => MessageStatus::Sent,
        };
        let error = error
            .filter(|_| status == MessageStatus::Failed)
            .map(str::to_owned);
        sqlx::query("UPDATE message SET status = ?, error = ? WHERE id = ?")
            .bind(status_name(status))
            .bind(&error)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some((id, status, error)))
    }

    /// The messages exchanged with a peer, the newest `limit` before
    /// `before`, oldest first.
    /// How far a conversation of a radio was read, and how many messages
    /// others wrote in it after that: one's own and reactions not counted.
    pub async fn read_state(&self, radio: RadioId, peer: Peer) -> Result<(Option<MessageId>, u32)> {
        let (read, unread): (Option<i64>, i64) = match peer {
            Peer::Channel { channel } => {
                sqlx::query_as(
                    "WITH c AS (SELECT channel_id FROM channel_slot WHERE radio_id = ? AND slot = ?),
                    r AS (SELECT message_id FROM conversation_read WHERE channel_id = (SELECT channel_id FROM c))
                    SELECT (SELECT message_id FROM r),
                        (SELECT count(*) FROM message
                            WHERE channel_id = (SELECT channel_id FROM c)
                            AND outgoing = 0 AND emoji = 0
                            AND id > coalesce((SELECT message_id FROM r), 0))",
                )
                .bind(radio)
                .bind(i64::from(channel))
                .fetch_one(&self.pool)
                .await?
            }
            Peer::Node { node } => {
                sqlx::query_as(
                    "WITH r AS (SELECT message_id FROM conversation_read WHERE radio_id = ? AND node = ?)
                    SELECT (SELECT message_id FROM r),
                        (SELECT count(*) FROM message
                            WHERE radio_id = ? AND to_num != ? AND from_num = ?
                            AND outgoing = 0 AND emoji = 0
                            AND id > coalesce((SELECT message_id FROM r), 0))",
                )
                .bind(radio)
                .bind(i64::from(node))
                .bind(radio)
                .bind(i64::from(BROADCAST))
                .bind(i64::from(node))
                .fetch_one(&self.pool)
                .await?
            }
        };
        Ok((read, u32::try_from(unread).unwrap_or_default()))
    }

    /// How many messages others wrote in all the conversations of a radio
    /// that were not read yet: one's own and reactions not counted.
    pub async fn unread(&self, radio: RadioId) -> Result<u32> {
        let (unread,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM message m
            WHERE m.radio_id = ? AND m.outgoing = 0 AND m.emoji = 0
            AND m.id > coalesce(
                CASE WHEN m.to_num = ?
                    THEN (SELECT r.message_id FROM conversation_read r WHERE r.channel_id = m.channel_id)
                    ELSE (SELECT r.message_id FROM conversation_read r
                        WHERE r.radio_id = m.radio_id AND r.node = m.from_num)
                END, 0)",
        )
        .bind(radio)
        .bind(i64::from(BROADCAST))
        .fetch_one(&self.pool)
        .await?;
        Ok(u32::try_from(unread).unwrap_or_default())
    }

    /// Reads a conversation of a radio up to a message, never back: the
    /// newest read now. None for a channel with no slot on the radio.
    pub async fn mark_read(
        &self,
        radio: RadioId,
        peer: Peer,
        up_to: MessageId,
    ) -> Result<Option<MessageId>> {
        match peer {
            Peer::Channel { channel } => {
                let Some((channel_id,)): Option<(i64,)> = sqlx::query_as(
                    "SELECT channel_id FROM channel_slot WHERE radio_id = ? AND slot = ?",
                )
                .bind(radio)
                .bind(i64::from(channel))
                .fetch_optional(&self.pool)
                .await?
                else {
                    return Ok(None);
                };
                sqlx::query(
                    "INSERT INTO conversation_read (radio_id, channel_id, message_id) VALUES (?, ?, ?)
                    ON CONFLICT (channel_id) WHERE channel_id IS NOT NULL
                    DO UPDATE SET message_id = max(message_id, excluded.message_id)",
                )
                .bind(radio)
                .bind(channel_id)
                .bind(up_to)
                .execute(&self.pool)
                .await?;
            }
            Peer::Node { node } => {
                sqlx::query(
                    "INSERT INTO conversation_read (radio_id, node, message_id) VALUES (?, ?, ?)
                    ON CONFLICT (radio_id, node) WHERE node IS NOT NULL
                    DO UPDATE SET message_id = max(message_id, excluded.message_id)",
                )
                .bind(radio)
                .bind(i64::from(node))
                .bind(up_to)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(self.read_state(radio, peer).await?.0)
    }

    /// Forgets a message a radio keeps, and the reactions to it, which are
    /// part of it: only here, not on the mesh. The ids of those forgotten,
    /// the message's first; none if the radio keeps no such message.
    pub async fn forget_message(&self, radio: RadioId, id: MessageId) -> Result<Vec<MessageId>> {
        let mut tx = self.pool.begin().await?;
        let Some((packet,)): Option<(i64,)> =
            sqlx::query_as("SELECT packet_id FROM message WHERE radio_id = ? AND id = ?")
                .bind(radio)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
        else {
            return Ok(Vec::new());
        };
        let reactions: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM message WHERE radio_id = ? AND emoji = 1 AND reply_id = ? AND id != ?",
        )
        .bind(radio)
        .bind(packet)
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let forgotten: Vec<MessageId> = std::iter::once(id)
            .chain(reactions.into_iter().map(|(r,)| r))
            .collect();
        for gone in &forgotten {
            sqlx::query("DELETE FROM message WHERE id = ?")
                .bind(gone)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(forgotten)
    }

    pub async fn messages(
        &self,
        radio: RadioId,
        peer: Peer,
        before: Option<MessageId>,
        limit: u32,
    ) -> Result<Vec<Message>> {
        let before = before.unwrap_or(i64::MAX);
        let rows: Vec<MessageRow> = match peer {
            Peer::Channel { channel } => {
                sqlx::query_as(
                    "SELECT * FROM message_view
                    WHERE channel_id = (
                        SELECT channel_id FROM channel_slot WHERE radio_id = ? AND slot = ?
                    ) AND id < ?
                    ORDER BY id DESC LIMIT ?",
                )
                .bind(radio)
                .bind(i64::from(channel))
                .bind(before)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            Peer::Node { node } => {
                sqlx::query_as(
                    "SELECT * FROM message_view
                    WHERE radio_id = ? AND to_num != ? AND id < ?
                        AND ((outgoing = 1 AND to_num = ?) OR (outgoing = 0 AND from_num = ?))
                    ORDER BY id DESC LIMIT ?",
                )
                .bind(radio)
                .bind(i64::from(BROADCAST))
                .bind(before)
                .bind(i64::from(node))
                .bind(i64::from(node))
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows.into_iter().rev().map(MessageRow::message).collect())
    }

    /// Takes a radio's channels as just read: each slot's conversation is
    /// the channel's, wherever it was before, and through its name or key
    /// being changed where it is. Whether any slot has another now.
    /// Keeps a copy of a message heard, and the way it came; the message it
    /// is, as now kept, if it is.
    pub async fn add_route(
        &self,
        radio: RadioId,
        packet_id: u32,
        route: &RouteRow,
    ) -> Result<Option<Message>> {
        sqlx::query(
            "INSERT INTO message_route
                (radio_id, packet_id, path, hash_size, flooded, snr, rssi, scope, at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(radio)
        .bind(i64::from(packet_id))
        .bind(&route.path)
        .bind(route.hash_size)
        .bind(route.flooded)
        .bind(route.snr)
        .bind(route.rssi)
        .bind(&route.scope)
        .bind(route.at)
        .execute(&self.pool)
        .await?;
        let size = usize::try_from(route.hash_size).unwrap_or(1).max(1);
        for hop in route.path.chunks(size) {
            sqlx::query(
                "INSERT INTO message_relay (radio_id, packet_id, hop, at) VALUES (?, ?, ?, ?)
                ON CONFLICT DO NOTHING",
            )
            .bind(radio)
            .bind(i64::from(packet_id))
            .bind(hop)
            .bind(route.at)
            .execute(&self.pool)
            .await?;
        }
        let row: Option<MessageRow> =
            sqlx::query_as("SELECT * FROM message_view WHERE radio_id = ? AND packet_id = ?")
                .bind(radio)
                .bind(i64::from(packet_id))
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(MessageRow::message))
    }

    /// A radio's message by its packet id, as now kept.
    pub async fn message_by_packet(
        &self,
        radio: RadioId,
        packet_id: u32,
    ) -> Result<Option<Message>> {
        let row: Option<MessageRow> = sqlx::query_as(
            "SELECT * FROM message_view WHERE radio_id = ? AND packet_id = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(radio)
        .bind(i64::from(packet_id))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(MessageRow::message))
    }

    /// The password a radio logs in to a room with, if one is kept.
    pub async fn node_password(&self, radio: RadioId, node: u32) -> Result<Option<String>> {
        use base64::Engine;
        let row: Option<(String,)> =
            sqlx::query_as("SELECT password FROM node_password WHERE radio_id = ? AND node = ?")
                .bind(radio)
                .bind(i64::from(node))
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(kept,)| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(kept)
                .ok()?;
            String::from_utf8(bytes).ok()
        }))
    }

    /// Keeps the password a room took, for the next login; none forgets it.
    pub async fn set_node_password(
        &self,
        radio: RadioId,
        node: u32,
        password: Option<&str>,
    ) -> Result<()> {
        use base64::Engine;
        match password {
            Some(password) => {
                sqlx::query(
                    "INSERT INTO node_password (radio_id, node, password) VALUES (?, ?, ?)
                    ON CONFLICT (radio_id, node) DO UPDATE SET password = excluded.password",
                )
                .bind(radio)
                .bind(i64::from(node))
                .bind(base64::engine::general_purpose::STANDARD.encode(password))
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM node_password WHERE radio_id = ? AND node = ?")
                    .bind(radio)
                    .bind(i64::from(node))
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// A radio's message, as now kept.
    pub async fn radio_message(&self, radio: RadioId, id: MessageId) -> Result<Option<Message>> {
        let row: Option<MessageRow> =
            sqlx::query_as("SELECT * FROM message_view WHERE radio_id = ? AND id = ?")
                .bind(radio)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(MessageRow::message))
    }

    /// The copies of a message heard, the first first: none if there is no
    /// such message.
    pub async fn routes(&self, radio: RadioId, id: MessageId) -> Result<Option<Vec<RouteRow>>> {
        let packet: Option<(i64,)> =
            sqlx::query_as("SELECT packet_id FROM message WHERE radio_id = ? AND id = ?")
                .bind(radio)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let Some((packet,)) = packet else {
            return Ok(None);
        };
        Ok(Some(
            sqlx::query_as(
                "SELECT path, hash_size, flooded, snr, rssi, scope, at FROM message_route
                WHERE radio_id = ? AND packet_id = ? ORDER BY id",
            )
            .bind(radio)
            .bind(packet)
            .fetch_all(&self.pool)
            .await?,
        ))
    }

    /// The region each of a radio's channels floods in, by the channel: as
    /// meshctl keeps it, by its fingerprint.
    pub async fn channel_scopes(&self, radio: RadioId) -> Result<HashMap<Fingerprint, String>> {
        let rows: Vec<(Vec<u8>, String)> =
            sqlx::query_as("SELECT fingerprint, scope FROM channel_scope WHERE radio_id = ?")
                .bind(radio)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(print, scope)| Some((print.try_into().ok()?, scope)))
            .collect())
    }

    /// Keeps the region a channel floods in, by its fingerprint, or none
    /// for the radio's default: for the channel itself, whichever slot it is
    /// in, or before it is in any.
    pub async fn set_channel_scope(
        &self,
        radio: RadioId,
        print: &Fingerprint,
        scope: Option<&str>,
    ) -> Result<()> {
        match scope {
            Some(scope) => {
                sqlx::query(
                    "INSERT INTO channel_scope (radio_id, fingerprint, scope) VALUES (?, ?, ?)
                    ON CONFLICT (radio_id, fingerprint) DO UPDATE SET scope = excluded.scope",
                )
                .bind(radio)
                .bind(print.as_slice())
                .bind(scope)
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM channel_scope WHERE radio_id = ? AND fingerprint = ?")
                    .bind(radio)
                    .bind(print.as_slice())
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn follow_channels(&self, radio: RadioId, now: &[Slot], at: i64) -> Result<bool> {
        // Taken for writing from the start: it reads, then writes, and a
        // transaction that only then asks to write fails at once if another
        // is writing, instead of waiting its turn.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let had: Vec<(i64, i64, Vec<u8>)> = sqlx::query_as(
            "SELECT s.slot, s.channel_id, c.fingerprint
            FROM channel_slot s JOIN channel c ON c.id = s.channel_id
            WHERE s.radio_id = ?",
        )
        .bind(radio)
        .fetch_all(&mut *tx)
        .await?;
        let had: Vec<(u32, i64, Fingerprint)> = had
            .into_iter()
            .filter_map(|(slot, id, print)| {
                Some((u32::try_from(slot).ok()?, id, print.try_into().ok()?))
            })
            .collect();
        let known: Vec<(Vec<u8>, i64)> =
            sqlx::query_as("SELECT fingerprint, id FROM channel WHERE radio_id = ?")
                .bind(radio)
                .fetch_all(&mut *tx)
                .await?;
        let known: HashMap<Fingerprint, i64> = known
            .into_iter()
            .filter_map(|(print, id)| Some((print.try_into().ok()?, id)))
            .collect();

        let mut ids: Vec<i64> = Vec::with_capacity(now.len());
        for (slot, chat) in now.iter().zip(assign(&had, now, &known)) {
            let id = match chat {
                Chat::Known(id) => {
                    sqlx::query("UPDATE channel SET name = ?, seen_at = ? WHERE id = ?")
                        .bind(&slot.name)
                        .bind(at)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    id
                }
                Chat::Rekeyed(id) => {
                    sqlx::query(
                        "UPDATE channel SET fingerprint = ?, name = ?, seen_at = ? WHERE id = ?",
                    )
                    .bind(&slot.fingerprint[..])
                    .bind(&slot.name)
                    .bind(at)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    id
                }
                Chat::SameAs(earlier) => ids[earlier],
                Chat::New => {
                    sqlx::query_scalar(
                        "INSERT INTO channel (radio_id, fingerprint, name, seen_at)
                    VALUES (?, ?, ?, ?) RETURNING id",
                    )
                    .bind(radio)
                    .bind(&slot.fingerprint[..])
                    .bind(&slot.name)
                    .bind(at)
                    .fetch_one(&mut *tx)
                    .await?
                }
            };
            ids.push(id);
        }

        sqlx::query("DELETE FROM channel_slot WHERE radio_id = ?")
            .bind(radio)
            .execute(&mut *tx)
            .await?;
        for (slot, id) in now.iter().zip(&ids) {
            sqlx::query("INSERT INTO channel_slot (radio_id, slot, channel_id) VALUES (?, ?, ?)")
                .bind(radio)
                .bind(i64::from(slot.index))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        let mut before: Vec<(u32, i64)> = had.iter().map(|(slot, id, _)| (*slot, *id)).collect();
        let mut after: Vec<(u32, i64)> = now.iter().map(|s| s.index).zip(ids).collect();
        before.sort_unstable();
        after.sort_unstable();
        Ok(before != after)
    }

    /// Forgets the conversation of the channel in a slot, with its messages;
    /// not one another slot has too, the very same channel twice.
    pub async fn forget_channel(&self, radio: RadioId, slot: u32) -> Result<()> {
        sqlx::query(
            "DELETE FROM channel WHERE id = (
                SELECT channel_id FROM channel_slot WHERE radio_id = ? AND slot = ?
            ) AND (SELECT count(*) FROM channel_slot WHERE channel_id = channel.id) = 1",
        )
        .bind(radio)
        .bind(i64::from(slot))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every conversation that has messages, with the newest one in it.
    pub async fn conversations(&self, radio: RadioId) -> Result<Vec<Conversation>> {
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT * FROM message_view WHERE id IN (
                SELECT max(m.id) FROM message m
                JOIN channel_slot s ON s.channel_id = m.channel_id
                WHERE s.radio_id = ?
                GROUP BY m.channel_id
                UNION ALL
                SELECT max(id) FROM message WHERE radio_id = ? AND to_num != ?
                GROUP BY CASE WHEN outgoing = 1 THEN to_num ELSE from_num END
            ) ORDER BY id DESC",
        )
        .bind(radio)
        .bind(radio)
        .bind(i64::from(BROADCAST))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(MessageRow::message)
            .map(|last| Conversation {
                peer: peer_of(&last),
                last: Some(last),
                unread: 0,
                read_up_to: None,
            })
            .collect())
    }

    /// Keeps a traceroute just sent.
    pub async fn add_traceroute(
        &self,
        radio: RadioId,
        packet_id: u32,
        origin: u32,
        target: u32,
        at: i64,
    ) -> Result<Traceroute> {
        let id = sqlx::query(
            "INSERT INTO traceroute (radio_id, packet_id, origin, target, at, status)
            VALUES (?, ?, ?, ?, ?, 'pending')",
        )
        .bind(radio)
        .bind(i64::from(packet_id))
        .bind(i64::from(origin))
        .bind(i64::from(target))
        .bind(at)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        self.traceroute(id)
            .await?
            .context("a traceroute just kept is not there")
    }

    async fn traceroute(&self, id: TracerouteId) -> Result<Option<Traceroute>> {
        let row: Option<TracerouteRow> = sqlx::query_as("SELECT * FROM traceroute WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(TracerouteRow::traceroute))
    }

    /// The traceroute still waiting for the answer to a packet: its id, and
    /// the two ends of the route.
    pub async fn pending_traceroute(
        &self,
        radio: RadioId,
        packet_id: u32,
    ) -> Result<Option<(TracerouteId, u32, u32)>> {
        let row: Option<(i64, i64, i64)> = sqlx::query_as(
            "SELECT id, origin, target FROM traceroute
            WHERE radio_id = ? AND packet_id = ? AND status = 'pending'",
        )
        .bind(radio)
        .bind(i64::from(packet_id))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, origin, target)| (id, u32_from(origin), u32_from(target))))
    }

    /// Settles a pending traceroute: answered, with its routes, or failed.
    pub async fn settle_traceroute(
        &self,
        id: TracerouteId,
        outcome: Result<(&[Hop], &[Hop]), &str>,
    ) -> Result<Option<Traceroute>> {
        let (status, error, towards, back) = match outcome {
            Ok((towards, back)) => (
                TracerouteStatus::Done,
                None,
                Some(serde_json::to_string(towards)?),
                Some(serde_json::to_string(back)?),
            ),
            Err(error) => (TracerouteStatus::Failed, Some(error), None, None),
        };
        sqlx::query(
            "UPDATE traceroute SET status = ?, error = ?, towards = ?, back = ?
            WHERE id = ? AND status = 'pending'",
        )
        .bind(traceroute_status_name(status))
        .bind(error)
        .bind(towards)
        .bind(back)
        .bind(id)
        .execute(&self.pool)
        .await?;
        self.traceroute(id).await
    }

    /// Gives up on the traceroutes sent before `before` that were never
    /// answered, and returns them.
    pub async fn expire_traceroutes(&self, radio: RadioId, before: i64) -> Result<Vec<Traceroute>> {
        let rows: Vec<TracerouteRow> = sqlx::query_as(
            "UPDATE traceroute SET status = 'failed', error = 'TIMEOUT'
            WHERE radio_id = ? AND status = 'pending' AND at < ?
            RETURNING *",
        )
        .bind(radio)
        .bind(before)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(TracerouteRow::traceroute).collect())
    }

    /// A radio's traceroutes, to one node or to any, the newest first.
    pub async fn traceroutes(
        &self,
        radio: RadioId,
        node: Option<u32>,
        before: Option<TracerouteId>,
        limit: u32,
    ) -> Result<Vec<Traceroute>> {
        let rows: Vec<TracerouteRow> = sqlx::query_as(
            "SELECT * FROM traceroute
            WHERE radio_id = ? AND (? IS NULL OR target = ?) AND (? IS NULL OR id < ?)
            ORDER BY id DESC LIMIT ?",
        )
        .bind(radio)
        .bind(node.map(i64::from))
        .bind(node.map(i64::from))
        .bind(before)
        .bind(before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(TracerouteRow::traceroute).collect())
    }

    /// Forgets the routes traced from a radio to a node but the latest, which
    /// its page shows, and those still waiting; how many were.
    pub async fn forget_traceroutes(&self, radio: RadioId, target: u32) -> Result<u64> {
        Ok(sqlx::query(
            "DELETE FROM traceroute WHERE radio_id = ? AND target = ? AND status != 'pending'
                AND id < (SELECT max(id) FROM traceroute WHERE radio_id = ? AND target = ?)",
        )
        .bind(radio)
        .bind(i64::from(target))
        .bind(radio)
        .bind(i64::from(target))
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// When the radio last sent a traceroute, if ever.
    pub async fn last_traceroute_at(&self, radio: RadioId) -> Result<Option<i64>> {
        Ok(
            sqlx::query_scalar("SELECT max(at) FROM traceroute WHERE radio_id = ?")
                .bind(radio)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

#[derive(sqlx::FromRow)]
struct TracerouteRow {
    id: i64,
    target: i64,
    at: i64,
    status: String,
    error: Option<String>,
    towards: Option<String>,
    back: Option<String>,
}

impl TracerouteRow {
    fn traceroute(self) -> Traceroute {
        let hops = |json: Option<String>| -> Vec<Hop> {
            json.and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default()
        };
        Traceroute {
            id: self.id,
            target: u32_from(self.target),
            at: self.at,
            status: match self.status.as_str() {
                "done" => TracerouteStatus::Done,
                "failed" => TracerouteStatus::Failed,
                _ => TracerouteStatus::Pending,
            },
            error: self.error,
            towards: hops(self.towards),
            back: hops(self.back),
        }
    }
}

fn traceroute_status_name(status: TracerouteStatus) -> &'static str {
    match status {
        TracerouteStatus::Pending => "pending",
        TracerouteStatus::Done => "done",
        TracerouteStatus::Failed => "failed",
    }
}

/// Who a message was exchanged with.
pub fn peer_of(message: &Message) -> Peer {
    if message.to == BROADCAST {
        Peer::Channel {
            channel: message.channel,
        }
    } else if message.outgoing {
        Peer::Node { node: message.to }
    } else {
        Peer::Node { node: message.from }
    }
}

/// A message to keep, as heard or as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMessageRow {
    pub packet_id: u32,
    pub from: u32,
    pub sender_name: Option<String>,
    pub to: u32,
    pub channel: u32,
    pub text: String,
    pub at: i64,
    pub outgoing: bool,
    pub status: MessageStatus,
    pub reply_id: Option<u32>,
    pub emoji: bool,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub hops_away: Option<u32>,
    pub via_mqtt: bool,
    /// The region it went flooded in.
    pub scope: Option<String>,
    /// When its sender says it sent it, by its clock.
    pub sent_at: Option<i64>,
    /// Who wrote it, for a room's post: its author's node.
    pub author: Option<u32>,
}

/// A packet as kept, with one radio's hearing of it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PacketRow {
    /// The hearing's id.
    pub id: SightingId,
    pub packet_id: i64,
    pub from_num: i64,
    pub to_num: i64,
    pub channel: i64,
    pub portnum: Option<i32>,
    pub payload: Vec<u8>,
    pub request_id: Option<i64>,
    pub reply_id: Option<i64>,
    pub want_response: bool,
    pub pki: bool,
    pub at: i64,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub hop_limit: i64,
    pub hop_start: i64,
    pub relay_node: Option<i64>,
    pub via_mqtt: bool,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: i64,
    packet_id: i64,
    from_num: i64,
    sender_name: Option<String>,
    to_num: i64,
    channel: i64,
    text: String,
    at: i64,
    outgoing: bool,
    status: String,
    error: Option<String>,
    reply_id: Option<i64>,
    emoji: bool,
    snr: Option<f32>,
    rssi: Option<i32>,
    hops_away: Option<i64>,
    via_mqtt: bool,
    scope: Option<String>,
    heard: i64,
    sent_at: Option<i64>,
    repeaters: i64,
    author_num: Option<i64>,
}

impl MessageRow {
    fn message(self) -> Message {
        Message {
            id: self.id,
            packet_id: u32_from(self.packet_id),
            from: u32_from(self.from_num),
            sender_name: self.sender_name,
            to: u32_from(self.to_num),
            channel: u32_from(self.channel),
            text: self.text,
            at: self.at,
            outgoing: self.outgoing,
            status: status_from(&self.status),
            error: self.error,
            reply_id: self.reply_id.map(u32_from),
            emoji: self.emoji,
            snr: self.snr,
            rssi: self.rssi,
            hops_away: self.hops_away.map(u32_from),
            via_mqtt: self.via_mqtt,
            scope: self.scope,
            heard: u32::try_from(self.heard).unwrap_or(0),
            sent_at: self.sent_at,
            repeaters: u32::try_from(self.repeaters).unwrap_or(0),
            author: self.author_num.map(u32_from),
        }
    }
}

/// A copy of a message heard, as kept.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RouteRow {
    pub path: Vec<u8>,
    pub hash_size: i64,
    pub flooded: bool,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub scope: Option<String>,
    pub at: i64,
}

fn status_name(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Received => "received",
        MessageStatus::Sending => "sending",
        MessageStatus::Sent => "sent",
        MessageStatus::Delivered => "delivered",
        MessageStatus::Failed => "failed",
    }
}

fn status_from(name: &str) -> MessageStatus {
    match name {
        "sending" => MessageStatus::Sending,
        "sent" => MessageStatus::Sent,
        "delivered" => MessageStatus::Delivered,
        "failed" => MessageStatus::Failed,
        _ => MessageStatus::Received,
    }
}

/// The conversation of the channel in a slot: if the slot is not known yet,
/// one of its own, known by the slot alone until the radio says which
/// channel it has.
async fn chat_of_slot(
    tx: &mut SqliteConnection,
    radio: RadioId,
    slot: u32,
    at: i64,
) -> Result<i64> {
    let known: Option<i64> =
        sqlx::query_scalar("SELECT channel_id FROM channel_slot WHERE radio_id = ? AND slot = ?")
            .bind(radio)
            .bind(i64::from(slot))
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(id) = known {
        return Ok(id);
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO channel (radio_id, fingerprint, name, seen_at) VALUES (?, ?, '', ?)
        ON CONFLICT (radio_id, fingerprint) DO UPDATE SET seen_at = excluded.seen_at
        RETURNING id",
    )
    .bind(radio)
    .bind(&provisional(slot)[..])
    .bind(at)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO channel_slot (radio_id, slot, channel_id) VALUES (?, ?, ?)")
        .bind(radio)
        .bind(i64::from(slot))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    Ok(id)
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// How many emojis' uses are kept: the most used, beyond those shown.
const EMOJIS_KEPT: usize = 64;

/// How often an emoji was picked, and when last.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct EmojiUse {
    emoji: String,
    count: u32,
    last: i64,
}

/// The most used first; among as used, the latest.
fn sort_uses(uses: &mut [EmojiUse]) {
    uses.sort_by(|a, b| b.count.cmp(&a.count).then(b.last.cmp(&a.last)));
}

fn ranked(mut uses: Vec<EmojiUse>) -> Vec<String> {
    sort_uses(&mut uses);
    uses.into_iter().map(|u| u.emoji).collect()
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
            shown: true,
            badge: Default::default(),
        }
    }

    #[tokio::test]
    async fn emojis_rank_by_use_then_by_the_latest() {
        let store = Store::in_memory().await;
        assert!(store.emojis().await.unwrap().is_empty());
        store.use_emoji("👍", 10).await.unwrap();
        store.use_emoji("❤️", 20).await.unwrap();
        store.use_emoji("👍", 30).await.unwrap();
        store.use_emoji("😂", 40).await.unwrap();
        // 👍 twice; ❤️ and 😂 once, 😂 the latest.
        assert_eq!(store.emojis().await.unwrap(), ["👍", "😂", "❤️"]);
        // Kept apart from the interface's preferences.
        assert_eq!(store.preferences().await.unwrap(), Preferences::default());
    }

    #[tokio::test]
    async fn preferences_are_kept() {
        let store = Store::in_memory().await;
        assert_eq!(store.preferences().await.unwrap(), Preferences::default());

        let dark = Preferences {
            theme: meshctl_api::Theme::Dark,
            language: Some(meshctl_api::Language::Es),
            ..Default::default()
        };
        store.set_preferences(&dark).await.unwrap();
        store.set_preferences(&dark).await.unwrap();
        assert_eq!(store.preferences().await.unwrap(), dark);

        // A newer meshctl's, which this one cannot read.
        sqlx::query("UPDATE preference SET value = '{\"theme\":\"sepia\"}'")
            .execute(&store.pool)
            .await
            .unwrap();
        assert_eq!(store.preferences().await.unwrap(), Preferences::default());
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

        // How its button looks, and whether it is shown, are kept too.
        changed.shown = false;
        changed.badge = Badge {
            color: Some("#f0a63a".into()),
            content: meshctl_api::BadgeContent::Emoji {
                emoji: "📡".into()
            },
        };
        assert!(store.update_radio(added.id, &changed).await.unwrap());
        let kept = store.radio(added.id).await.unwrap().unwrap();
        assert_eq!(kept.settings, changed);
        // Back to the button every radio has, nothing is kept of it.
        changed.badge = Badge::default();
        assert!(store.update_radio(added.id, &changed).await.unwrap());
        let badge: Option<String> = sqlx::query_scalar("SELECT badge FROM radio WHERE id = ?")
            .bind(added.id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(badge.is_none());

        // Where its map was left, from never to somewhere.
        assert_eq!(store.map_view(added.id).await.unwrap(), Some(None));
        let view = MapView {
            latitude: 43.36,
            longitude: -8.41,
            zoom: 11.0,
        };
        assert!(store.set_map_view(added.id, &view).await.unwrap());
        assert_eq!(store.map_view(added.id).await.unwrap(), Some(Some(view)));
        assert!(!store.set_map_view(999, &view).await.unwrap());
        assert_eq!(store.map_view(999).await.unwrap(), None);

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
                public_key: vec![7; 32],
                licensed: false,
                unmessagable: true,
            }),
            position: Some(Position {
                latitude: 43.36,
                longitude: -8.41,
                altitude: Some(12),
                radius: Some(2918),
                source: Some(PositionSource::Manual),
            }),
            last_heard: Some(1_795_000_000),
            snr: Some(6.5),
            rssi: Some(-90),
            hops_away: Some(2),
            via_mqtt: false,
            favorite: true,
            battery_level: Some(101),
            voltage: Some(4.2),
            channel_utilization: Some(12.5),
            on_radio: None,
            status: None,
            status_at: None,
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
    async fn a_new_node_behind_the_radio_keeps_what_the_old_one_knew() {
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
        let mut nums: Vec<_> = store
            .nodes(radio)
            .await
            .unwrap()
            .iter()
            .map(|n| n.num)
            .collect();
        nums.sort_unstable();
        assert_eq!(nums, [1, 2]);
        assert_eq!(store.local(radio).await.unwrap().unwrap().num, 8);
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
    async fn forgetting_a_node_leaves_its_channel_messages_and_the_rest() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        store.save_node(radio, &node(9, "Faro")).await.unwrap();
        store.save_node(radio, &node(5, "Otro")).await.unwrap();
        // Its conversation with the radio (7), and a message on a channel.
        store
            .add_message(radio, &text(1, 9, 7, false))
            .await
            .unwrap();
        store
            .add_message(radio, &text(2, 7, 9, true))
            .await
            .unwrap();
        store
            .add_message(radio, &text(3, 9, BROADCAST, false))
            .await
            .unwrap();
        store
            .add_message(radio, &text(4, 5, 7, false))
            .await
            .unwrap();
        // What it was asked, the routes traced to it, and what it sent.
        store
            .add_request(radio, 9, meshctl_api::RequestKind::Position, 0, 10, 100)
            .await
            .unwrap();
        store.add_traceroute(radio, 11, 7, 9, 100).await.unwrap();
        store.add_traceroute(radio, 12, 7, 5, 100).await.unwrap();
        let heard = |packet_id: u32, from: u32| Heard {
            packet_id,
            from,
            to: BROADCAST,
            channel: 0,
            portnum: Some(67),
            payload: Vec::new(),
            request_id: None,
            reply_id: None,
            want_response: false,
            pki: false,
            at: 100,
            snr: None,
            rssi: None,
            hop_limit: 3,
            hop_start: 3,
            relay_node: None,
            via_mqtt: false,
        };
        store.add_packet(radio, &heard(20, 9)).await.unwrap();
        store.add_packet(radio, &heard(21, 5)).await.unwrap();

        store.remove_node(radio, 9).await.unwrap();
        store.forget_node(radio, 9).await.unwrap();

        let count = |sql: &'static str| {
            let pool = store.pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(sql)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(count("SELECT count(*) FROM node WHERE num = 9").await, 0);
        assert_eq!(count("SELECT count(*) FROM node").await, 1);
        // Its channel message and the other node's stay.
        assert_eq!(count("SELECT count(*) FROM message").await, 2);
        assert_eq!(count("SELECT count(*) FROM request").await, 0);
        assert_eq!(count("SELECT count(*) FROM traceroute").await, 1);
        assert_eq!(count("SELECT count(*) FROM packet").await, 1);
        assert_eq!(count("SELECT count(*) FROM packet_seen").await, 1);
    }

    #[tokio::test]
    async fn a_messages_routes_are_kept_whether_it_came_first_or_not() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let route = |path: Vec<u8>, scope: Option<&str>| RouteRow {
            path,
            hash_size: 2,
            flooded: true,
            snr: Some(6.0),
            rssi: Some(-92),
            scope: scope.map(str::to_owned),
            at: 10,
        };
        // A copy before its message: nothing to tell of yet.
        assert!(
            store
                .add_route(
                    radio,
                    9,
                    &route(vec![0x52, 0x59, 0x55, 0x5c], Some("galicia"))
                )
                .await
                .unwrap()
                .is_none()
        );
        let kept = store
            .add_message(radio, &text(9, 5, BROADCAST, false))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((kept.heard, kept.scope.as_deref()), (1, Some("galicia")));
        // Another, after: the message as it is now.
        let told = store
            .add_route(radio, 9, &route(vec![0x55, 0x5c], None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((told.id, told.heard), (kept.id, 2));
        // Two copies, two repeaters: one of them on both.
        assert_eq!(told.repeaters, 2);
        let routes = store.routes(radio, kept.id).await.unwrap().unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].path, [0x52, 0x59, 0x55, 0x5c]);
        assert_eq!(store.routes(radio, kept.id + 1).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_channels_region_is_kept_by_the_channel() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let galicia = crate::channels::fingerprint("galicia", &[1; 16]);
        let casa = crate::channels::fingerprint("Casa", &[2; 16]);
        store
            .set_channel_scope(radio, &galicia, Some("galicia"))
            .await
            .unwrap();
        store
            .set_channel_scope(radio, &casa, Some("*"))
            .await
            .unwrap();
        // Changed, then taken away: the radio's default again.
        store
            .set_channel_scope(radio, &galicia, Some("europe"))
            .await
            .unwrap();
        store.set_channel_scope(radio, &casa, None).await.unwrap();
        let scopes = store.channel_scopes(radio).await.unwrap();
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes.get(&galicia).map(String::as_str), Some("europe"));
        // No channel is made of it: no conversation either.
        let channels: i64 = sqlx::query_scalar("SELECT count(*) FROM channel")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(channels, 0);
    }

    fn text(packet_id: u32, from: u32, to: u32, outgoing: bool) -> NewMessageRow {
        NewMessageRow {
            packet_id,
            from,
            sender_name: None,
            to,
            channel: 0,
            text: format!("#{packet_id}"),
            at: i64::from(packet_id),
            outgoing,
            status: if outgoing {
                MessageStatus::Sending
            } else {
                MessageStatus::Received
            },
            reply_id: None,
            emoji: false,
            snr: None,
            rssi: None,
            hops_away: None,
            via_mqtt: false,
            scope: None,
            sent_at: None,
            author: None,
        }
    }

    #[tokio::test]
    async fn conversations_are_read_on_and_never_back() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let kept = |row: NewMessageRow| {
            let store = &store;
            async move { store.add_message(radio, &row).await.unwrap().unwrap().id }
        };
        let channel = Peer::Channel { channel: 0 };
        let node = Peer::Node { node: 9 };
        // On the channel: two from others, one ours, one reaction.
        let first = kept(text(1, 9, BROADCAST, false)).await;
        kept(text(2, 5, BROADCAST, false)).await;
        kept(text(3, 7, BROADCAST, true)).await;
        kept(NewMessageRow {
            emoji: true,
            reply_id: Some(3),
            ..text(4, 9, BROADCAST, false)
        })
        .await;
        // From the node, directly: one.
        let direct = kept(text(5, 9, 7, false)).await;

        assert_eq!(store.read_state(radio, channel).await.unwrap(), (None, 2));
        assert_eq!(store.read_state(radio, node).await.unwrap(), (None, 1));
        assert_eq!(
            store.mark_read(radio, channel, first).await.unwrap(),
            Some(first)
        );
        assert_eq!(
            store.read_state(radio, channel).await.unwrap(),
            (Some(first), 1)
        );
        // Not back.
        assert_eq!(
            store.mark_read(radio, channel, first - 1).await.unwrap(),
            Some(first)
        );
        assert_eq!(
            store.mark_read(radio, node, direct).await.unwrap(),
            Some(direct)
        );
        assert_eq!(
            store.read_state(radio, node).await.unwrap(),
            (Some(direct), 0)
        );
        // The channel is unread still, each conversation on its own; and
        // the radio, all of them.
        assert_eq!(store.read_state(radio, channel).await.unwrap().1, 1);
        assert_eq!(store.unread(radio).await.unwrap(), 1);
        // A slot with no channel cannot be read.
        assert_eq!(
            store
                .mark_read(radio, Peer::Channel { channel: 5 }, first)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_message_forgotten_takes_its_reactions_and_nothing_else() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let kept = |row: &NewMessageRow| {
            let row = row.clone();
            let store = &store;
            async move { store.add_message(radio, &row).await.unwrap().unwrap().id }
        };
        let message = kept(&text(1, 9, BROADCAST, false)).await;
        let other = kept(&text(2, 9, BROADCAST, false)).await;
        let reaction = |packet_id: u32, to: u32| NewMessageRow {
            text: "👍".into(),
            reply_id: Some(to),
            emoji: true,
            ..text(packet_id, 5, BROADCAST, false)
        };
        let to_it = kept(&reaction(3, 1)).await;
        let to_other = kept(&reaction(4, 2)).await;
        // A reply to it is a message of its own: it stays.
        let reply = kept(&NewMessageRow {
            reply_id: Some(1),
            ..text(5, 5, BROADCAST, false)
        })
        .await;

        assert_eq!(
            store.forget_message(radio, message).await.unwrap(),
            [message, to_it]
        );
        assert!(
            store
                .forget_message(radio, message)
                .await
                .unwrap()
                .is_empty()
        );
        let left: Vec<MessageId> = store
            .messages(radio, Peer::Channel { channel: 0 }, None, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(left, [other, to_other, reply]);
    }

    #[tokio::test]
    async fn a_message_replayed_is_kept_once() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let heard = text(1, 9, BROADCAST, false);
        assert!(store.add_message(radio, &heard).await.unwrap().is_some());
        assert!(store.add_message(radio, &heard).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn answers_move_a_message_along() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        store
            .add_message(radio, &text(1, 7, 9, true))
            .await
            .unwrap();
        store
            .add_message(radio, &text(2, 7, 9, true))
            .await
            .unwrap();

        // Heard relayed by the radio itself, then acknowledged by node 9.
        let (_, status, _) = store.ack(radio, 1, 7, None).await.unwrap().unwrap();
        assert_eq!(status, MessageStatus::Sent);
        let (_, status, _) = store.ack(radio, 1, 9, None).await.unwrap().unwrap();
        assert_eq!(status, MessageStatus::Delivered);
        // A late answer does not take it back.
        let (_, status, _) = store
            .ack(radio, 1, 7, Some("MAX_RETRANSMIT"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, MessageStatus::Delivered);

        let (_, status, error) = store
            .ack(radio, 2, 7, Some("MAX_RETRANSMIT"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, MessageStatus::Failed);
        assert_eq!(error.as_deref(), Some("MAX_RETRANSMIT"));

        assert!(store.ack(radio, 3, 7, None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn messages_gather_by_who_they_were_exchanged_with() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        for row in [
            text(1, 9, BROADCAST, false),
            text(2, 9, 7, false),
            text(3, 7, 9, true),
            text(4, 8, 7, false),
            text(5, 8, BROADCAST, false),
        ] {
            store.add_message(radio, &row).await.unwrap();
        }

        let with_nine = store
            .messages(radio, Peer::Node { node: 9 }, None, 50)
            .await
            .unwrap();
        let ids: Vec<_> = with_nine.iter().map(|m| m.packet_id).collect();
        assert_eq!(ids, [2, 3]);

        let channel = store
            .messages(radio, Peer::Channel { channel: 0 }, None, 1)
            .await
            .unwrap();
        assert_eq!(channel[0].packet_id, 5, "the newest page");
        let before = store
            .messages(radio, Peer::Channel { channel: 0 }, Some(channel[0].id), 50)
            .await
            .unwrap();
        assert_eq!(before[0].packet_id, 1);

        let conversations = store.conversations(radio).await.unwrap();
        let peers: Vec<_> = conversations
            .iter()
            .map(|c| (c.peer, c.last.as_ref().unwrap().packet_id))
            .collect();
        assert_eq!(
            peers,
            [
                (Peer::Channel { channel: 0 }, 5),
                (Peer::Node { node: 8 }, 4),
                (Peer::Node { node: 9 }, 3),
            ]
        );
    }

    #[tokio::test]
    async fn what_was_left_behind_is_told_and_forgotten() {
        use crate::channels::fingerprint;
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let slot = |index: u32, name: &str| Slot {
            index,
            fingerprint: fingerprint(name, &[1]),
            name: name.into(),
        };
        let on = |packet_id: u32, channel: u32| NewMessageRow {
            channel,
            ..text(packet_id, 9, BROADCAST, false)
        };
        // A and B, then B taken off the radio, with two messages left.
        let both = [slot(0, "A"), slot(1, "B")];
        assert!(store.follow_channels(radio, &both, 10).await.unwrap());
        for row in [on(1, 0), on(2, 1), on(3, 1)] {
            store.add_message(radio, &row).await.unwrap();
        }
        assert!(store.follow_channels(radio, &both[..1], 20).await.unwrap());
        // With 5, kept, and with 6, gone: one each way.
        store.save_node(radio, &node(5, "Faro")).await.unwrap();
        for row in [
            text(4, 5, 1, false),
            text(5, 6, 1, false),
            text(6, 1, 6, true),
        ] {
            store.add_message(radio, &row).await.unwrap();
        }

        let storage = store.storage().await.unwrap();
        assert_eq!(storage.messages, 6);
        assert_eq!(
            (
                storage.gone_channels.conversations,
                storage.gone_channels.messages
            ),
            (1, 2)
        );
        assert_eq!(
            (
                storage.gone_directs.conversations,
                storage.gone_directs.messages
            ),
            (1, 2)
        );
        assert_eq!(store.forget_gone_channels().await.unwrap(), 2);
        assert_eq!(store.forget_gone_directs().await.unwrap(), 2);
        let storage = store.storage().await.unwrap();
        assert_eq!(storage.messages, 2);
        assert_eq!(storage.gone_channels, meshctl_api::Gone::default());
        assert_eq!(storage.gone_directs, meshctl_api::Gone::default());
        // Old messages, by when.
        assert_eq!(store.prune_messages(2).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn a_channel_is_one_conversation_wherever_it_is() {
        use crate::channels::fingerprint;
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let slot = |index: u32, name: &str| Slot {
            index,
            fingerprint: fingerprint(name, &[1]),
            name: name.into(),
        };
        let on = |packet_id: u32, channel: u32| NewMessageRow {
            channel,
            ..text(packet_id, 9, BROADCAST, false)
        };
        let in_channel = |channel: u32| {
            let store = store.clone();
            async move {
                store
                    .messages(radio, Peer::Channel { channel }, None, 50)
                    .await
                    .unwrap()
                    .iter()
                    .map(|m| (m.packet_id, m.channel))
                    .collect::<Vec<_>>()
            }
        };
        // Heard before the radio says which channels it has: known by slot.
        store.add_message(radio, &on(1, 1)).await.unwrap();
        let first = [slot(0, "A"), slot(1, "B"), slot(2, "C")];
        assert!(store.follow_channels(radio, &first, 10).await.unwrap());
        for row in [on(2, 0), on(3, 2)] {
            store.add_message(radio, &row).await.unwrap();
        }
        assert_eq!(in_channel(1).await, [(1, 1)]);

        // B and C swap: each conversation goes with its channel, and its
        // messages say the slot it has now.
        let swapped = [slot(0, "A"), slot(1, "C"), slot(2, "B")];
        assert!(store.follow_channels(radio, &swapped, 20).await.unwrap());
        assert_eq!(in_channel(1).await, [(3, 1)]);
        assert_eq!(in_channel(2).await, [(1, 2)]);

        // B renamed where it is: still its conversation.
        let renamed = [slot(0, "A"), slot(1, "C"), slot(2, "Be")];
        store.follow_channels(radio, &renamed, 30).await.unwrap();
        assert_eq!(in_channel(2).await, [(1, 2)]);

        // Gone: in no conversation; and back elsewhere, with its messages.
        let without = [slot(0, "A"), slot(1, "C")];
        store.follow_channels(radio, &without, 40).await.unwrap();
        assert_eq!(store.conversations(radio).await.unwrap().len(), 2);
        let back = [slot(0, "A"), slot(1, "C"), slot(5, "Be")];
        store.follow_channels(radio, &back, 50).await.unwrap();
        assert_eq!(in_channel(5).await, [(1, 5)]);

        // Twice the very same: one conversation, in both slots.
        let twice = [slot(0, "A"), slot(1, "C"), slot(5, "Be"), slot(6, "A")];
        store.follow_channels(radio, &twice, 60).await.unwrap();
        assert_eq!(in_channel(6).await, in_channel(0).await);
        // Forgetting one of them forgets nothing; the other, all of it.
        store.forget_channel(radio, 6).await.unwrap();
        store.follow_channels(radio, &back, 70).await.unwrap();
        assert_eq!(in_channel(0).await, [(2, 0)]);
        store.forget_channel(radio, 5).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM message")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(left, 2);
    }

    #[tokio::test]
    async fn a_traceroute_is_answered_failed_or_given_up_on() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        let first = store.add_traceroute(radio, 41, 7, 9, 100).await.unwrap();
        let second = store.add_traceroute(radio, 42, 7, 9, 130).await.unwrap();
        let third = store.add_traceroute(radio, 43, 7, 8, 160).await.unwrap();
        assert_eq!(first.status, TracerouteStatus::Pending);
        assert_eq!(store.last_traceroute_at(radio).await.unwrap(), Some(160));

        let (id, origin, target) = store.pending_traceroute(radio, 41).await.unwrap().unwrap();
        assert_eq!((id, origin, target), (first.id, 7, 9));
        let hops = [
            Hop {
                node: Some(7),
                snr: None,
            },
            Hop {
                node: Some(9),
                snr: Some(6.0),
            },
        ];
        let done = store
            .settle_traceroute(id, Ok((&hops, &hops)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(done.status, TracerouteStatus::Done);
        assert_eq!(done.towards, hops);
        // Settled once: no longer waiting for anything.
        assert!(store.pending_traceroute(radio, 41).await.unwrap().is_none());

        let failed = store
            .settle_traceroute(second.id, Err("NO_RESPONSE"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.status, TracerouteStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("NO_RESPONSE"));

        let expired = store.expire_traceroutes(radio, 200).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, third.id);
        assert_eq!(expired[0].error.as_deref(), Some("TIMEOUT"));

        let to_nine = store.traceroutes(radio, Some(9), None, 50).await.unwrap();
        let ids: Vec<_> = to_nine.iter().map(|t| t.id).collect();
        assert_eq!(ids, [second.id, first.id]);
        assert_eq!(
            store
                .traceroutes(radio, None, None, 50)
                .await
                .unwrap()
                .len(),
            3
        );
        // A page of them, before one: the older ones.
        let older = store
            .traceroutes(radio, Some(9), Some(second.id), 50)
            .await
            .unwrap();
        assert_eq!(older.iter().map(|t| t.id).collect::<Vec<_>>(), [first.id]);

        // Forgotten, but the latest, which the node's page shows; and those
        // still waiting.
        let waiting = store.add_traceroute(radio, 44, 7, 8, 300).await.unwrap();
        assert_eq!(store.forget_traceroutes(radio, 9).await.unwrap(), 1);
        let left = store.traceroutes(radio, Some(9), None, 50).await.unwrap();
        assert_eq!(left.iter().map(|t| t.id).collect::<Vec<_>>(), [second.id]);
        assert_eq!(store.forget_traceroutes(radio, 8).await.unwrap(), 1);
        let left = store.traceroutes(radio, Some(8), None, 50).await.unwrap();
        assert_eq!(left.iter().map(|t| t.id).collect::<Vec<_>>(), [waiting.id]);
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

    #[tokio::test]
    async fn a_room_password_is_kept_not_in_plain_sight() {
        let store = Store::in_memory().await;
        let radio = store.add_radio(&home()).await.unwrap().id;
        assert_eq!(store.node_password(radio, 7).await.unwrap(), None);
        store
            .set_node_password(radio, 7, Some("sésamo"))
            .await
            .unwrap();
        let (kept,): (String,) = sqlx::query_as("SELECT password FROM node_password")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_ne!(kept, "sésamo");
        assert_eq!(
            store.node_password(radio, 7).await.unwrap().as_deref(),
            Some("sésamo")
        );
        store.set_node_password(radio, 7, None).await.unwrap();
        assert_eq!(store.node_password(radio, 7).await.unwrap(), None);
    }
}
