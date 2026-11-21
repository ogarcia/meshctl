// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What a radio keeps secret, apart from everything else: nothing else the
//! API answers carries it, so a client holds a secret only once it asks for
//! that one.

use super::AppState;
use super::settings::refused;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use meshctl_api::{ChannelSecret, Error, ErrorCode, RadioId};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(channel_key))
}

/// Channel key
///
/// A channel's key itself, in base64 as the apps show it. Asked for on its
/// own, one channel at a time: the settings only say what kind of key each
/// channel has. Not to be kept by whoever asks, nor by anything in between;
/// meshctl logs each time it is handed out. Only while the radio is
/// connected.
#[utoipa::path(
    get,
    path = "/radios/{radio}/secrets/channels/{index}",
    tag = "secrets",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("index" = u32, Path, description = "The channel's slot, 0 to 7"),
    ),
    responses(
        (status = 200, description = "The key", body = ChannelSecret),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or no such slot", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
    )
)]
async fn channel_key(
    State(state): State<AppState>,
    Path((radio, index)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.channel_secret(radio, index) {
        Ok(Some(secret)) => ([(header::CACHE_CONTROL, "no-store")], Json(secret)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(Error {
                error: ErrorCode::NotFound,
            }),
        )
            .into_response(),
        Err(failure) => refused(failure),
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{RADIO, app, ask, get, json};
    use axum::http::StatusCode;
    use meshctl_api::{Error, ErrorCode};

    #[tokio::test]
    async fn a_key_needs_the_token() {
        let (status, _) = ask(
            app(Some("s3cret")).await,
            get(&format!("/api/v1/radios/{RADIO}/secrets/channels/0")),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_key_needs_the_radio_to_have_sent_its_settings() {
        // The test radio is in sync, but sent no settings.
        let (status, body) = ask(
            app(None).await,
            get(&format!("/api/v1/radios/{RADIO}/secrets/channels/0")),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json::<Error>(&body).error, ErrorCode::RadioNotConnected);
    }

    #[tokio::test]
    async fn there_is_no_key_of_a_radio_there_is_not() {
        let (status, body) = ask(
            app(None).await,
            get("/api/v1/radios/999/secrets/channels/0"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json::<Error>(&body).error, ErrorCode::NotFound);
    }
}
