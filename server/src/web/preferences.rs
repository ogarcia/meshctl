// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The interface's preferences, kept by the daemon so that every browser that
//! opens it finds them as they were left.

use super::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, Preferences};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(read, change))
}

fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "preferences");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Preferences
///
/// How the interface looks and speaks, the defaults until something is set.
#[utoipa::path(
    get,
    path = "/preferences",
    tag = "preferences",
    responses(
        (status = 200, description = "The preferences", body = Preferences),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn read(State(state): State<AppState>) -> Response {
    match state.radios.preferences().await {
        Ok(preferences) => Json(preferences).into_response(),
        Err(err) => failed(&err),
    }
}

/// Change the preferences
///
/// Replaces them. Whatever is left out takes its default. Every client
/// connected hears of it on the event stream, as `preferences`.
#[utoipa::path(
    put,
    path = "/preferences",
    tag = "preferences",
    request_body = Preferences,
    responses(
        (status = 200, description = "The preferences, as kept", body = Preferences),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn change(State(state): State<AppState>, Json(preferences): Json<Preferences>) -> Response {
    match state.radios.set_preferences(preferences).await {
        Ok(()) => Json(preferences).into_response(),
        Err(err) => failed(&err),
    }
}

#[cfg(test)]
mod tests {
    use crate::radios::Change;
    use crate::web::tests::{ask, get, json, radios};
    use crate::web::{AppState, Auth, router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use meshctl_api::{Preferences, Theme};

    #[tokio::test]
    async fn are_kept_and_told() {
        let radios = radios().await;
        let mut changes = radios.subscribe();
        let app = router(AppState {
            auth: Auth::new(None),
            radios: radios.clone(),
        });

        let (status, body) = ask(app.clone(), get("/api/v1/preferences")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json::<Preferences>(&body), Preferences::default());

        let request = Request::put("/api/v1/preferences")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"theme":"dark"}"#))
            .unwrap();
        let (status, _) = ask(app.clone(), request).await;
        assert_eq!(status, StatusCode::OK);

        let dark = Preferences {
            theme: Theme::Dark,
            ..Default::default()
        };
        let (_, body) = ask(app, get("/api/v1/preferences")).await;
        assert_eq!(json::<Preferences>(&body), dark);
        assert!(matches!(changes.recv().await.unwrap(), Change::Preferences(p) if p == dark));
    }
}
