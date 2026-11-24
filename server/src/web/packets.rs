// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The packets each radio hears: what it sees of the mesh, kept, and as it
//! comes on the event stream as `packet`.

use super::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    Error, ErrorCode, MessageRoute, Packet, PacketFilter, PacketKind, PacketReach, RadioId,
    SightingId,
};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// How many packets a page has when the client does not say.
const PAGE: u32 = 100;

/// The most a page can have.
const PAGE_MAX: u32 = 1000;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(routes))
}

/// Which packets to read.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Which {
    /// Only those heard before this one, for the page after the last read.
    before: Option<SightingId>,
    /// Only those that carry this kind of thing.
    kind: Option<PacketKind>,
    /// Only those on the channel in this slot.
    channel: Option<u32>,
    /// Only those that came this way.
    reach: Option<PacketReach>,
    /// Only those from or to this node.
    node: Option<u32>,
    /// None from this node, as the radio's own node, to leave them out.
    not_from: Option<u32>,
    /// How many, 100 if not said and at most 1000.
    limit: Option<u32>,
}

impl Which {
    fn filter(&self) -> PacketFilter {
        PacketFilter {
            kind: self.kind,
            channel: self.channel,
            reach: self.reach,
            node: self.node,
            not_from: self.not_from,
        }
    }
}

/// Packets
///
/// The packets the radio heard, the newest first, each with what it carries
/// read where it can be. Pages go back with `before`, the `id` of the last
/// packet read; each filter leaves out what it does not let through.
#[utoipa::path(
    get,
    path = "/radios/{radio}/packets",
    tag = "packets",
    params(("radio" = RadioId, Path, description = "The radio's id"), Which),
    responses(
        (status = 200, description = "The packets", body = Vec<Packet>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn list(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Query(which): Query<Which>,
) -> Response {
    let limit = which.limit.unwrap_or(PAGE).clamp(1, PAGE_MAX);
    match state
        .radios
        .packets(radio, which.before, &which.filter(), limit)
        .await
    {
        Ok(Some(packets)) => Json(packets).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(Error {
                error: ErrorCode::NotFound,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = format_args!("{err:#}"), "packets");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A packet's routes
///
/// Each copy of a packet a MeshCore radio heard, the first first, and the
/// way it came: the repeaters it went through, from its sender's side, each
/// by its hash and the nodes the radio knows that it may be; its signal, and
/// the region it came in. A Meshtastic radio keeps none of its packets'.
#[utoipa::path(
    get,
    path = "/radios/{radio}/packets/{id}/routes",
    tag = "packets",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = SightingId, Path, description = "The packet's id"),
    ),
    responses(
        (status = 200, description = "The routes", body = Vec<MessageRoute>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or packet", body = Error),
    )
)]
async fn routes(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, SightingId)>,
) -> Response {
    match state.radios.packet_routes(radio, id).await {
        Ok(Some(routes)) => Json(routes).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(Error {
                error: ErrorCode::NotFound,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = format_args!("{err:#}"), "packet routes");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{RADIO, ask, get, json, radios};
    use crate::web::{AppState, Auth, router};
    use axum::http::StatusCode;
    use meshctl_api::Packet;
    use meshctl_core::mesh::Heard;

    fn heard(packet_id: u32, from: u32) -> Heard {
        Heard {
            packet_id,
            from,
            to: 0xffff_ffff,
            channel: 0,
            portnum: Some(1),
            payload: b"hola".to_vec(),
            request_id: None,
            reply_id: None,
            want_response: false,
            pki: false,
            at: 1000 + packet_id,
            snr: Some(6.5),
            rssi: Some(-90),
            hop_limit: 3,
            hop_start: 3,
            relay_node: None,
            via_mqtt: false,
        }
    }

    #[tokio::test]
    async fn are_read_back_newest_first_and_once() {
        let radios = radios().await;
        let store = radios.store();
        for (id, from) in [(1, 0x10), (2, 0x20), (3, 0x10)] {
            assert!(
                store
                    .add_packet(RADIO, &heard(id, from))
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        // Replayed on reconnecting: heard once.
        assert!(
            store
                .add_packet(RADIO, &heard(2, 0x20))
                .await
                .unwrap()
                .is_none()
        );

        let app = router(AppState {
            auth: Auth::new(None),
            radios,
        });
        let (status, body) = ask(app.clone(), get("/api/v1/radios/1/packets")).await;
        assert_eq!(status, StatusCode::OK);
        let all: Vec<Packet> = json(&body);
        assert_eq!(
            all.iter().map(|p| p.packet_id).collect::<Vec<_>>(),
            [3, 2, 1]
        );
        assert_eq!(all[0].payload, Some("hola".into()));
        assert_eq!(all[0].hops_away, Some(0));

        let uri = format!("/api/v1/radios/1/packets?before={}&node=16", all[0].id);
        let (_, body) = ask(app.clone(), get(&uri)).await;
        let older: Vec<Packet> = json(&body);
        assert_eq!(older.iter().map(|p| p.packet_id).collect::<Vec<_>>(), [1]);

        let (status, _) = ask(app, get("/api/v1/radios/9/packets")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn are_filtered_as_the_live_view_asks() {
        let radios = radios().await;
        let store = radios.store();
        let packets = [
            // A text, heard straight from its sender on the first channel.
            heard(1, 0x10),
            // A position, two hops away, on the second channel.
            Heard {
                portnum: Some(3),
                channel: 1,
                hop_limit: 1,
                ..heard(2, 0x20)
            },
            // Telemetry by MQTT.
            Heard {
                portnum: Some(67),
                via_mqtt: true,
                ..heard(3, 0x30)
            },
            // Still encrypted.
            Heard {
                portnum: None,
                ..heard(4, 0x10)
            },
            // A port of no kind of its own: the store and forward.
            Heard {
                portnum: Some(65),
                ..heard(5, 0x20)
            },
        ];
        for packet in &packets {
            store.add_packet(RADIO, packet).await.unwrap();
        }
        let app = router(AppState {
            auth: Auth::new(None),
            radios,
        });
        let ids = |query: &'static str| {
            let app = app.clone();
            async move {
                let (status, body) =
                    ask(app, get(&format!("/api/v1/radios/1/packets?{query}"))).await;
                assert_eq!(status, StatusCode::OK, "{query}");
                json::<Vec<Packet>>(&body)
                    .iter()
                    .map(|p| p.packet_id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(ids("kind=text").await, [1]);
        assert_eq!(ids("kind=position").await, [2]);
        assert_eq!(ids("kind=encrypted").await, [4]);
        assert_eq!(ids("kind=other").await, [5]);
        assert_eq!(ids("channel=1").await, [2]);
        assert_eq!(ids("reach=direct").await, [5, 4, 1]);
        assert_eq!(ids("reach=mesh").await, [2]);
        assert_eq!(ids("reach=mqtt").await, [3]);
        assert_eq!(ids("not_from=16").await, [5, 3, 2]);
        assert_eq!(ids("node=16&kind=text").await, [1]);
        assert_eq!(ids("reach=direct&channel=0&not_from=32").await, [4, 1]);
    }
}
