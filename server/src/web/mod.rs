// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! HTTP: the API, its reference, and the interface.
//!
//! The API lives under `/api/v1`, and every route is declared through utoipa's
//! router, so the reference at `/api/docs` is built from the same calls that
//! register the handlers and cannot drift from them.
//!
//! Reading is `GET` and changing is anything else. That is what lets a proxy
//! in front open the read-only part to everyone and keep the rest for whoever
//! it authenticates, with a rule of its own and nothing here.

mod api;
mod assets;
mod auth;

pub use auth::Auth;

use axum::Router;
use axum::response::Html;
use axum::routing::get;
use meshctl_api::API_V1;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_scalar::Scalar;

/// Where the reference is served.
const DOCS_PATH: &str = "/api/docs";

/// Where the document behind it is served, for clients and code generators.
const OPENAPI_PATH: &str = "/api/openapi.json";

/// Scalar's page, pinned so that it does not change under us from one day to
/// the next.
const SCALAR_HTML: &str = r#"<!doctype html>
<html>
  <head>
    <title>meshctl API</title>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
  </head>
  <body>
    <script id="api-reference" type="application/json">$spec</script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.76.4"></script>
  </body>
</html>
"#;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "meshctl",
        description = "The API behind meshctl's panel, for any other client too.",
        license(name = "GPL-3.0-or-later", identifier = "GPL-3.0-or-later"),
    ),
    tags((name = "meta", description = "What is answering")),
    components(schemas(meshctl_api::Error, meshctl_api::ErrorCode)),
)]
struct Reference;

/// Every route meshctl answers.
pub fn router(auth: Auth) -> Router {
    let (v1, reference) = OpenApiRouter::with_openapi(Reference::openapi())
        .nest(API_V1, api::router())
        .split_for_parts();

    let spec = serde_json::to_string(&reference).expect("the reference serializes");
    let docs = Scalar::new(reference).custom_html(SCALAR_HTML).to_html();

    Router::new()
        .merge(v1.layer(axum::middleware::from_fn_with_state(auth, auth::check)))
        .route(DOCS_PATH, get(move || async move { Html(docs) }))
        .route(
            OPENAPI_PATH,
            get(move || async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    spec,
                )
            }),
        )
        .fallback(assets::serve)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Asks the router, the way axum would, and returns the status and body.
    pub async fn ask(router: Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, body.to_vec())
    }

    pub fn get(uri: &str) -> Request<Body> {
        Request::get(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn the_reference_documents_the_api() {
        let (status, body) = ask(router(Auth::new(None)), get(OPENAPI_PATH)).await;
        assert_eq!(status, StatusCode::OK);
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(spec["paths"]["/api/v1/version"]["get"].is_object());
    }

    #[tokio::test]
    async fn the_reference_has_a_page() {
        let (status, body) = ask(router(Auth::new(None)), get(DOCS_PATH)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8(body).unwrap().contains("/api/v1/version"));
    }

    #[tokio::test]
    async fn an_unknown_api_address_answers_in_json() {
        let (status, body) = ask(router(Auth::new(None)), get("/api/v1/nowhere")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, br#"{"error":"not_found"}"#);
    }
}
