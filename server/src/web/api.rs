// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Version 1 of the API.

use axum::Json;
use meshctl_api::Version;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter {
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
        (status = 401, description = "The access token is missing or wrong", body = meshctl_api::Error),
    )
)]
async fn version() -> Json<Version> {
    Json(Version {
        name: "meshctl".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{ask, get};
    use crate::web::{Auth, router};
    use axum::http::StatusCode;
    use meshctl_api::Version;

    #[tokio::test]
    async fn says_what_is_answering() {
        let (status, body) = ask(router(Auth::new(None)), get("/api/v1/version")).await;
        assert_eq!(status, StatusCode::OK);
        let version: Version = serde_json::from_slice(&body).unwrap();
        assert_eq!(version.name, "meshctl");
    }
}
