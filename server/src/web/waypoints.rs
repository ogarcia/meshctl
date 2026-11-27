// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Waypoints: each Meshtastic radio's, heard and shared, and whether its map
//! shows them.

use super::AppState;
use crate::waypoints::WaypointFailure;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::waypoints::{MapWaypoints, NewWaypoint, Waypoint};
use meshctl_api::{Error, ErrorCode, RadioId};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, share))
        .routes(routes!(change, delete))
        .routes(routes!(watch))
        .routes(routes!(set_map_waypoints))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "waypoints");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

fn refused(failure: WaypointFailure) -> Response {
    match failure {
        WaypointFailure::NoSuchRadio | WaypointFailure::NoSuchWaypoint => {
            error(StatusCode::NOT_FOUND, ErrorCode::NotFound)
        }
        WaypointFailure::NotConnected => error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        WaypointFailure::Invalid => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidWaypoint)
        }
        WaypointFailure::Locked => error(StatusCode::FORBIDDEN, ErrorCode::WaypointLocked),
        WaypointFailure::TooSoon => error(StatusCode::TOO_MANY_REQUESTS, ErrorCode::TooSoon),
        WaypointFailure::Failed(err) => failed(&err),
    }
}

/// Waypoints
///
/// The waypoints a radio heard or shared, those not deleted nor expired, the
/// most recently changed first. Only Meshtastic radios have them.
#[utoipa::path(
    get,
    path = "/radios/{radio}/waypoints",
    tag = "waypoints",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The waypoints", body = Vec<Waypoint>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn list(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.waypoints(radio).await {
        Ok(Some(waypoints)) => Json(waypoints).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Share a waypoint
///
/// Shares a waypoint through the radio with everyone on a channel, as the
/// official apps do, with an id of its own. Locked, no other node may change
/// or delete it. It is kept as it went, and every client hears of it. The
/// firmware takes no more than one every ten seconds, from any client:
/// meshctl says so before sending one too soon.
///
/// With `local`, it is meshctl's alone, never sent: on any radio, MeshCore's
/// and one not connected too, with no channel nor lock. Changed with
/// `local` off, it is shared then; one shared cannot be made local.
#[utoipa::path(
    post,
    path = "/radios/{radio}/waypoints",
    tag = "waypoints",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = NewWaypoint,
    responses(
        (status = 201, description = "Shared", body = Waypoint),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a waypoint that can be shared, a channel the radio has not, or not a Meshtastic radio: `invalid_waypoint`", body = Error),
        (status = 429, description = "Another went through the radio less than ten seconds ago: `too_soon`", body = Error),
    )
)]
async fn share(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(new): Json<NewWaypoint>,
) -> Response {
    match state.radios.share_waypoint(radio, new).await {
        Ok(waypoint) => (StatusCode::CREATED, Json(waypoint)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change a waypoint
///
/// Changes a waypoint for everyone: sent again, with its id, as it is now.
/// Not one locked to another node. As sharing one, it waits ten seconds
/// after the last.
#[utoipa::path(
    put,
    path = "/radios/{radio}/waypoints/{id}",
    tag = "waypoints",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = u32, Path, description = "The waypoint's id"),
    ),
    request_body = NewWaypoint,
    responses(
        (status = 200, description = "Changed", body = Waypoint),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "Locked to another node: `waypoint_locked`", body = Error),
        (status = 404, description = "There is no such radio or waypoint", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a waypoint that can be shared: `invalid_waypoint`", body = Error),
        (status = 429, description = "Another went through the radio less than ten seconds ago: `too_soon`", body = Error),
    )
)]
async fn change(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, u32)>,
    Json(new): Json<NewWaypoint>,
) -> Response {
    match state.radios.change_waypoint(radio, id, new).await {
        Ok(waypoint) => Json(waypoint).into_response(),
        Err(failure) => refused(failure),
    }
}

/// For whom a waypoint is deleted.
#[derive(Debug, Deserialize, IntoParams)]
struct Deletion {
    /// For everyone, sent again as deleted, which only whoever may change it
    /// can; or, by default, in meshctl alone.
    #[serde(default)]
    everyone: bool,
}

/// Delete a waypoint
///
/// Deletes a waypoint in meshctl alone, or for everyone (`everyone`): sent
/// again as deleted, which only whoever may change it can, and which waits
/// ten seconds after the last sent. Every client hears it is gone.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/waypoints/{id}",
    tag = "waypoints",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = u32, Path, description = "The waypoint's id"),
        Deletion,
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "Locked to another node: `waypoint_locked`", body = Error),
        (status = 404, description = "There is no such radio or waypoint", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 429, description = "Another went through the radio less than ten seconds ago: `too_soon`", body = Error),
    )
)]
async fn delete(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, u32)>,
    Query(deletion): Query<Deletion>,
) -> Response {
    match state
        .radios
        .delete_waypoint(radio, id, deletion.everyone)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// Whether a waypoint's fence is watched.
#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
struct Watch {
    watched: bool,
}

/// Watch a waypoint's fence
///
/// Whether meshctl tells of nodes going in or out of a waypoint's fence:
/// the radio's own are, as asked when shared; another's, once asked here.
#[utoipa::path(
    put,
    path = "/radios/{radio}/waypoints/{id}/watched",
    tag = "waypoints",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = u32, Path, description = "The waypoint's id"),
    ),
    request_body = Watch,
    responses(
        (status = 204, description = "Kept"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or waypoint", body = Error),
    )
)]
async fn watch(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, u32)>,
    Json(watch): Json<Watch>,
) -> Response {
    match state.radios.watch_waypoint(radio, id, watch.watched).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Waypoints on the map
///
/// Whether a radio's map shows its waypoints. Every client hears of it, as
/// the radios changed.
#[utoipa::path(
    put,
    path = "/radios/{radio}/map-waypoints",
    tag = "waypoints",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = MapWaypoints,
    responses(
        (status = 204, description = "Kept"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn set_map_waypoints(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(shown): Json<MapWaypoints>,
) -> Response {
    match state.radios.set_map_waypoints(radio, shown.shown).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}
