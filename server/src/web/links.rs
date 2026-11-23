// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Links to add nodes and channels from: what one holds, and adding it.

use super::AppState;
use super::settings::refused;
use crate::radios::LinkFailure;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, ErrorCode, Link, LinkContents, RadioId};
use meshctl_core::radio::AdminError;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(read))
        .routes(routes!(add))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn failed(failure: LinkFailure) -> Response {
    match failure {
        LinkFailure::Invalid => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidLink),
        LinkFailure::NoFreeSlots => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::NoFreeSlots),
        LinkFailure::Settings(failure) => refused(failure),
        LinkFailure::Admin(AdminError::NotConnected) => {
            error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected)
        }
        LinkFailure::Admin(AdminError::NoAnswer) => {
            error(StatusCode::GATEWAY_TIMEOUT, ErrorCode::RadioDidNotAnswer)
        }
        LinkFailure::Admin(AdminError::Refused(reason)) => {
            tracing::debug!(%reason, "contact refused");
            error(StatusCode::BAD_GATEWAY, ErrorCode::RadioRefused)
        }
        LinkFailure::Admin(AdminError::Unsupported) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::UnsupportedKind)
        }
        LinkFailure::Admin(AdminError::OwnNode | AdminError::UnknownNode) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidLink)
        }
    }
}

/// Read a link
///
/// What a link holds, against what the radio has and knows, before anything
/// is added from it: a Meshtastic contact (`https://meshtastic.org/v/#…`),
/// with whether it brings its key and whether the radio knows the node; or
/// channels (`https://meshtastic.org/e/#…`), each with whether the radio has
/// it, how many slots are free, and whether the link's LoRa settings are not
/// the radio's. Nothing changes. Only while the radio is connected.
#[utoipa::path(
    post,
    path = "/radios/{radio}/links/read",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = Link,
    responses(
        (status = 200, description = "What it holds", body = LinkContents),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a link meshctl reads: `invalid_link`; a MeshCore radio: `unsupported_kind`", body = Error),
    )
)]
async fn read(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(link): Json<Link>,
) -> Response {
    match state.radios.read_link(radio, &link.url) {
        Ok(contents) => Json(contents).into_response(),
        Err(failure) => failed(failure),
    }
}

/// Add from a link
///
/// Adds what a link holds to the radio: a contact, which the radio keeps as
/// a node with its key, once it says it does; or channels. With `mode` at
/// `add`, each to a free slot, those the radio has already left out; at
/// `replace`, in place of every one, the first the primary, the slots left
/// over emptied, and with `lora` the link's LoRa settings too, on which the
/// radio reboots. Nothing else changes.
#[utoipa::path(
    post,
    path = "/radios/{radio}/links",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = Link,
    responses(
        (status = 204, description = "Added"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a link meshctl reads: `invalid_link`; channels that do not fit: `no_free_slots`; a MeshCore radio: `unsupported_kind`", body = Error),
        (status = 502, description = "The radio would not: `radio_refused`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn add(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(link): Json<Link>,
) -> Response {
    match state.radios.add_link(radio, &link).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => failed(failure),
    }
}
