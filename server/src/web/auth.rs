// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Who may use the API, decided in one place.
//!
//! Today there is one answer: whoever has the token, or everyone when there is
//! no token. Anything finer (a header from a trusted proxy, sessions, users)
//! is another way of reaching that answer here, and the handlers do not change.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, ErrorCode, TOKEN_HEADER};
use std::sync::Arc;

/// What the API asks for.
#[derive(Debug, Clone)]
pub struct Auth {
    token: Option<Arc<str>>,
    read_only: bool,
}

impl Auth {
    pub fn new(token: Option<String>) -> Self {
        Self {
            token: token.map(Into::into),
            read_only: false,
        }
    }

    /// Makes every non-GET/HEAD API operation unavailable while keeping the
    /// read-only API and event stream usable.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Whether whoever gave this token, or none, may use the API.
    pub fn allows(&self, given: Option<&[u8]>) -> bool {
        match (&self.token, given) {
            (None, _) => true,
            (Some(expected), Some(given)) => same(given, expected.as_bytes()),
            (Some(_), None) => false,
        }
    }

    pub fn allows_method(&self, method: &Method) -> bool {
        !self.read_only || matches!(*method, Method::GET | Method::HEAD)
    }
}

/// Lets the request through, or answers 401.
pub async fn check(State(auth): State<Auth>, request: Request, next: Next) -> Response {
    let given = request
        .headers()
        .get(TOKEN_HEADER)
        .map(|value| value.as_bytes());
    if auth.allows(given) {
        if auth.allows_method(request.method()) {
            next.run(request).await
        } else {
            (
                StatusCode::FORBIDDEN,
                Json(Error {
                    error: ErrorCode::ReadOnly,
                }),
            )
                .into_response()
        }
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(Error {
                error: ErrorCode::Unauthorized,
            }),
        )
            .into_response()
    }
}

/// Compares in a time that depends on the length alone, so that how long a
/// refusal takes says nothing about how much of a guess was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use crate::web::{
        Auth,
        tests::{app, app_read_only, ask, get},
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use meshctl_api::TOKEN_HEADER;

    async fn guarded() -> axum::Router {
        app(Some("s3cret")).await
    }

    fn with_token(token: &str) -> Request<Body> {
        Request::get("/api/v1/version")
            .header(TOKEN_HEADER, token)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn without_a_token_configured_the_api_is_open() {
        let (status, _) = ask(app(None).await, get("/api/v1/version")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn with_one_configured_it_is_asked_for() {
        let (status, body) = ask(guarded().await, get("/api/v1/version")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, br#"{"error":"unauthorized"}"#);
    }

    #[tokio::test]
    async fn the_wrong_one_is_refused() {
        let (status, _) = ask(guarded().await, with_token("s3creT")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_right_one_is_let_through() {
        let (status, _) = ask(guarded().await, with_token("s3cret")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_reference_and_the_interface_need_none() {
        let (status, _) = ask(guarded().await, get("/api/openapi.json")).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn read_only_refuses_a_mutating_request() {
        let request = Request::post("/api/v1/radios/1/messages")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"text":"hello","channel":0}"#))
            .unwrap();
        let (status, body) = ask(app_read_only().await, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, br#"{"error":"read_only"}"#);
    }

    #[test]
    fn read_only_allows_reads_but_not_writes() {
        let auth = Auth::new(None).read_only(true);
        assert!(auth.allows_method(&axum::http::Method::GET));
        assert!(auth.allows_method(&axum::http::Method::HEAD));
        assert!(!auth.allows_method(&axum::http::Method::POST));
        assert!(!auth.allows_method(&axum::http::Method::PUT));
        assert!(!auth.allows_method(&axum::http::Method::DELETE));
    }
}
