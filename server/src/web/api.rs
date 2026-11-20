// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Version 1 of the API, but for the event stream.

use super::{AppState, convert};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{Channel, Error, ErrorCode, Node, Status, Version};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(version))
        .routes(routes!(status))
        .routes(routes!(channels))
        .routes(routes!(nodes))
        .routes(routes!(node))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

/// Version
///
/// The program answering and its version.
#[utoipa::path(
    get,
    path = "/version",
    tag = "meta",
    responses(
        (status = 200, description = "What is answering", body = Version),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn version() -> Json<Version> {
    Json(Version {
        name: "meshctl".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

/// Status
///
/// How the link to the radio is doing, and which radio it is once it has said.
#[utoipa::path(
    get,
    path = "/status",
    tag = "radio",
    responses(
        (status = 200, description = "The link and the radio", body = Status),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn status(State(state): State<AppState>) -> Json<Status> {
    Json(state.mesh.read(|mesh| Status {
        link: convert::link(mesh.link),
        local: mesh.local.as_ref().map(convert::local),
    }))
}

/// Channels
///
/// The channels configured on the radio, in slot order, without their keys.
#[utoipa::path(
    get,
    path = "/channels",
    tag = "radio",
    responses(
        (status = 200, description = "The channels", body = Vec<Channel>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn channels(State(state): State<AppState>) -> Json<Vec<Channel>> {
    Json(
        state
            .mesh
            .read(|mesh| mesh.channels.values().map(convert::channel).collect()),
    )
}

/// Nodes
///
/// Every node the radio knows of, the most recently heard first.
#[utoipa::path(
    get,
    path = "/nodes",
    tag = "nodes",
    responses(
        (status = 200, description = "The nodes", body = Vec<Node>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn nodes(State(state): State<AppState>) -> Json<Vec<Node>> {
    let mut nodes: Vec<Node> = state
        .mesh
        .read(|mesh| mesh.nodes.values().map(convert::node).collect());
    nodes.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then(a.num.cmp(&b.num)));
    Json(nodes)
}

/// Node
///
/// One node, by its id (`!90cda275`) or its number (`2429395573`).
#[utoipa::path(
    get,
    path = "/nodes/{id}",
    tag = "nodes",
    params(("id" = String, Path, description = "The node's id or number", example = "!90cda275")),
    responses(
        (status = 200, description = "The node", body = Node),
        (status = 400, description = "That is neither an id nor a number", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "The radio knows of no such node", body = Error),
    )
)]
async fn node(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(num) = node_num(&id) else {
        return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
    };
    match state
        .mesh
        .read(|mesh| mesh.nodes.get(&num).map(convert::node))
    {
        Some(node) => Json(node).into_response(),
        None => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
    }
}

/// A node number from an id, `!` and hex digits, or from the number itself.
fn node_num(id: &str) -> Option<u32> {
    match id.strip_prefix('!') {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => id.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{app, ask, get};
    use axum::http::StatusCode;
    use meshctl_api::{LinkState, Node, Status, Version};

    #[tokio::test]
    async fn says_what_is_answering() {
        let (status, body) = ask(app(None), get("/api/v1/version")).await;
        assert_eq!(status, StatusCode::OK);
        let version: Version = serde_json::from_slice(&body).unwrap();
        assert_eq!(version.name, "meshctl");
    }

    #[tokio::test]
    async fn says_how_the_radio_is() {
        let (_, body) = ask(app(None), get("/api/v1/status")).await;
        let status: Status = serde_json::from_slice(&body).unwrap();
        assert_eq!(status.link, LinkState::Ready);
        assert_eq!(status.local.unwrap().id, "!3134c955");
    }

    #[tokio::test]
    async fn lists_the_most_recently_heard_first() {
        let (_, body) = ask(app(None), get("/api/v1/nodes")).await;
        let nodes: Vec<Node> = serde_json::from_slice(&body).unwrap();
        let names: Vec<_> = nodes.iter().map(|n| n.long_name.as_deref()).collect();
        assert_eq!(names, [Some("Home"), Some("Faro")]);
    }

    #[tokio::test]
    async fn finds_a_node_by_id_or_number() {
        for address in ["/api/v1/nodes/!90cda275", "/api/v1/nodes/2429395573"] {
            let (status, body) = ask(app(None), get(address)).await;
            assert_eq!(status, StatusCode::OK, "{address}");
            let node: Node = serde_json::from_slice(&body).unwrap();
            assert_eq!(node.long_name.as_deref(), Some("Faro"));
        }
    }

    #[tokio::test]
    async fn tells_a_missing_node_from_a_wrong_address() {
        let (status, _) = ask(app(None), get("/api/v1/nodes/!00000001")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = ask(app(None), get("/api/v1/nodes/!zz")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
