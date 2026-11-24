// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Requests: asking a node for what it tells on request, and the answers.

use super::AppState;
use crate::radios::AskFailure;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use meshctl_api::{Error, ErrorCode, Forgotten, NewRequest, RadioId, Request, RequestId, Waits};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// How many requests a page has when the client does not say.
const PAGE: u32 = 50;

/// The most a page can have.
const PAGE_MAX: u32 = 500;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, ask, forget))
        .routes(routes!(waits))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "requests");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Which requests to read.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Which {
    /// Only those to this node.
    node: Option<u32>,
    /// Only those sent before this one, for the next page.
    before: Option<RequestId>,
    /// How many, 50 if not said and at most 500.
    limit: Option<u32>,
}

/// Requests
///
/// What nodes were asked for through the radio, the newest first, with what
/// each answered.
#[utoipa::path(
    get,
    path = "/radios/{radio}/requests",
    tag = "requests",
    params(("radio" = RadioId, Path, description = "The radio's id"), Which),
    responses(
        (status = 200, description = "The requests", body = Vec<Request>),
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
    match state
        .radios
        .requests(radio, which.node, which.before, limit)
        .await
    {
        Ok(Some(requests)) => Json(requests).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Ask a node
///
/// Asks a node for something it tells on request: who it is, where it is,
/// its telemetry or its neighbours. It is answered at once, as `pending`; the
/// answer arrives on the event stream as `request`, within two minutes.
///
/// The node answers on the channel asked on, which it has to have too. Nodes
/// keep from answering too often: their node info once a minute, and their
/// position or neighbours once in three minutes, whoever asks. A request it
/// keeps from answering ends in `TIMEOUT`; one it has nothing to answer
/// with, as environment telemetry without sensors, in `NO_RESPONSE`.
///
/// The radio itself sends positions and telemetry at a pace, whichever node
/// they are for, and drops what comes sooner: so does meshctl, with
/// `too_soon` and how long to wait, as the waits tell beforehand.
#[utoipa::path(
    post,
    path = "/radios/{radio}/requests",
    tag = "requests",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = NewRequest,
    responses(
        (status = 202, description = "Sent", body = Request),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Everyone, or the radio itself: `invalid_node`; a channel it does not use: `invalid_channel`; a kind of radio that does not do this yet, or a request it does not make: `unsupported_kind`; a node a MeshCore radio knows no path to: `no_path`", body = Error),
        (status = 429, description = "The same is still waiting for its answer, or the radio sent the like too recently: `too_soon`, with `Retry-After` for the latter", body = Error),
    )
)]
async fn ask(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(new): Json<NewRequest>,
) -> Response {
    match state.radios.request(radio, new).await {
        Ok(request) => (StatusCode::ACCEPTED, Json(request)).into_response(),
        Err(AskFailure::NoSuchRadio) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(AskFailure::NotConnected) => error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        Err(AskFailure::Unsupported) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::UnsupportedKind)
        }
        Err(AskFailure::InvalidNode) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidNode)
        }
        Err(AskFailure::InvalidChannel) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidChannel)
        }
        Err(AskFailure::Pending) => error(StatusCode::TOO_MANY_REQUESTS, ErrorCode::TooSoon),
        Err(AskFailure::NoPath) => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::NoPath),
        Err(AskFailure::TooSoon { wait }) => (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, wait.max(1).to_string())],
            Json(Error {
                error: ErrorCode::TooSoon,
            }),
        )
            .into_response(),
        Err(AskFailure::Login(failure)) => super::meshcore::login_failed(failure),
        Err(AskFailure::Failed(err)) => failed(&err),
    }
}

/// Whose history to forget.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Whose {
    /// The node's number.
    node: u32,
}

/// Forget requests
///
/// Forgets what a node was asked for through the radio, but what still
/// waits for its answer. The packets that answered stay, as what the node
/// told: its page shows the same.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/requests",
    tag = "requests",
    params(("radio" = RadioId, Path, description = "The radio's id"), Whose),
    responses(
        (status = 200, description = "How many were forgotten", body = Forgotten),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn forget(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Query(whose): Query<Whose>,
) -> Response {
    match state.radios.forget_requests(radio, whose.node).await {
        Ok(Some(count)) => Json(Forgotten { count }).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Waits
///
/// How long until the radio sends each paced kind of thing again, in
/// seconds: positions, telemetry of any kind and traceroutes, whichever node
/// they are for. Asking sooner gets `too_soon`.
#[utoipa::path(
    get,
    path = "/radios/{radio}/waits",
    tag = "requests",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The waits", body = Waits),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn waits(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.waits(radio).await {
        Ok(Some(waits)) => Json(waits).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{app, ask, get, json};
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use meshctl_api::{Error, ErrorCode};

    fn post(body: &str) -> Request<Body> {
        Request::post("/api/v1/radios/1/requests")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn a_request_needs_a_node_a_channel_and_a_radio_connected() {
        // The radio of the tests is never connected: it is stored disabled.
        let (status, body) = ask(
            app(None).await,
            post(r#"{"node":2429396597,"kind":"position"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json::<Error>(&body).error, ErrorCode::RadioNotConnected);

        let (status, body) = ask(
            app(None).await,
            post(r#"{"node":4294967295,"kind":"neighbors"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json::<Error>(&body).error, ErrorCode::InvalidNode);

        let (status, _) = ask(app(None).await, get("/api/v1/radios/1/requests")).await;
        assert_eq!(status, StatusCode::OK);
    }
}
