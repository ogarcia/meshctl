// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Traceroutes: tracing the route from a radio to a node, and the ones traced.

use super::AppState;
use crate::radios::TraceFailure;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, ErrorCode, NewTraceroute, Pace, RadioId, Traceroute};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// How many traceroutes a page has when the client does not say.
const PAGE: u32 = 50;

/// The most a page can have.
const PAGE_MAX: u32 = 500;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list, trace))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "traceroutes");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Which traceroutes to read.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Which {
    /// Only those to this node.
    node: Option<u32>,
    /// How many, 50 if not said and at most 500.
    limit: Option<u32>,
}

/// Traceroutes
///
/// The traceroutes the radio sent, the newest first: those to one node, with
/// `node`, or to any.
#[utoipa::path(
    get,
    path = "/radios/{radio}/traceroutes",
    tag = "traceroutes",
    params(("radio" = RadioId, Path, description = "The radio's id"), Which),
    responses(
        (status = 200, description = "The traceroutes", body = Vec<Traceroute>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn list(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Query(which): Query<Which>,
) -> Response {
    let limit = which.limit.unwrap_or(PAGE).clamp(1, PAGE_MAX);
    match state.radios.traceroutes(radio, which.node, limit).await {
        Ok(Some(traceroutes)) => Json(traceroutes).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Trace a route
///
/// Traces the route from the radio to a node and back. It is answered at once,
/// as `pending`; the answer, or the failure, arrives on the event stream as
/// `traceroute`, within two minutes. A radio traces one route every 30
/// seconds at most, as each one floods the mesh both ways.
#[utoipa::path(
    post,
    path = "/radios/{radio}/traceroutes",
    tag = "traceroutes",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = NewTraceroute,
    responses(
        (status = 202, description = "Sent", body = Traceroute),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Everyone, or the radio itself: `invalid_node`", body = Error),
        (status = 422, description = "A channel the radio does not use: `invalid_channel`", body = Error),
        (status = 429, description = "Too soon after the last one: `too_soon`, with `Retry-After`", body = Error),
    )
)]
async fn trace(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(request): Json<NewTraceroute>,
) -> Response {
    match state
        .radios
        .traceroute(radio, request.node, request.channel)
        .await
    {
        Ok(traceroute) => (StatusCode::ACCEPTED, Json(traceroute)).into_response(),
        Err(TraceFailure::NoSuchRadio) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(TraceFailure::NotConnected) => {
            error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected)
        }
        Err(TraceFailure::Unsupported) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::UnsupportedKind)
        }
        Err(TraceFailure::InvalidNode) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidNode)
        }
        Err(TraceFailure::InvalidChannel) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidChannel)
        }
        Err(TraceFailure::TooSoon { wait }) => (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                header::RETRY_AFTER,
                wait.clamp(1, Pace::Traceroute.interval()).to_string(),
            )],
            Json(Error {
                error: ErrorCode::TooSoon,
            }),
        )
            .into_response(),
        Err(TraceFailure::Failed(err)) => failed(&err),
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{RADIO, app, ask, get, json};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use meshctl_api::{Error, ErrorCode, Traceroute};

    fn post(node: u32) -> Request<Body> {
        Request::post(format!("/api/v1/radios/{RADIO}/traceroutes"))
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"node":{node}}}"#)))
            .unwrap()
    }

    #[tokio::test]
    async fn none_traced_yet() {
        let (status, body) = ask(
            app(None).await,
            get(&format!("/api/v1/radios/{RADIO}/traceroutes")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(json::<Vec<Traceroute>>(&body).is_empty());
    }

    #[tokio::test]
    async fn what_cannot_be_traced_says_why() {
        for node in [u32::MAX, 0x3134_c955] {
            let (status, body) = ask(app(None).await, post(node)).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{node}");
            assert_eq!(json::<Error>(&body).error, ErrorCode::InvalidNode);
        }
        // The test radio is in sync but has no link to write to.
        let (status, body) = ask(app(None).await, post(0x90cd_a275)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json::<Error>(&body).error, ErrorCode::RadioNotConnected);
    }
}
