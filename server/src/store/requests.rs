// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the nodes were asked for, and what they answered.

use super::{Store, u32_from};
use anyhow::{Context, Result};
use meshctl_api::{Pace, RadioId, Request, RequestId, RequestKind, RequestStatus, SightingId};
use meshctl_proto::meshtastic::PortNum;

#[derive(sqlx::FromRow)]
struct RequestRow {
    id: RequestId,
    node: i64,
    kind: String,
    channel: i64,
    status: String,
    error: Option<String>,
    at: i64,
    answered_at: Option<i64>,
    answer: Option<SightingId>,
}

fn kind_name(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::NodeInfo => "node_info",
        RequestKind::Position => "position",
        RequestKind::DeviceTelemetry => "device_telemetry",
        RequestKind::EnvironmentTelemetry => "environment_telemetry",
        RequestKind::PowerTelemetry => "power_telemetry",
        RequestKind::LocalStats => "local_stats",
        RequestKind::Neighbors => "neighbors",
    }
}

fn kind_from(name: &str) -> RequestKind {
    match name {
        "position" => RequestKind::Position,
        "device_telemetry" => RequestKind::DeviceTelemetry,
        "environment_telemetry" => RequestKind::EnvironmentTelemetry,
        "power_telemetry" => RequestKind::PowerTelemetry,
        "local_stats" => RequestKind::LocalStats,
        "neighbors" => RequestKind::Neighbors,
        _ => RequestKind::NodeInfo,
    }
}

/// The port a request is answered on.
fn port_of(kind: RequestKind) -> PortNum {
    match kind {
        RequestKind::NodeInfo => PortNum::NodeinfoApp,
        RequestKind::Position => PortNum::PositionApp,
        RequestKind::DeviceTelemetry
        | RequestKind::EnvironmentTelemetry
        | RequestKind::PowerTelemetry
        | RequestKind::LocalStats => PortNum::TelemetryApp,
        RequestKind::Neighbors => PortNum::NeighborinfoApp,
    }
}

fn status_from(name: &str) -> RequestStatus {
    match name {
        "answered" => RequestStatus::Answered,
        "failed" => RequestStatus::Failed,
        _ => RequestStatus::Pending,
    }
}

impl Store {
    async fn request_of(&self, row: RequestRow) -> Result<Request> {
        let answer = match row.answer {
            Some(sighting) => self.sighting(sighting).await?.map(crate::packets::packet),
            None => None,
        };
        Ok(Request {
            id: row.id,
            node: u32_from(row.node),
            kind: kind_from(&row.kind),
            channel: u32_from(row.channel),
            status: status_from(&row.status),
            error: row.error,
            at: row.at,
            answered_at: row.answered_at,
            answer,
        })
    }

    async fn requests_of(&self, rows: Vec<RequestRow>) -> Result<Vec<Request>> {
        let mut requests = Vec::with_capacity(rows.len());
        for row in rows {
            requests.push(self.request_of(row).await?);
        }
        Ok(requests)
    }

    pub async fn add_request(
        &self,
        radio: RadioId,
        node: u32,
        kind: RequestKind,
        channel: u32,
        packet_id: u32,
        at: i64,
    ) -> Result<Request> {
        let row: RequestRow = sqlx::query_as(
            "INSERT INTO request (radio_id, node, kind, channel, packet_id, status, at)
            VALUES (?, ?, ?, ?, ?, 'pending', ?)
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer",
        )
        .bind(radio)
        .bind(i64::from(node))
        .bind(kind_name(kind))
        .bind(i64::from(channel))
        .bind(i64::from(packet_id))
        .bind(at)
        .fetch_optional(&self.pool)
        .await?
        .context("a request just kept is not there")?;
        self.request_of(row).await
    }

    /// When a radio last asked any node for something sent at its own pace,
    /// if it ever did: a traceroute, or a request of that pace.
    pub async fn last_paced_at(&self, radio: RadioId, pace: Pace) -> Result<Option<i64>> {
        if pace == Pace::Traceroute {
            return self.last_traceroute_at(radio).await;
        }
        let kinds = [
            RequestKind::NodeInfo,
            RequestKind::Position,
            RequestKind::DeviceTelemetry,
            RequestKind::EnvironmentTelemetry,
            RequestKind::PowerTelemetry,
            RequestKind::LocalStats,
            RequestKind::Neighbors,
        ]
        .into_iter()
        .filter(|kind| kind.pace() == Some(pace))
        .map(|kind| format!("'{}'", kind_name(kind)))
        .collect::<Vec<_>>()
        .join(", ");
        let query = format!("SELECT max(at) FROM request WHERE radio_id = ? AND kind IN ({kinds})");
        Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .bind(radio)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Whether the same thing was asked of the same node and is still waiting.
    pub async fn is_asking(&self, radio: RadioId, node: u32, kind: RequestKind) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM request
            WHERE radio_id = ? AND node = ? AND kind = ? AND status = 'pending' LIMIT 1",
        )
        .bind(radio)
        .bind(i64::from(node))
        .bind(kind_name(kind))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Takes a packet heard as the answer to the request it says it answers,
    /// if that is one waiting, the node asked sent it, and it is the kind of
    /// answer asked for: a refusal comes on another port, as a routing error.
    pub async fn answer_request(
        &self,
        radio: RadioId,
        request_id: u32,
        from: u32,
        portnum: Option<i32>,
        sighting: SightingId,
        at: i64,
    ) -> Result<Option<Request>> {
        let waiting: Option<(RequestId, String)> = sqlx::query_as(
            "SELECT id, kind FROM request
            WHERE radio_id = ? AND packet_id = ? AND node = ? AND status = 'pending'",
        )
        .bind(radio)
        .bind(i64::from(request_id))
        .bind(i64::from(from))
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, kind)) = waiting else {
            return Ok(None);
        };
        if portnum != Some(port_of(kind_from(&kind)) as i32) {
            return Ok(None);
        }
        let row: RequestRow = sqlx::query_as(
            "UPDATE request SET status = 'answered', answered_at = ?, answer = ?
            WHERE id = ?
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer",
        )
        .bind(at)
        .bind(sighting)
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(Some(self.request_of(row).await?))
    }

    /// A request that will not be answered, and why.
    pub async fn fail_request(
        &self,
        radio: RadioId,
        packet_id: u32,
        error: &str,
    ) -> Result<Option<Request>> {
        let row: Option<RequestRow> = sqlx::query_as(
            "UPDATE request SET status = 'failed', error = ?
            WHERE radio_id = ? AND packet_id = ? AND status = 'pending'
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer",
        )
        .bind(error)
        .bind(radio)
        .bind(i64::from(packet_id))
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(self.request_of(row).await?)),
            None => Ok(None),
        }
    }

    /// The requests sent before `before` that are still waiting, given up on.
    pub async fn expire_requests(&self, radio: RadioId, before: i64) -> Result<Vec<Request>> {
        let rows = sqlx::query_as(
            "UPDATE request SET status = 'failed', error = 'TIMEOUT'
            WHERE radio_id = ? AND status = 'pending' AND at < ?
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer",
        )
        .bind(radio)
        .bind(before)
        .fetch_all(&self.pool)
        .await?;
        self.requests_of(rows).await
    }

    /// A radio's requests, to one node or to any, the newest first.
    pub async fn requests(
        &self,
        radio: RadioId,
        node: Option<u32>,
        limit: u32,
    ) -> Result<Vec<Request>> {
        let node = node.map(i64::from);
        let rows = sqlx::query_as(
            "SELECT id, node, kind, channel, status, error, at, answered_at, answer
            FROM request
            WHERE radio_id = ? AND (? IS NULL OR node = ?)
            ORDER BY id DESC LIMIT ?",
        )
        .bind(radio)
        .bind(node)
        .bind(node)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        self.requests_of(rows).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_api::{RadioKind, RadioSettings};
    use meshctl_core::mesh::Heard;

    async fn store() -> Store {
        let store = Store::in_memory().await;
        store
            .add_radio(&RadioSettings {
                name: "Home".into(),
                kind: RadioKind::Meshtastic,
                address: "127.0.0.1:1".into(),
                enabled: false,
                shown: true,
                badge: Default::default(),
            })
            .await
            .unwrap();
        store
    }

    fn answer(request_id: u32, from: u32, portnum: i32) -> Heard {
        Heard {
            packet_id: 900,
            from,
            to: 1,
            channel: 0,
            portnum: Some(portnum),
            payload: Vec::new(),
            request_id: Some(request_id),
            reply_id: None,
            want_response: false,
            pki: false,
            at: 20,
            snr: None,
            rssi: None,
            hop_limit: 3,
            hop_start: 3,
            relay_node: None,
            via_mqtt: false,
        }
    }

    #[tokio::test]
    async fn telemetry_of_every_kind_goes_at_one_pace() {
        let store = store().await;
        assert_eq!(store.last_paced_at(1, Pace::Telemetry).await.unwrap(), None);
        store
            .add_request(1, 0x55, RequestKind::DeviceTelemetry, 0, 1, 10)
            .await
            .unwrap();
        store
            .add_request(1, 0x66, RequestKind::LocalStats, 0, 2, 14)
            .await
            .unwrap();
        // Node info goes whenever asked: it sets no pace.
        store
            .add_request(1, 0x66, RequestKind::NodeInfo, 0, 3, 20)
            .await
            .unwrap();
        assert_eq!(
            store.last_paced_at(1, Pace::Telemetry).await.unwrap(),
            Some(14)
        );
        assert_eq!(store.last_paced_at(1, Pace::Position).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_answer_is_taken_only_from_the_node_and_port_asked() {
        let store = store().await;
        let asked = store
            .add_request(1, 0x55, RequestKind::Position, 0, 77, 10)
            .await
            .unwrap();
        assert_eq!(asked.status, RequestStatus::Pending);
        assert!(
            store
                .is_asking(1, 0x55, RequestKind::Position)
                .await
                .unwrap()
        );

        // A routing answer for it is no position.
        let routing = store
            .add_packet(1, &answer(77, 0x55, 5))
            .await
            .unwrap()
            .unwrap();
        let taken = store
            .answer_request(1, 77, 0x55, routing.portnum, routing.id, 20)
            .await
            .unwrap();
        assert!(taken.is_none());

        let mut heard = answer(77, 0x55, PortNum::PositionApp as i32);
        heard.packet_id = 901;
        let position = store.add_packet(1, &heard).await.unwrap().unwrap();
        let answered = store
            .answer_request(1, 77, 0x55, position.portnum, position.id, 21)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(answered.status, RequestStatus::Answered);
        assert_eq!(answered.answered_at, Some(21));
        assert_eq!(answered.answer.unwrap().packet_id, 901);
        assert!(
            !store
                .is_asking(1, 0x55, RequestKind::Position)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_request_fails_or_expires() {
        let store = store().await;
        store
            .add_request(1, 0x55, RequestKind::Neighbors, 0, 1, 10)
            .await
            .unwrap();
        store
            .add_request(1, 0x66, RequestKind::LocalStats, 2, 2, 100)
            .await
            .unwrap();
        let refused = store
            .fail_request(1, 1, "NO_RESPONSE")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refused.error.as_deref(), Some("NO_RESPONSE"));
        assert!(
            store
                .fail_request(1, 1, "NO_RESPONSE")
                .await
                .unwrap()
                .is_none()
        );

        let expired = store.expire_requests(1, 200).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].error.as_deref(), Some("TIMEOUT"));
        assert_eq!(expired[0].channel, 2);

        let to_one = store.requests(1, Some(0x55), 10).await.unwrap();
        assert_eq!(to_one.len(), 1);
        assert_eq!(store.requests(1, None, 10).await.unwrap().len(), 2);
    }
}
