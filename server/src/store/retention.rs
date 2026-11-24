// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How long what is kept is kept: the packets heard, the messages. And what
//! is left behind, cleaned away as asked: the conversations of channels no
//! radio has, and with nodes meshctl no longer keeps.

use super::Store;
use anyhow::Result;
use meshctl_api::{BROADCAST, Gone, Retention, Storage};
use meshctl_proto::meshtastic::{PortNum, Telemetry};
use prost::Message as _;

/// For telemetry, which kind it carries, as its field in the protobufs
/// numbers it; `None` for any other port, or what does not read.
pub(super) fn telemetry_kind(portnum: Option<i32>, payload: &[u8]) -> Option<i64> {
    if portnum != Some(PortNum::TelemetryApp as i32) {
        return None;
    }
    let variant = Telemetry::decode(payload).ok()?.variant?;
    // Its field's number, as written: whatever kinds the protobufs know.
    let mut written = Vec::new();
    variant.encode(&mut written);
    let (tag, _) = prost::encoding::decode_key(&mut written.as_slice()).ok()?;
    Some(i64::from(tag))
}

/// Which messages, as `m`, are direct ones with a node meshctl no longer
/// keeps; the broadcast address bound first. A macro, for the SQL to stay
/// static.
macro_rules! gone_direct {
    () => {
        " m.to_num != ? AND NOT EXISTS (
            SELECT 1 FROM node n WHERE n.radio_id = m.radio_id
                AND n.num = CASE WHEN m.outgoing THEN m.to_num ELSE m.from_num END) "
    };
}

/// Which channels, as `c`, no radio has in a slot any more.
macro_rules! gone_channel {
    () => {
        " NOT EXISTS (SELECT 1 FROM channel_slot s WHERE s.channel_id = c.id) "
    };
}

impl Store {
    /// How long what meshctl keeps is kept, or the defaults when it was never
    /// said. A row that does not read, written by a newer meshctl, counts as
    /// none.
    pub async fn retention(&self) -> Result<Retention> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM preference WHERE key = 'retention'")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|(value,)| {
                serde_json::from_str(&value)
                    .inspect_err(|err| tracing::warn!(%err, "ignoring the stored retention"))
                    .ok()
            })
            .unwrap_or_default())
    }

    pub async fn set_retention(&self, retention: &Retention) -> Result<()> {
        sqlx::query(
            "INSERT INTO preference (key, value) VALUES ('retention', ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(serde_json::to_string(retention)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forgets the packets first heard before `before` (seconds since the
    /// epoch) but each node's latest of each kind, however old: what its page
    /// shows of it. The requests and traceroutes sent before then go too,
    /// but those still waiting. How many packets went.
    pub async fn prune_packets(&self, before: i64) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let packets = sqlx::query(
            "DELETE FROM packet WHERE at < ?1 AND id NOT IN (
                SELECT id FROM (
                    SELECT id, row_number() OVER (
                        PARTITION BY from_num, portnum, kind ORDER BY at DESC, id DESC
                    ) AS latest
                    FROM packet WHERE portnum IS NOT NULL
                ) WHERE latest = 1
            )",
        )
        .bind(before)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query("DELETE FROM request WHERE at < ? AND status != 'pending'")
            .bind(before)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM traceroute WHERE at < ? AND status != 'pending'")
            .bind(before)
            .execute(&mut *tx)
            .await?;
        // And MeshCore's, raw, by the last copy heard of each.
        let raw = sqlx::query("DELETE FROM meshcore_packet WHERE last_at < ?")
            .bind(before)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(packets + raw)
    }

    /// Forgets the messages heard or sent before `before`. How many went.
    pub async fn prune_messages(&self, before: i64) -> Result<u64> {
        // The ways they came with them.
        sqlx::query("DELETE FROM message_route WHERE at < ?")
            .bind(before)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM message_relay WHERE at < ?")
            .bind(before)
            .execute(&self.pool)
            .await?;
        Ok(sqlx::query("DELETE FROM message WHERE at < ?")
            .bind(before)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    /// Forgets the channels no radio has in a slot any more, and their
    /// conversations. How many messages went.
    pub async fn forget_gone_channels(&self) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let (messages,): (i64,) = sqlx::query_as(concat!(
            "SELECT COUNT(*) FROM message m JOIN channel c ON c.id = m.channel_id
            WHERE ",
            gone_channel!()
        ))
        .fetch_one(&mut *tx)
        .await?;
        // Their messages and what was read of them go with them.
        sqlx::query(concat!("DELETE FROM channel AS c WHERE ", gone_channel!()))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(u64::try_from(messages).unwrap_or_default())
    }

    /// Forgets the direct conversations with nodes meshctl no longer keeps.
    /// How many messages went.
    pub async fn forget_gone_directs(&self) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let messages = sqlx::query(concat!("DELETE FROM message AS m WHERE ", gone_direct!()))
            .bind(i64::from(BROADCAST))
            .execute(&mut *tx)
            .await?
            .rows_affected();
        sqlx::query(
            "DELETE FROM conversation_read AS r WHERE r.node IS NOT NULL AND NOT EXISTS (
                SELECT 1 FROM node n WHERE n.radio_id = r.radio_id AND n.num = r.node)",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(messages)
    }

    /// What is kept, and how much room it takes.
    pub async fn storage(&self) -> Result<Storage> {
        let count = |sql: &'static str| async move {
            let (n,): (i64,) = sqlx::query_as(sql).fetch_one(&self.pool).await?;
            Ok::<_, anyhow::Error>(u64::try_from(n).unwrap_or_default())
        };
        let (oldest_packet,): (Option<i64>,) = sqlx::query_as("SELECT MIN(at) FROM packet")
            .fetch_one(&self.pool)
            .await?;
        let (gone_channel_chats, gone_channel_messages): (i64, i64) = sqlx::query_as(concat!(
            "SELECT COUNT(*), COALESCE(SUM(
                (SELECT COUNT(*) FROM message m WHERE m.channel_id = c.id)), 0)
            FROM channel c WHERE ",
            gone_channel!(),
            "
                AND EXISTS (SELECT 1 FROM message m WHERE m.channel_id = c.id)"
        ))
        .fetch_one(&self.pool)
        .await?;
        let (gone_direct_chats, gone_direct_messages): (i64, i64) = sqlx::query_as(concat!(
            "SELECT COUNT(DISTINCT m.radio_id * 4294967296
                + CASE WHEN m.outgoing THEN m.to_num ELSE m.from_num END), COUNT(*)
            FROM message m WHERE ",
            gone_direct!()
        ))
        .bind(i64::from(BROADCAST))
        .fetch_one(&self.pool)
        .await?;
        let gone = |chats: i64, messages: i64| Gone {
            conversations: u64::try_from(chats).unwrap_or_default(),
            messages: u64::try_from(messages).unwrap_or_default(),
        };
        Ok(Storage {
            bytes: self.used_bytes().await?,
            packets: count("SELECT COUNT(*) FROM packet").await?,
            oldest_packet,
            messages: count("SELECT COUNT(*) FROM message").await?,
            nodes: count("SELECT COUNT(*) FROM node").await?,
            gone_channels: gone(gone_channel_chats, gone_channel_messages),
            gone_directs: gone(gone_direct_chats, gone_direct_messages),
        })
    }

    /// The pages of the database in use, in bytes.
    async fn used_bytes(&self) -> Result<u64> {
        let pragma = |name: &'static str| async move {
            let (n,): (i64,) = sqlx::query_as(name).fetch_one(&self.pool).await?;
            Ok::<_, anyhow::Error>(u64::try_from(n).unwrap_or_default())
        };
        let pages = pragma("PRAGMA page_count").await?;
        let free = pragma("PRAGMA freelist_count").await?;
        let size = pragma("PRAGMA page_size").await?;
        Ok(pages.saturating_sub(free) * size)
    }

    /// Gives the room freed back to the disk. How much, in bytes.
    pub async fn reclaim(&self) -> Result<u64> {
        let (free,): (i64,) = sqlx::query_as("PRAGMA freelist_count")
            .fetch_one(&self.pool)
            .await?;
        let (size,): (i64,) = sqlx::query_as("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await?;
        sqlx::query("PRAGMA incremental_vacuum")
            .execute(&self.pool)
            .await?;
        Ok(u64::try_from(free * size).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_api::{RadioKind, RadioSettings};
    use meshctl_core::mesh::Heard;
    use meshctl_proto::meshtastic::{DeviceMetrics, EnvironmentMetrics, telemetry};

    fn telemetry(variant: telemetry::Variant) -> Vec<u8> {
        Telemetry {
            time: 0,
            variant: Some(variant),
        }
        .encode_to_vec()
    }

    fn heard(packet_id: u32, from: u32, portnum: PortNum, payload: Vec<u8>, at: u32) -> Heard {
        Heard {
            packet_id,
            from,
            to: BROADCAST,
            channel: 0,
            portnum: Some(portnum as i32),
            payload,
            request_id: None,
            reply_id: None,
            want_response: false,
            pki: false,
            at,
            snr: None,
            rssi: None,
            hop_limit: 3,
            hop_start: 3,
            relay_node: None,
            via_mqtt: false,
        }
    }

    async fn store_with_radio() -> (Store, i64) {
        let store = Store::in_memory().await;
        let radio = store
            .add_radio(&RadioSettings {
                name: "Casa".into(),
                kind: RadioKind::Meshtastic,
                address: "127.0.0.1:4403".into(),
                enabled: true,
                shown: true,
                badge: Default::default(),
            })
            .await
            .unwrap()
            .id;
        (store, radio)
    }

    #[test]
    fn telemetry_is_told_by_its_kind() {
        let device = telemetry(telemetry::Variant::DeviceMetrics(DeviceMetrics::default()));
        let environment = telemetry(telemetry::Variant::EnvironmentMetrics(
            EnvironmentMetrics::default(),
        ));
        let port = Some(PortNum::TelemetryApp as i32);
        assert_eq!(telemetry_kind(port, &device), Some(2));
        assert_eq!(telemetry_kind(port, &environment), Some(3));
        assert_eq!(
            telemetry_kind(Some(PortNum::PositionApp as i32), &device),
            None
        );
    }

    #[tokio::test]
    async fn old_packets_go_but_each_nodes_latest_of_each_kind() {
        let (store, radio) = store_with_radio().await;
        let device = || telemetry(telemetry::Variant::DeviceMetrics(DeviceMetrics::default()));
        let environment = || {
            telemetry(telemetry::Variant::EnvironmentMetrics(
                EnvironmentMetrics::default(),
            ))
        };
        // Node 7: environment long ago, device long ago and lately.
        for packet in [
            heard(1, 7, PortNum::TelemetryApp, environment(), 100),
            heard(2, 7, PortNum::TelemetryApp, device(), 110),
            heard(3, 7, PortNum::TelemetryApp, device(), 120),
            heard(4, 7, PortNum::TelemetryApp, device(), 1_000),
            heard(5, 7, PortNum::PositionApp, vec![], 130),
            heard(6, 7, PortNum::PositionApp, vec![], 140),
        ] {
            store.add_packet(radio, &packet).await.unwrap();
        }
        assert_eq!(store.prune_packets(500).await.unwrap(), 3);
        let (left,): (String,) =
            sqlx::query_as("SELECT group_concat(packet_id, ',') FROM (SELECT packet_id FROM packet ORDER BY packet_id)")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        // The only environment, the latest device, the latest position.
        assert_eq!(left, "1,4,6");
    }
}
