// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios plugged in where the daemon runs, to pick one from.

use super::AppState;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, ErrorCode, PluggedAsk, PluggedRadio};
use meshctl_core::link::plugged::plugged;
use meshctl_core::link::usb;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(ask))
}

/// Radios plugged in
///
/// The radios plugged in where the daemon runs, now: its serial ports that
/// are USB ones, or, within an app that opens them, as on Android, its
/// devices by USB. Each with what to give as its address.
#[utoipa::path(
    get,
    path = "/devices",
    tag = "radios",
    responses(
        (status = 200, description = "The radios plugged in", body = Vec<PluggedRadio>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn list() -> Response {
    let plugged: Vec<PluggedRadio> = plugged()
        .await
        .into_iter()
        .map(|p| PluggedRadio {
            address: p.address,
            ids: p.ids,
            maker: p.maker,
            product: p.product,
            chip: p.chip.map(str::to_owned),
            ready: p.ready,
        })
        .collect();
    Json(plugged).into_response()
}

/// Ask leave to use a radio
///
/// Within an app that opens radios by USB, as on Android, asks the user for
/// leave to use one, by its address; answers whether meshctl has it, which
/// it may only once the user says. Elsewhere there is no one to ask.
#[utoipa::path(
    post,
    path = "/devices/ask",
    tag = "radios",
    request_body = PluggedAsk,
    responses(
        (status = 200, description = "Whether meshctl may use it now", body = bool),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "Nothing to ask here: `not_found`", body = Error),
    )
)]
async fn ask(Json(asked): Json<PluggedAsk>) -> Response {
    match usb::ask(&asked.address).await {
        Some(allowed) => Json(allowed).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(Error {
                error: ErrorCode::NotFound,
            }),
        )
            .into_response(),
    }
}
