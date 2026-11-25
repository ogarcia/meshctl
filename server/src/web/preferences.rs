// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The interface's preferences, kept by the daemon so that every browser that
//! opens it finds them as they were left.

use super::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{AppSettings, EmojiUsed, Error, ErrorCode, Preferences};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(read, change))
        .routes(routes!(emojis, use_emoji))
        .routes(routes!(app, set_app))
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

/// The emojis most used
///
/// Those picked in the interface, the most used first and, among as used,
/// the latest: what it offers first.
#[utoipa::path(
    get,
    path = "/preferences/emojis",
    tag = "preferences",
    responses(
        (status = 200, description = "The emojis", body = Vec<String>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn emojis(State(state): State<AppState>) -> Response {
    match state.radios.emojis().await {
        Ok(emojis) => Json(emojis).into_response(),
        Err(err) => failed(&err),
    }
}

/// Count an emoji used
///
/// One more time an emoji was picked. Gives the emojis most used, as they
/// are now.
#[utoipa::path(
    post,
    path = "/preferences/emojis",
    tag = "preferences",
    request_body = EmojiUsed,
    responses(
        (status = 200, description = "The emojis, the most used first", body = Vec<String>),
        (status = 400, description = "That is not an emoji", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn use_emoji(State(state): State<AppState>, Json(used): Json<EmojiUsed>) -> Response {
    // An emoji: a few characters, beyond plain text (a keycap has a digit,
    // and more), none a space or a control.
    let emoji = used.emoji.trim();
    if emoji.is_empty()
        || emoji.len() > 32
        || emoji.is_ascii()
        || emoji.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(Error {
                error: ErrorCode::BadRequest,
            }),
        )
            .into_response();
    }
    match state.radios.use_emoji(emoji).await {
        Ok(emojis) => Json(emojis).into_response(),
        Err(err) => failed(&err),
    }
}

/// The app's settings
///
/// What the app meshctl runs within lets be set, as it is now: none of
/// what it does not have, and all none when it runs on its own.
#[utoipa::path(
    get,
    path = "/preferences/app",
    tag = "preferences",
    responses(
        (status = 200, description = "The app's settings", body = AppSettings),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn app(State(state): State<AppState>) -> Response {
    match state.radios.app_settings().await {
        Ok(settings) => Json(settings).into_response(),
        Err(err) => failed(&err),
    }
}

/// Change the app's settings
///
/// Those given; the rest as they are.
#[utoipa::path(
    put,
    path = "/preferences/app",
    tag = "preferences",
    request_body = AppSettings,
    responses(
        (status = 200, description = "The app's settings, as they are now", body = AppSettings),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "meshctl runs within no app: `not_found`", body = Error),
    )
)]
async fn set_app(State(state): State<AppState>, Json(settings): Json<AppSettings>) -> Response {
    match state.radios.set_app_settings(settings).await {
        Ok(true) => app(State(state)).await,
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(Error {
                error: ErrorCode::NotFound,
            }),
        )
            .into_response(),
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
    async fn emojis_are_counted_and_only_emojis() {
        let radios = radios().await;
        let app = router(AppState {
            auth: Auth::new(None),
            radios,
        });
        let post = |body: &'static str| {
            Request::post("/api/v1/preferences/emojis")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        let (status, body) = ask(app.clone(), post(r#"{"emoji":"📡"}"#)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json::<Vec<String>>(&body), ["📡"]);
        for not in [
            r#"{"emoji":""}"#,
            r#"{"emoji":"hola"}"#,
            r#"{"emoji":"👍 👍"}"#,
        ] {
            let (status, _) = ask(app.clone(), post(not)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{not}");
        }
        let (_, body) = ask(app, get("/api/v1/preferences/emojis")).await;
        assert_eq!(json::<Vec<String>>(&body), ["📡"]);
    }

    #[tokio::test]
    async fn the_app_s_are_only_within_one() {
        use meshctl_api::AppSettings;
        use std::sync::{Arc, Mutex};

        struct Told(Mutex<Vec<bool>>);
        impl crate::Host for Told {
            fn keep_running(&self, on: bool) {
                self.0.lock().unwrap().push(on);
            }
        }

        let radios = radios().await;
        let app = router(AppState {
            auth: Auth::new(None),
            radios: radios.clone(),
        });
        let put = |body: &'static str| {
            Request::put("/api/v1/preferences/app")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        // On its own: nothing to set.
        let (_, body) = ask(app.clone(), get("/api/v1/preferences/app")).await;
        assert_eq!(json::<AppSettings>(&body), AppSettings::default());
        let (status, _) = ask(app.clone(), put(r#"{"background":false}"#)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Within an app: told on at first, then off as set, and kept.
        let told = Arc::new(Told(Mutex::new(Vec::new())));
        radios.set_host(told.clone()).await;
        let (_, body) = ask(app.clone(), get("/api/v1/preferences/app")).await;
        assert_eq!(json::<AppSettings>(&body).background, Some(true));
        let (status, body) = ask(app.clone(), put(r#"{"background":false}"#)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json::<AppSettings>(&body).background, Some(false));
        assert_eq!(*told.0.lock().unwrap(), [true, false]);
    }

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
