// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the nodes were asked for, and what they answered.

use super::{Store, u32_from};
use anyhow::{Context, Result};
use meshctl_api::{
    Pace, RadioId, Request, RequestId, RequestKind, RequestReply, RequestStatus, SightingId,
};
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
    reply: Option<String>,
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
        RequestKind::Regions => "regions",
        RequestKind::Status => "status",
        RequestKind::Telemetry => "telemetry",
        RequestKind::Owner => "owner",
        RequestKind::AccessList => "access_list",
        RequestKind::Ping => "ping",
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
        "regions" => RequestKind::Regions,
        "status" => RequestKind::Status,
        "telemetry" => RequestKind::Telemetry,
        "owner" => RequestKind::Owner,
        "access_list" => RequestKind::AccessList,
        "ping" => RequestKind::Ping,
        _ => RequestKind::NodeInfo,
    }
}

/// The port a request is answered on; none for one answered with no
/// packet.
fn port_of(kind: RequestKind) -> Option<PortNum> {
    Some(match kind {
        RequestKind::NodeInfo => PortNum::NodeinfoApp,
        RequestKind::Position => PortNum::PositionApp,
        RequestKind::DeviceTelemetry
        | RequestKind::EnvironmentTelemetry
        | RequestKind::PowerTelemetry
        | RequestKind::LocalStats => PortNum::TelemetryApp,
        RequestKind::Neighbors => PortNum::NeighborinfoApp,
        RequestKind::Regions
        | RequestKind::Status
        | RequestKind::Telemetry
        | RequestKind::Owner
        | RequestKind::AccessList
        | RequestKind::Ping => return None,
    })
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
            reply: row.reply.and_then(|json| serde_json::from_str(&json).ok()),
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
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer, reply",
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
            RequestKind::Regions,
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

    /// Forgets what a node was asked for through a radio, but what still
    /// waits for its answer; how many were. The packets that answered stay,
    /// as what the node told.
    pub async fn forget_requests(&self, radio: RadioId, node: u32) -> Result<u64> {
        Ok(sqlx::query(
            "DELETE FROM request WHERE radio_id = ? AND node = ? AND status != 'pending'",
        )
        .bind(radio)
        .bind(i64::from(node))
        .execute(&self.pool)
        .await?
        .rows_affected())
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
        if portnum.is_none() || portnum != port_of(kind_from(&kind)).map(|port| port as i32) {
            return Ok(None);
        }
        let row: RequestRow = sqlx::query_as(
            "UPDATE request SET status = 'answered', answered_at = ?, answer = ?
            WHERE id = ?
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer, reply",
        )
        .bind(at)
        .bind(sighting)
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(Some(self.request_of(row).await?))
    }

    /// Takes what a node answered with no packet as the answer to the
    /// request it says it answers, if that is one waiting and the node asked
    /// sent it.
    pub async fn reply_request(
        &self,
        radio: RadioId,
        request_id: u32,
        from: u32,
        reply: &RequestReply,
        at: i64,
    ) -> Result<Option<Request>> {
        let row: Option<RequestRow> = sqlx::query_as(
            "UPDATE request SET status = 'answered', answered_at = ?, reply = ?
            WHERE radio_id = ? AND packet_id = ? AND node = ? AND status = 'pending'
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer, reply",
        )
        .bind(at)
        .bind(serde_json::to_string(reply)?)
        .bind(radio)
        .bind(i64::from(request_id))
        .bind(i64::from(from))
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(self.request_of(row).await?)),
            None => Ok(None),
        }
    }

    /// Gives up on a request a node did not answer in time, by the id it
    /// went with: none if it is not waited for any longer.
    pub async fn unanswered_request(
        &self,
        radio: RadioId,
        request_id: u32,
        from: u32,
    ) -> Result<Option<Request>> {
        let row: Option<RequestRow> = sqlx::query_as(
            "UPDATE request SET status = 'failed', error = 'TIMEOUT'
            WHERE radio_id = ? AND packet_id = ? AND node = ? AND status = 'pending'
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer, reply",
        )
        .bind(radio)
        .bind(i64::from(request_id))
        .bind(i64::from(from))
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(self.request_of(row).await?)),
            None => Ok(None),
        }
    }

    /// Each node's latest answer to a regions request through a radio, and
    /// when it came.
    pub async fn latest_regions(&self, radio: RadioId) -> Result<Vec<(u32, RequestReply, i64)>> {
        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT node, reply, answered_at FROM request
            WHERE id IN (
                SELECT max(id) FROM request
                WHERE radio_id = ? AND kind = 'regions' AND status = 'answered'
                    AND reply IS NOT NULL AND answered_at IS NOT NULL
                GROUP BY node
            )",
        )
        .bind(radio)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(node, reply, at)| {
                Some((u32_from(node), serde_json::from_str(&reply).ok()?, at))
            })
            .collect())
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
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer, reply",
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
            RETURNING id, node, kind, channel, status, error, at, answered_at, answer, reply",
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
        before: Option<RequestId>,
        limit: u32,
    ) -> Result<Vec<Request>> {
        let node = node.map(i64::from);
        let rows = sqlx::query_as(
            "SELECT id, node, kind, channel, status, error, at, answered_at, answer, reply
            FROM request
            WHERE radio_id = ? AND (? IS NULL OR node = ?) AND (? IS NULL OR id < ?)
            ORDER BY id DESC LIMIT ?",
        )
        .bind(radio)
        .bind(node)
        .bind(node)
        .bind(before)
        .bind(before)
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
    async fn a_reply_answers_only_the_request_it_names_from_the_node_asked() {
        let store = store().await;
        store
            .add_request(1, 0x55, RequestKind::Regions, 0, 77, 10)
            .await
            .unwrap();
        let reply = RequestReply::Regions {
            regions: vec!["europe".into(), "galicia".into()],
            unscoped: true,
            clock: Some(1_718_789_276),
        };
        assert!(
            store
                .reply_request(1, 77, 0x66, &reply, 20)
                .await
                .unwrap()
                .is_none()
        );
        let answered = store
            .reply_request(1, 77, 0x55, &reply, 20)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(answered.status, RequestStatus::Answered);
        assert_eq!(answered.reply, Some(reply.clone()));
        assert_eq!(
            store.requests(1, Some(0x55), None, 1).await.unwrap()[0].reply,
            Some(reply)
        );
        // No packet answers it.
        let heard = store
            .add_packet(1, &answer(78, 0x55, PortNum::TextMessageApp as i32))
            .await
            .unwrap()
            .unwrap();
        store
            .add_request(1, 0x55, RequestKind::Regions, 0, 78, 30)
            .await
            .unwrap();
        assert!(
            store
                .answer_request(1, 78, 0x55, heard.portnum, heard.id, 31)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_ping_not_back_is_given_up_on_only_while_waited_for() {
        let store = store().await;
        store
            .add_request(1, 0x55, RequestKind::Ping, 0, 90, 10)
            .await
            .unwrap();
        // Not from another node.
        assert!(
            store
                .unanswered_request(1, 90, 0x66)
                .await
                .unwrap()
                .is_none()
        );
        let lost = store
            .unanswered_request(1, 90, 0x55)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lost.status, RequestStatus::Failed);
        assert_eq!(lost.error.as_deref(), Some("TIMEOUT"));
        // Once only.
        assert!(
            store
                .unanswered_request(1, 90, 0x55)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn each_nodes_latest_regions_are_gathered() {
        let store = store().await;
        let said = |regions: &[&str], unscoped| RequestReply::Regions {
            regions: regions.iter().map(|r| (*r).to_owned()).collect(),
            unscoped,
            clock: None,
        };
        store
            .add_request(1, 0x55, RequestKind::Regions, 0, 1, 10)
            .await
            .unwrap();
        store
            .reply_request(1, 1, 0x55, &said(&["europe"], false), 11)
            .await
            .unwrap();
        store
            .add_request(1, 0x55, RequestKind::Regions, 0, 2, 20)
            .await
            .unwrap();
        store
            .reply_request(1, 2, 0x55, &said(&["galicia"], true), 21)
            .await
            .unwrap();
        // One still waiting, and one that failed, say nothing.
        store
            .add_request(1, 0x66, RequestKind::Regions, 0, 3, 30)
            .await
            .unwrap();
        let mut latest = store.latest_regions(1).await.unwrap();
        latest.sort_by_key(|(node, ..)| *node);
        assert_eq!(latest, [(0x55, said(&["galicia"], true), 21)]);
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

        let to_one = store.requests(1, Some(0x55), None, 10).await.unwrap();
        assert_eq!(to_one.len(), 1);
        assert_eq!(store.requests(1, None, None, 10).await.unwrap().len(), 2);
        // A page of them, before the latest: the older.
        let older = store
            .requests(1, None, Some(to_one[0].id), 10)
            .await
            .unwrap();
        assert!(older.iter().all(|r| r.id < to_one[0].id));

        // Forgotten, but what still waits.
        let waiting = store
            .add_request(1, 0x55, RequestKind::Position, 0, 3, 300)
            .await
            .unwrap();
        assert_eq!(store.forget_requests(1, 0x55).await.unwrap(), 1);
        let left = store.requests(1, Some(0x55), None, 10).await.unwrap();
        assert_eq!(left.iter().map(|r| r.id).collect::<Vec<_>>(), [waiting.id]);
        // Those to other nodes stay.
        assert_eq!(
            store.requests(1, Some(0x66), None, 10).await.unwrap().len(),
            1
        );
    }
}
