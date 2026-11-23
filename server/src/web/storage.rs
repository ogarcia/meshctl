// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl keeps, for how long, and cleaning away what is left behind.

use super::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{Cleaned, Error, ErrorCode, Retention, Storage};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(storage))
        .routes(routes!(retention, change_retention))
        .routes(routes!(clean))
        .routes(routes!(forget_gone_channels))
        .routes(routes!(forget_gone_directs))
}

fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "storage");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Storage
///
/// What meshctl keeps, how much room it takes, and what was left behind:
/// the conversations of channels no radio has, and with nodes no longer
/// kept.
#[utoipa::path(
    get,
    path = "/storage",
    tag = "storage",
    responses(
        (status = 200, description = "What is kept", body = Storage),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn storage(State(state): State<AppState>) -> Response {
    match state.radios.storage().await {
        Ok(storage) => Json(storage).into_response(),
        Err(err) => failed(&err),
    }
}

/// Retention
///
/// How long what meshctl keeps is kept, the defaults until something is set.
#[utoipa::path(
    get,
    path = "/storage/retention",
    tag = "storage",
    responses(
        (status = 200, description = "How long each is kept", body = Retention),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn retention(State(state): State<AppState>) -> Response {
    match state.radios.retention().await {
        Ok(retention) => Json(retention).into_response(),
        Err(err) => failed(&err),
    }
}

/// Change the retention
///
/// Replaces it, whatever is left out taking its default, and cleans away at
/// once what is now kept for longer. Every client connected hears of it on
/// the event stream, as `retention`, and of what went as `cleaned`.
#[utoipa::path(
    put,
    path = "/storage/retention",
    tag = "storage",
    request_body = Retention,
    responses(
        (status = 200, description = "How long each is kept, as kept", body = Retention),
        (status = 400, description = "A time under a day, or over a hundred years", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn change_retention(
    State(state): State<AppState>,
    Json(retention): Json<Retention>,
) -> Response {
    if !retention.valid() {
        return (
            StatusCode::BAD_REQUEST,
            Json(Error {
                error: ErrorCode::BadRequest,
            }),
        )
            .into_response();
    }
    match state.radios.set_retention(retention).await {
        Ok(()) => Json(retention).into_response(),
        Err(err) => failed(&err),
    }
}

/// Clean now
///
/// Cleans away what is kept for longer than it is kept, as meshctl does once
/// a day on its own.
#[utoipa::path(
    post,
    path = "/storage/clean",
    tag = "storage",
    responses(
        (status = 200, description = "What went", body = Cleaned),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn clean(State(state): State<AppState>) -> Response {
    match state.radios.clean().await {
        Ok(cleaned) => Json(cleaned).into_response(),
        Err(err) => failed(&err),
    }
}

/// Forget the channels gone
///
/// Forgets the conversations of channels no radio has any more, for good.
#[utoipa::path(
    post,
    path = "/storage/gone-channels",
    tag = "storage",
    responses(
        (status = 200, description = "What went", body = Cleaned),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn forget_gone_channels(State(state): State<AppState>) -> Response {
    match state.radios.forget_gone_channels().await {
        Ok(cleaned) => Json(cleaned).into_response(),
        Err(err) => failed(&err),
    }
}

/// Forget the directs gone
///
/// Forgets the direct conversations with nodes meshctl no longer keeps, for
/// good.
#[utoipa::path(
    post,
    path = "/storage/gone-directs",
    tag = "storage",
    responses(
        (status = 200, description = "What went", body = Cleaned),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn forget_gone_directs(State(state): State<AppState>) -> Response {
    match state.radios.forget_gone_directs().await {
        Ok(cleaned) => Json(cleaned).into_response(),
        Err(err) => failed(&err),
    }
}
