// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios, and what each one knows: its own status, its channels and the
//! nodes it hears.

use super::{AppState, convert};
use crate::radios::{Forgetting, tidy, validate};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    Channel, Contact, Error, ErrorCode, LinkState, MapMaxAge, MapView, Node, NodeCheck,
    NodeFavorite, NodeLatest, NodeTelemetry, Radio, RadioId, RadioOrder, RadioSettings, Status,
};
use meshctl_core::radio::{AdminError, Mesh};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, add))
        .routes(routes!(reorder))
        .routes(routes!(map_view, set_map_view))
        .routes(routes!(set_map_max_age))
        .routes(routes!(get, update, remove))
        .routes(routes!(status))
        .routes(routes!(contact))
        .routes(routes!(node_contact))
        .routes(routes!(channels))
        .routes(routes!(nodes))
        .routes(routes!(node, remove_node))
        .routes(routes!(checks))
        .routes(routes!(latest))
        .routes(routes!(favorite))
        .routes(routes!(telemetry))
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
        (status = 422, description = "Not a radio meshctl can have: `invalid_name`, `invalid_address`, `invalid_badge` or `unsupported_kind`", body = Error),
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

/// Order the radios
///
/// Puts the radios in the order the interface lists them in.
#[utoipa::path(
    put,
    path = "/radios/order",
    tag = "radios",
    request_body = RadioOrder,
    responses(
        (status = 204, description = "Ordered"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 422, description = "The order does not name every radio once: `invalid_order`", body = Error),
    )
)]
async fn reorder(State(state): State<AppState>, Json(order): Json<RadioOrder>) -> Response {
    match state.radios.reorder(&order.radios).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidOrder),
        Err(err) => failed(&err),
    }
}

/// Map view
///
/// Where the interface's map was left for a radio; null if it never was moved.
#[utoipa::path(
    get,
    path = "/radios/{radio}/map-view",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "Where it was left", body = Option<MapView>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn map_view(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.map_view(radio).await {
        Ok(Some(view)) => Json(view).into_response(),
        Ok(None) => not_found(),
        Err(err) => failed(&err),
    }
}

/// Keep the map view
///
/// Keeps where the interface's map was left for a radio, to go back to.
/// Nobody else is told: each browser goes on where it is.
#[utoipa::path(
    put,
    path = "/radios/{radio}/map-view",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = MapView,
    responses(
        (status = 204, description = "Kept"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 422, description = "Not a place on the map: `invalid_map_view`", body = Error),
    )
)]
async fn set_map_view(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(view): Json<MapView>,
) -> Response {
    if !view.is_valid() {
        return error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidMapView);
    }
    match state.radios.set_map_view(radio, &view).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(err) => failed(&err),
    }
}

/// Nodes on the map
///
/// How long ago a radio's nodes have to have been heard to be on its map,
/// in seconds; null for every node. Its own is always there. Every client
/// hears of it, as the radios changed.
#[utoipa::path(
    put,
    path = "/radios/{radio}/map-max-age",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = MapMaxAge,
    responses(
        (status = 204, description = "Kept"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn set_map_max_age(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(age): Json<MapMaxAge>,
) -> Response {
    match state.radios.set_map_max_age(radio, age.max_age).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
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
        (status = 422, description = "Not a radio meshctl can have: `invalid_name`, `invalid_address`, `invalid_badge` or `unsupported_kind`", body = Error),
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
        since: None,
        local: None,
    });
    Json(status).into_response()
}

/// The radio's contact
///
/// A link that shares the radio's own node, as its kind's apps open it: in
/// a QR, or as text. Not yet while the radio has not said who it is.
#[utoipa::path(
    get,
    path = "/radios/{radio}/contact",
    tag = "radios",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The link", body = Contact),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it has not said who it is yet", body = Error),
    )
)]
async fn contact(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    let kind = match state.radios.get(radio).await {
        Ok(Some(radio)) => radio.kind,
        Ok(None) => return Missing::NoSuchRadio.into_response(),
        Err(err) => return Missing::Failed(err).into_response(),
    };
    match state.radios.contact(radio, kind, None) {
        Some(url) => Json(Contact { url }).into_response(),
        None => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
    }
}

/// A node's contact
///
/// A link that shares a node the radio knows of, as the radio's kind's apps
/// open it: in a QR, or as text. Not while the node has not said who it is,
/// or without its public key.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/contact",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    responses(
        (status = 200, description = "The link", body = Contact),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or node, or no key of it yet", body = Error),
    )
)]
async fn node_contact(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
) -> Response {
    let kind = match state.radios.get(radio).await {
        Ok(Some(radio)) => radio.kind,
        Ok(None) => return Missing::NoSuchRadio.into_response(),
        Err(err) => return Missing::Failed(err).into_response(),
    };
    match state.radios.contact(radio, kind, Some(node)) {
        Some(url) => Json(Contact { url }).into_response(),
        None => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
    }
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
            mesh.read(|state| {
                state
                    .channels
                    .values()
                    .map(|channel| convert::channel(state, channel))
                    .collect()
            })
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

/// How much of a node to forget.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Removal {
    /// All kept of it too: what it was asked and the routes traced to it, the
    /// conversation with it, and what it was heard to send, its telemetry
    /// with it. What it wrote in channels stays. Without it, only the radio
    /// forgets it, and meshctl's list with it.
    #[serde(default)]
    everything: bool,
    /// All kept of it, as with `everything`, but by meshctl alone: the radio
    /// is not asked to forget it, and it is read again from the radio as it
    /// has it. Not with `everything`.
    #[serde(default)]
    history: bool,
}

/// Remove node
///
/// Has the radio forget a node: out of its node list until it is heard
/// again. Nothing goes on the air. meshctl keeps it, as `on_radio: false`,
/// which every client hears of as `node`. With `everything`, meshctl
/// forgets it and all it kept of it besides, which every client hears of
/// as `node_removed` and `node_forgotten`; a node the radio is known not to
/// have is not asked of it. With `history`, meshctl alone forgets all it
/// kept of it, and reads it again from the radio, which every client hears
/// of as `node`: on Meshtastic the radio's whole node list is read again.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/nodes/{node}",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = String, Path, description = "The node's id or number", example = "!90cda275"),
        Removal,
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 400, description = "That is neither a node id nor a number, or it is the radio's own node", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows of no such node", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "A kind of radio that does not do this yet: `unsupported_kind`", body = Error),
        (status = 502, description = "The radio would not do it: `radio_refused`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn remove_node(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, String)>,
    Query(removal): Query<Removal>,
) -> Response {
    let Some(num) = node_num(&node) else {
        return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
    };
    let how = match (removal.everything, removal.history) {
        (false, false) => Forgetting::Radio,
        (true, false) => Forgetting::Everything,
        (false, true) => Forgetting::Meshctl,
        (true, true) => return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest),
    };
    match state.radios.remove_node(radio, num, how).await {
        Ok(Ok(Some(()))) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(None) | Err(AdminError::UnknownNode)) => not_found(),
        Ok(Err(AdminError::OwnNode | AdminError::NoPath)) => {
            error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest)
        }
        Ok(Err(AdminError::NotConnected)) => {
            error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected)
        }
        Ok(Err(AdminError::Unsupported)) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::UnsupportedKind)
        }
        Ok(Err(AdminError::NoAnswer)) => {
            error(StatusCode::GATEWAY_TIMEOUT, ErrorCode::RadioDidNotAnswer)
        }
        Ok(Err(AdminError::Refused(reason))) => {
            tracing::warn!(
                radio,
                node = num,
                reason,
                "the radio would not forget a node"
            );
            error(StatusCode::BAD_GATEWAY, ErrorCode::RadioRefused)
        }
        Err(err) => {
            tracing::error!(error = format_args!("{err:#}"), "remove node");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Over how long a node's telemetry is read.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Window {
    /// In seconds back from now: from a quarter of an hour to a month, a
    /// day if not said.
    window: Option<i64>,
}

/// The shortest and longest while a node's telemetry is read over.
const WINDOW_MIN: i64 = 15 * 60;
const WINDOW_MAX: i64 = 31 * 24 * 3600;

/// Node telemetry
///
/// A node's telemetry over a while, as series of points to chart: its
/// device's, its environment sensors' and its power sensors', as it sent
/// them, and the SNR and RSSI it was heard at directly. A long while's are
/// averaged into a few hundred points. `oldest` says how far back there is
/// anything at all.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/telemetry",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = String, Path, description = "The node's id or number", example = "!90cda275"),
        Window,
    ),
    responses(
        (status = 200, description = "The telemetry", body = NodeTelemetry),
        (status = 400, description = "That is neither a node id nor a number", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows of no such node", body = Error),
    )
)]
async fn telemetry(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, String)>,
    Query(window): Query<Window>,
) -> Response {
    let Some(num) = node_num(&node) else {
        return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
    };
    let window = window
        .window
        .unwrap_or(24 * 3600)
        .clamp(WINDOW_MIN, WINDOW_MAX);
    match state.radios.telemetry(radio, num, window).await {
        Ok(Some(telemetry)) => Json(telemetry).into_response(),
        Ok(None) => not_found(),
        Err(err) => {
            tracing::error!(error = format_args!("{err:#}"), "node telemetry");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Favourite node
///
/// Marks a node a favourite of the radio, or no longer, and the apps show it
/// first: a Meshtastic radio keeps it in its node list however long it goes
/// unheard; a MeshCore radio, one of its contacts, never lets it go to make
/// room for another. Nothing goes on the air. Every client hears of it as
/// `node`.
#[utoipa::path(
    put,
    path = "/radios/{radio}/nodes/{node}/favorite",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = String, Path, description = "The node's id or number", example = "!90cda275"),
    ),
    request_body = NodeFavorite,
    responses(
        (status = 204, description = "Done"),
        (status = 400, description = "That is neither a node id nor a number", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows of no such node", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "A kind of radio that does not do this yet: `unsupported_kind`", body = Error),
        (status = 502, description = "The radio would not do it: `radio_refused`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn favorite(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, String)>,
    Json(change): Json<NodeFavorite>,
) -> Response {
    let Some(num) = node_num(&node) else {
        return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
    };
    match state.radios.set_favorite(radio, num, change.favorite).await {
        Ok(Some(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) | Err(AdminError::UnknownNode) => not_found(),
        Err(AdminError::OwnNode | AdminError::NoPath) => {
            error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest)
        }
        Err(AdminError::NotConnected) => error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        Err(AdminError::Unsupported) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::UnsupportedKind)
        }
        Err(AdminError::NoAnswer) => {
            error(StatusCode::GATEWAY_TIMEOUT, ErrorCode::RadioDidNotAnswer)
        }
        Err(AdminError::Refused(_)) => error(StatusCode::BAD_GATEWAY, ErrorCode::RadioRefused),
    }
}

/// Node's latest
///
/// What a node last told of each kind, as the radio heard it: who it is,
/// where, its telemetry of each kind and its neighbours, each the packet it
/// came in, whether sent of its own accord or answering a request.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/latest",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = String, Path, description = "The node's id or number", example = "!90cda275"),
    ),
    responses(
        (status = 200, description = "The latest of each kind", body = NodeLatest),
        (status = 400, description = "That is neither a node id nor a number", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows of no such node", body = Error),
    )
)]
async fn latest(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, String)>,
) -> Response {
    let Some(num) = node_num(&node) else {
        return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
    };
    match state.radios.latest(radio, num).await {
        Ok(Some(latest)) => Json(latest).into_response(),
        Ok(None) => not_found(),
        Err(err) => {
            tracing::error!(error = format_args!("{err:#}"), "node's latest");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Node checks
///
/// What is amiss with a node, found in what it sent: its public key the same
/// as other nodes', more than one key in its latest node info, or more than
/// one hardware model. Empty when nothing is.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/checks",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = String, Path, description = "The node's id or number", example = "!90cda275"),
    ),
    responses(
        (status = 200, description = "The checks", body = Vec<NodeCheck>),
        (status = 400, description = "That is neither a node id nor a number", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows of no such node", body = Error),
    )
)]
async fn checks(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, String)>,
) -> Response {
    let Some(num) = node_num(&node) else {
        return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest);
    };
    match state.radios.checks(radio, num).await {
        Ok(Some(checks)) => Json(checks).into_response(),
        Ok(None) => not_found(),
        Err(err) => {
            tracing::error!(error = format_args!("{err:#}"), "node checks");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A node number from an id, `!` and hex digits, or from the number itself.
pub(super) fn node_num(id: &str) -> Option<u32> {
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
