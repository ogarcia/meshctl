// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl keeps between runs, in SQLite: for now, the radios.

use anyhow::{Context, Result};
use meshctl_api::{RadioId, RadioKind, RadioSettings};
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
}

#[derive(sqlx::FromRow)]
struct RadioRow {
    id: i64,
    name: String,
    kind: String,
    address: String,
    enabled: bool,
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
        })
    }
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
        let rows: Vec<RadioRow> =
            sqlx::query_as("SELECT id, name, kind, address, enabled FROM radio ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().filter_map(RadioRow::record).collect())
    }

    pub async fn radio(&self, id: RadioId) -> Result<Option<RadioRecord>> {
        let row: Option<RadioRow> =
            sqlx::query_as("SELECT id, name, kind, address, enabled FROM radio WHERE id = ?")
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
}

fn now() -> i64 {
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
