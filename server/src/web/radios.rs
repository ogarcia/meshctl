// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios, and what each one knows: its own status, its channels and the
//! nodes it hears.

use super::{AppState, convert};
use crate::radios::{tidy, validate};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    Channel, Error, ErrorCode, LinkState, Node, Radio, RadioId, RadioSettings, Status,
};
use meshctl_core::radio::Mesh;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, add))
        .routes(routes!(get, update, remove))
        .routes(routes!(status))
        .routes(routes!(channels))
        .routes(routes!(nodes))
        .routes(routes!(node))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, ErrorCode::NotFound)
}

/// Something the database would not do, which the client can do nothing about.
fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "database");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Radios
///
/// Every radio meshctl knows of, and how its link is doing.
#[utoipa::path(
    get,
    path = "/radios",
    tag = "radios",
    responses(
        (status = 200, description = "The radios", body = Vec<Radio>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn list(State(state): State<AppState>) -> Response {
    match state.radios.list().await {
        Ok(radios) => Json(radios).into_response(),
        Err(err) => failed(&err),
    }
}

/// Add a radio
///
/// Adds a radio, and connects to it if it is enabled.
#[utoipa::path(
    post,
    path = "/radios",
    tag = "radios",
    request_body = RadioSettings,
    responses(
        (status = 201, description = "Added", body = Radio),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 422, description = "Not a radio meshctl can have: `invalid_name`, `invalid_address` or `unsupported_kind`", body = Error),
    )
)]
async fn add(State(state): State<AppState>, Json(settings): Json<RadioSettings>) -> Response {
    if let Err(code) = validate(&settings) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, code);
    }
    match state.radios.add(tidy(settings)).await {
        Ok(radio) => (StatusCode::CREATED, Json(radio)).into_response(),
        Err(err) => failed(&err),
    }
}

/// Radio
///
/// One radio, and how its link is doing.
#[utoipa::path(
    get,
    path = "/radios/{radio}",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The radio", body = Radio),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn get(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.get(radio).await {
        Ok(Some(radio)) => Json(radio).into_response(),
        Ok(None) => not_found(),
        Err(err) => failed(&err),
    }
}

/// Change a radio
///
/// Replaces a radio's settings, and reconnects to it as it is now.
#[utoipa::path(
    put,
    path = "/radios/{radio}",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = RadioSettings,
    responses(
        (status = 200, description = "Changed", body = Radio),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 422, description = "Not a radio meshctl can have: `invalid_name`, `invalid_address` or `unsupported_kind`", body = Error),
    )
)]
async fn update(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(settings): Json<RadioSettings>,
) -> Response {
    if let Err(code) = validate(&settings) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, code);
    }
    match state.radios.update(radio, tidy(settings)).await {
        Ok(Some(radio)) => Json(radio).into_response(),
        Ok(None) => not_found(),
        Err(err) => failed(&err),
    }
}

/// Remove a radio
///
/// Disconnects from a radio and forgets it.
#[utoipa::path(
    delete,
    path = "/radios/{radio}",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn remove(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.remove(radio).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(err) => failed(&err),
    }
}

/// Why there is nothing to say about a radio.
enum Missing {
    NoSuchRadio,
    Failed(anyhow::Error),
}

impl IntoResponse for Missing {
    fn into_response(self) -> Response {
        match self {
            Self::NoSuchRadio => not_found(),
            Self::Failed(err) => failed(&err),
        }
    }
}

/// What a radio knows, if there is such a radio: `None` inside when it is
/// disabled, and so knows nothing.
async fn mesh_of(state: &AppState, radio: RadioId) -> Result<Option<Mesh>, Missing> {
    match state.radios.get(radio).await {
        Ok(Some(_)) => Ok(state.radios.mesh(radio)),
        Ok(None) => Err(Missing::NoSuchRadio),
        Err(err) => Err(Missing::Failed(err)),
    }
}

/// Radio status
///
/// How the link to the radio is doing, and which radio it is once it has said.
#[utoipa::path(
    get,
    path = "/radios/{radio}/status",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The link and the radio", body = Status),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn status(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    if let Err(missing) = mesh_of(&state, radio).await {
        return missing.into_response();
    }
    let status = state.radios.status(radio).unwrap_or(Status {
        link: LinkState::Disabled,
        local: None,
    });
    Json(status).into_response()
}

/// Channels
///
/// The channels configured on the radio, in slot order, without their keys.
#[utoipa::path(
    get,
    path = "/radios/{radio}/channels",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The channels", body = Vec<Channel>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn channels(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match mesh_of(&state, radio).await {
        Ok(mesh) => Json(mesh.map_or_else(Vec::new, |mesh| {
            mesh.read(|mesh| mesh.channels.values().map(convert::channel).collect())
        }))
        .into_response(),
        Err(missing) => missing.into_response(),
    }
}

/// Nodes
///
/// Every node the radio knows of, the most recently heard first.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes",
    tag = "nodes",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The nodes", body = Vec<Node>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn nodes(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    let mesh = match mesh_of(&state, radio).await {
        Ok(mesh) => mesh,
        Err(missing) => return missing.into_response(),
    };
    let mut nodes: Vec<Node> = mesh.map_or_else(Vec::new, |mesh| {
        mesh.read(|mesh| mesh.nodes.values().map(convert::node).collect())
    });
    nodes.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then(a.num.cmp(&b.num)));
    Json(nodes).into_response()
}

/// Node
///
/// One node the radio knows of, by its id (`!90cda275`) or its number
/// (`2429395573`).
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = String, Path, description = "The node's id or number", example = "!90cda275"),
    ),
    responses(
        (status = 200, description = "The node", body = Node),
        (status = 400, description = "That is neither a node id nor a number", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows of no such node", body = Error),
    )
)]
async fn node(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, String)>,
) -> Response {
    let Some(num) = node_num(&node) else {
        return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
    };
    let mesh = match mesh_of(&state, radio).await {
        Ok(mesh) => mesh,
        Err(missing) => return missing.into_response(),
    };
    match mesh.and_then(|mesh| mesh.read(|mesh| mesh.nodes.get(&num).map(convert::node))) {
        Some(node) => Json(node).into_response(),
        None => not_found(),
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
    use crate::web::tests::{RADIO, app, ask, get, json};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use meshctl_api::{Error, ErrorCode, LinkState, Node, Radio, Status};

    fn write(method: &str, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn says_how_a_radio_is() {
        let (_, body) = ask(
            app(None).await,
            get(&format!("/api/v1/radios/{RADIO}/status")),
        )
        .await;
        let status: Status = json(&body);
        assert_eq!(status.link, LinkState::Ready);
        assert_eq!(status.local.unwrap().id, "!3134c955");
    }

    #[tokio::test]
    async fn lists_the_most_recently_heard_first() {
        let (_, body) = ask(
            app(None).await,
            get(&format!("/api/v1/radios/{RADIO}/nodes")),
        )
        .await;
        let nodes: Vec<Node> = json(&body);
        let names: Vec<_> = nodes.iter().map(|n| n.long_name.as_deref()).collect();
        assert_eq!(names, [Some("Home"), Some("Faro")]);
    }

    #[tokio::test]
    async fn finds_a_node_by_id_or_number() {
        for node in ["!90cda275", "2429395573"] {
            let address = format!("/api/v1/radios/{RADIO}/nodes/{node}");
            let (status, body) = ask(app(None).await, get(&address)).await;
            assert_eq!(status, StatusCode::OK, "{address}");
            assert_eq!(json::<Node>(&body).long_name.as_deref(), Some("Faro"));
        }
    }

    #[tokio::test]
    async fn tells_a_missing_node_from_a_wrong_address() {
        let app = app(None).await;
        let missing = format!("/api/v1/radios/{RADIO}/nodes/!00000001");
        assert_eq!(
            ask(app.clone(), get(&missing)).await.0,
            StatusCode::NOT_FOUND
        );
        let wrong = format!("/api/v1/radios/{RADIO}/nodes/!zz");
        assert_eq!(
            ask(app.clone(), get(&wrong)).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ask(app, get("/api/v1/radios/999/nodes")).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn radios_are_added_changed_and_removed() {
        let app = app(None).await;
        let (status, body) = ask(
            app.clone(),
            write(
                "POST",
                "/api/v1/radios",
                r#"{"name":" Shed ","kind":"meshtastic","address":"127.0.0.1:1","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let radio: Radio = json(&body);
        assert_eq!(radio.name, "Shed");
        assert_eq!(radio.link, LinkState::Disabled);

        let (_, body) = ask(app.clone(), get("/api/v1/radios")).await;
        assert_eq!(json::<Vec<Radio>>(&body).len(), 2);

        let address = format!("/api/v1/radios/{}", radio.id);
        let (status, body) = ask(
            app.clone(),
            write(
                "PUT",
                &address,
                r#"{"name":"Shed","kind":"meshtastic","address":"/dev/ttyACM0","enabled":false}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json::<Radio>(&body).address, "/dev/ttyACM0");

        let (status, _) = ask(app.clone(), write("DELETE", &address, "")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(ask(app, get(&address)).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_radio_meshctl_cannot_have_is_refused_with_a_reason() {
        let cases = [
            (
                r#"{"name":"","kind":"meshtastic","address":"h"}"#,
                ErrorCode::InvalidName,
            ),
            (
                r#"{"name":"x","kind":"meshtastic","address":"h:p"}"#,
                ErrorCode::InvalidAddress,
            ),
        ];
        for (body, code) in cases {
            let (status, answer) =
                ask(app(None).await, write("POST", "/api/v1/radios", body)).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert_eq!(json::<Error>(&answer).error, code, "{body}");
        }
    }
}
