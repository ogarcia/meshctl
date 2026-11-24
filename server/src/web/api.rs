// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Version 1 of the API: what is answering. The radios and what they hear are
//! in [`super::radios`], the event stream in [`super::events`].

use super::AppState;
use axum::Json;
use meshctl_api::{Error, Version};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(version))
}

/// Version
///
/// The program answering and its version.
#[utoipa::path(
    get,
    path = "/version",
    tag = "meta",
    responses(
        (status = 200, description = "What is answering", body = Version),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn version() -> Json<Version> {
    Json(Version {
        name: "meshctl".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        revision: Some(env!("MESHCTL_GIT_REVISION"))
            .filter(|revision| !revision.is_empty())
            .map(Into::into),
    })
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{app, ask, get};
    use axum::http::StatusCode;
    use meshctl_api::Version;

    #[tokio::test]
    async fn says_what_is_answering() {
        let (status, body) = ask(app(None).await, get("/api/v1/version")).await;
        assert_eq!(status, StatusCode::OK);
        let version: Version = serde_json::from_slice(&body).unwrap();
        assert_eq!(version.name, "meshctl");
    }
}
