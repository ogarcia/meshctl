// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What only MeshCore radios have: their own settings, which are not
//! Meshtastic's, and their figures, asked for now.

use super::AppState;
use super::settings::refused;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, MeshcoreSettings, MeshcoreStatus, RadioId};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(settings, change_settings))
        .routes(routes!(status))
}

/// MeshCore settings
///
/// A MeshCore radio's own settings, as it last said them. Only while it is
/// connected.
#[utoipa::path(
    get,
    path = "/radios/{radio}/meshcore/settings",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The settings", body = MeshcoreSettings),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
    )
)]
async fn settings(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.meshcore_settings(radio) {
        Ok(settings) => Json(settings).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change MeshCore settings
///
/// Replaces them. Each that changes is sent as the radio takes it, and
/// applied at once, its LoRa settings too, with no reboot; then they are read
/// back from it, and arrive as `synced` on the event stream. A refusal
/// stops there: what was changed before it stays changed.
///
/// - `name`: up to 31 bytes.
/// - `frequency`: 150000 to 2500000 kHz; `bandwidth`: 7000 to 500000 Hz;
///   `spreading_factor`: 5 to 12; `coding_rate`: 5 to 8.
/// - `tx_power`: -9 to `max_tx_power` dBm.
/// - `repeat`: only on firmware that has it, and only on 433.000, 869.495
///   or 918.000 MHz.
#[utoipa::path(
    put,
    path = "/radios/{radio}/meshcore/settings",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = MeshcoreSettings,
    responses(
        (status = 204, description = "Changed, and read back"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not settings the radio can have: `invalid_settings`; not a MeshCore radio: `unsupported_kind`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change_settings(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(wanted): Json<MeshcoreSettings>,
) -> Response {
    match state.radios.change_meshcore_settings(radio, &wanted).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// MeshCore status
///
/// A MeshCore radio's battery, storage and figures, asked of it now: how
/// long it has been on, how busy and noisy it hears the air, what it has
/// sent and received. Each is none if its firmware does not say it.
#[utoipa::path(
    get,
    path = "/radios/{radio}/meshcore/status",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The figures", body = MeshcoreStatus),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn status(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.meshcore_status(radio).await {
        Ok(status) => Json(status).into_response(),
        Err(failure) => refused(failure),
    }
}
