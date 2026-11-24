// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The packets MeshCore radios heard, raw: each once, by what names it in
//! all its copies, and each copy apart, with the way it came. Told to the
//! live view as packets, as Meshtastic's are.

use super::{Store, u32_from};
use anyhow::Result;
use meshctl_api::{Packet, PacketFilter, PacketKind, PacketReach, RadioId, SightingId};
use meshctl_core::mesh::BROADCAST;
use meshctl_core::meshcore::raw;

/// The channel of a packet that went on none, as no filter picks it: an
/// advert, a path, what goes between two nodes.
const NO_CHANNEL: u32 = u32::MAX;

/// A copy of a MeshCore packet heard, read, to keep.
pub struct NewMeshcoreCopy {
    pub key: u64,
    pub payload_type: u8,
    /// The live view's kind of it, as the API names kinds.
    pub kind: PacketKind,
    pub from: Option<u32>,
    pub to: Option<u32>,
    pub channel: Option<u32>,
    /// What could be read of it.
    pub info: serde_json::Value,
    pub payload_len: usize,
    pub at: i64,
    pub snr: f32,
    pub rssi: i32,
    pub route: u8,
    /// The region it came in, by its name; `?` for one whose key is not
    /// known.
    pub scope: Option<String>,
    pub path: Vec<u8>,
    pub hash_size: u8,
}

#[derive(sqlx::FromRow)]
struct MeshcoreRow {
    id: i64,
    key: i64,
    payload_type: i64,
    kind: String,
    from_num: Option<i64>,
    to_num: Option<i64>,
    channel: Option<i64>,
    info: String,
    payload_len: i64,
    at: i64,
    copies: i64,
    snr: Option<f32>,
    rssi: Option<i32>,
    route: i64,
    scope: Option<String>,
    path: Vec<u8>,
    hash_size: i64,
}

/// The kind a kept packet was given, as the API names it.
fn kind_name(kind: PacketKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".to_owned())
}

impl MeshcoreRow {
    /// As the live view takes a packet: its first copy's signal and way,
    /// and in its payload what was read of it, how it came and how many
    /// copies were heard.
    fn packet(self) -> Packet {
        let size = usize::try_from(self.hash_size).unwrap_or(1).max(1);
        let payload_type = u8::try_from(self.payload_type).unwrap_or(0xff);
        let hops: Vec<String> = self
            .path
            .chunks(size)
            .map(|hop| hop.iter().map(|b| format!("{b:02x}")).collect())
            .collect();
        let flooded = matches!(
            u8::try_from(self.route).unwrap_or(0),
            raw::ROUTE_FLOOD | raw::ROUTE_TRANSPORT_FLOOD
        );
        let mut payload: serde_json::Value =
            serde_json::from_str(&self.info).unwrap_or(serde_json::Value::Null);
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "route".into(),
                (if flooded { "flood" } else { "direct" }).into(),
            );
            if let Some(scope) = &self.scope {
                object.insert("region".into(), scope.clone().into());
            }
            // A trace's path is the SNR each hop heard it at, told apart.
            if payload_type != raw::PAYLOAD_TRACE {
                object.insert("path".into(), hops.clone().into());
            }
            object.insert("copies".into(), self.copies.into());
            object.insert("size".into(), self.payload_len.into());
        }
        // A channel's text not opened is as encrypted as a sealed packet of
        // Meshtastic's is: no port.
        let sealed = self.kind == "encrypted" && payload_type == 0x05;
        let trace = payload_type == raw::PAYLOAD_TRACE;
        Packet {
            id: self.id,
            packet_id: u32::try_from(self.key & 0xffff_ffff).unwrap_or(0),
            from: self.from_num.map_or(0, u32_from),
            to: self.to_num.map_or(BROADCAST, u32_from),
            channel: self.channel.map_or(NO_CHANNEL, u32_from),
            port: (!sealed).then(|| raw::payload_name(payload_type).to_owned()),
            payload: Some(payload),
            request_id: None,
            reply_id: None,
            want_response: false,
            pki: false,
            at: self.at,
            snr: self.snr,
            rssi: self.rssi,
            // A trace's path is the SNR of each hop, not hops it went
            // through; a direct one's, the way it goes, not the way it came.
            hops_away: (flooded && !trace).then(|| u32::try_from(hops.len()).unwrap_or(0)),
            hop_limit: 0,
            hop_start: 0,
            relay_node: (flooded && !trace)
                .then(|| self.path.chunks(size).last().and_then(|hop| hop.first()))
                .flatten()
                .map(|b| u32::from(*b)),
            via_mqtt: false,
        }
    }
}

/// What a packet row is read as: the packet, how many copies, and its
/// first copy's.
const SELECT: &str = "SELECT p.id, p.key, p.payload_type, p.kind, p.from_num, p.to_num,
        p.channel, p.info, p.payload_len, p.at,
        (SELECT count(*) FROM meshcore_packet_copy n WHERE n.packet = p.id) AS copies,
        f.snr, f.rssi, f.route, f.scope, f.path, f.hash_size
    FROM meshcore_packet p
    JOIN meshcore_packet_copy f
        ON f.id = (SELECT min(c.id) FROM meshcore_packet_copy c WHERE c.packet = p.id)";

impl Store {
    /// Keeps a copy of a MeshCore packet heard: the packet the first time,
    /// and each copy. The packet as now kept.
    pub async fn add_meshcore_copy(
        &self,
        radio: RadioId,
        copy: &NewMeshcoreCopy,
    ) -> Result<Packet> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        #[allow(clippy::cast_possible_wrap)]
        let key = copy.key as i64;
        let (packet,): (i64,) = sqlx::query_as(
            "INSERT INTO meshcore_packet (radio_id, key, payload_type, kind, from_num, to_num,
                channel, info, payload_len, at, last_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (radio_id, key) DO UPDATE SET last_at = excluded.last_at
            RETURNING id",
        )
        .bind(radio)
        .bind(key)
        .bind(i64::from(copy.payload_type))
        .bind(kind_name(copy.kind))
        .bind(copy.from.map(i64::from))
        .bind(copy.to.map(i64::from))
        .bind(copy.channel.map(i64::from))
        .bind(copy.info.to_string())
        .bind(i64::try_from(copy.payload_len).unwrap_or(0))
        .bind(copy.at)
        .bind(copy.at)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO meshcore_packet_copy (packet, at, snr, rssi, route, scope, path, hash_size)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(packet)
        .bind(copy.at)
        .bind(copy.snr)
        .bind(copy.rssi)
        .bind(i64::from(copy.route))
        .bind(&copy.scope)
        .bind(&copy.path)
        .bind(i64::from(copy.hash_size))
        .execute(&mut *tx)
        .await?;
        let row: MeshcoreRow = sqlx::QueryBuilder::new(SELECT)
            .push(" WHERE p.id = ")
            .push_bind(packet)
            .build_query_as()
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(row.packet())
    }

    /// The packets a MeshCore radio heard, the newest first, as the live view
    /// filters them: by kind, channel, how they came, a node.
    pub async fn meshcore_packets(
        &self,
        radio: RadioId,
        before: Option<SightingId>,
        filter: &PacketFilter,
        limit: u32,
    ) -> Result<Vec<Packet>> {
        let mut query = sqlx::QueryBuilder::new(format!("{SELECT} WHERE p.radio_id = "));
        query.push_bind(radio);
        if let Some(before) = before {
            query.push(" AND p.id < ").push_bind(before);
        }
        if let Some(kind) = filter.kind {
            query.push(" AND p.kind = ").push_bind(kind_name(kind));
        }
        if let Some(channel) = filter.channel {
            query
                .push(" AND p.channel = ")
                .push_bind(i64::from(channel));
        }
        match filter.reach {
            // Heard from its sender: with no repeater on its way.
            Some(PacketReach::Direct) => {
                query.push(" AND f.route IN (0, 1) AND length(f.path) = 0");
            }
            Some(PacketReach::Mesh) => {
                query.push(" AND f.route IN (0, 1) AND length(f.path) > 0");
            }
            // MeshCore has none.
            Some(PacketReach::Mqtt) => {
                query.push(" AND 0");
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
            query
                .push(" AND (p.from_num IS NULL OR p.from_num != ")
                .push_bind(i64::from(num))
                .push(")");
        }
        query
            .push(" ORDER BY p.id DESC LIMIT ")
            .push_bind(i64::from(limit));
        let rows: Vec<MeshcoreRow> = query.build_query_as().fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(MeshcoreRow::packet).collect())
    }

    /// The copies of a packet a MeshCore radio heard, the first first, each
    /// as the way it came: none if there is no such packet.
    pub async fn meshcore_routes(
        &self,
        radio: RadioId,
        packet: SightingId,
    ) -> Result<Option<Vec<super::RouteRow>>> {
        let known: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM meshcore_packet WHERE radio_id = ? AND id = ?")
                .bind(radio)
                .bind(packet)
                .fetch_optional(&self.pool)
                .await?;
        if known.is_none() {
            return Ok(None);
        }
        Ok(Some(
            sqlx::query_as(
                "SELECT path, hash_size, route IN (0, 1) AS flooded, snr, rssi, scope, at
                FROM meshcore_packet_copy WHERE packet = ? ORDER BY id",
            )
            .bind(packet)
            .fetch_all(&self.pool)
            .await?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_api::{RadioKind, RadioSettings};
    use serde_json::json;

    async fn store_with_radio() -> (Store, RadioId) {
        let store = Store::in_memory().await;
        let radio = store
            .add_radio(&RadioSettings {
                name: "Cova".into(),
                kind: RadioKind::Meshcore,
                address: "127.0.0.1:5000".into(),
                enabled: true,
                shown: true,
                badge: Default::default(),
            })
            .await
            .unwrap()
            .id;
        (store, radio)
    }

    fn text(key: u64, path: &[u8], at: i64) -> NewMeshcoreCopy {
        NewMeshcoreCopy {
            key,
            payload_type: 0x05,
            kind: PacketKind::Text,
            from: None,
            to: None,
            channel: Some(1),
            info: json!({ "sender": "Pepe", "text": "Bos días" }),
            payload_len: 40,
            at,
            snr: 6.0,
            rssi: -92,
            route: raw::ROUTE_FLOOD,
            scope: None,
            path: path.to_vec(),
            hash_size: 1,
        }
    }

    fn advert(key: u64, at: i64) -> NewMeshcoreCopy {
        NewMeshcoreCopy {
            payload_type: raw::PAYLOAD_ADVERT,
            kind: PacketKind::NodeInfo,
            from: Some(0x5259_6067),
            channel: None,
            info: json!({ "name": "Torre" }),
            ..text(key, &[], at)
        }
    }

    #[tokio::test]
    async fn copies_of_a_packet_are_one_packet() {
        let (store, radio) = store_with_radio().await;
        let first = store
            .add_meshcore_copy(radio, &text(7, &[0x52, 0x55], 100))
            .await
            .unwrap();
        let second = store
            .add_meshcore_copy(radio, &text(7, &[0x53, 0x55], 101))
            .await
            .unwrap();
        assert_eq!(first.id, second.id);
        let payload = second.payload.unwrap();
        assert_eq!(payload["copies"], 2);
        // As the first copy came: its path, its time.
        assert_eq!(payload["path"], json!(["52", "55"]));
        assert_eq!(second.at, 100);
        assert_eq!(second.hops_away, Some(2));
        assert_eq!(second.relay_node, Some(0x55));
        assert_eq!(second.port.as_deref(), Some("GRP_TXT"));
        let all = store
            .meshcore_packets(radio, None, &PacketFilter::default(), 50)
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        // Each copy, the way it came.
        let routes = store
            .meshcore_routes(radio, first.id)
            .await
            .unwrap()
            .unwrap();
        let paths: Vec<_> = routes.iter().map(|r| r.path.clone()).collect();
        assert_eq!(paths, [vec![0x52, 0x55], vec![0x53, 0x55]]);
        assert!(routes.iter().all(|r| r.flooded));
        assert_eq!(
            store.meshcore_routes(radio, first.id + 1).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn filtered_by_how_they_came_and_their_channel() {
        let (store, radio) = store_with_radio().await;
        store
            .add_meshcore_copy(radio, &advert(1, 100))
            .await
            .unwrap();
        store
            .add_meshcore_copy(radio, &text(2, &[0x52], 101))
            .await
            .unwrap();
        // Each packet read, as whether it is the advert.
        let adverts = |packets: Vec<Packet>| {
            packets
                .into_iter()
                .map(|p| p.payload.unwrap()["name"].is_string())
                .collect::<Vec<_>>()
        };
        let read = |filter: PacketFilter| {
            let store = store.clone();
            async move {
                adverts(
                    store
                        .meshcore_packets(radio, None, &filter, 50)
                        .await
                        .unwrap(),
                )
            }
        };
        let reach = |reach| PacketFilter {
            reach: Some(reach),
            ..Default::default()
        };
        // The advert, straight; the text, through a repeater; by MQTT, none.
        assert_eq!(read(reach(PacketReach::Direct)).await, [true]);
        assert_eq!(read(reach(PacketReach::Mesh)).await, [false]);
        assert!(read(reach(PacketReach::Mqtt)).await.is_empty());
        let channel = |channel| PacketFilter {
            channel: Some(channel),
            ..Default::default()
        };
        assert_eq!(read(channel(1)).await, [false]);
        assert!(read(channel(0)).await.is_empty());
        // The advert went on no channel, and the live view leaves it out of
        // channel 0 as the list does.
        let advert = store
            .meshcore_packets(radio, None, &PacketFilter::default(), 50)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert!(!channel(0).lets(&advert));
        assert_eq!(advert.from, 0x5259_6067);
    }
}
