// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The automatic replies: what meshctl answers on its own, and where.

use super::AppState;
use crate::store::Keeping;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    AutoReply, AutoReplyId, AutoReplyLimits, AutoReplyOrder, AutoReplySettings, ChannelBusy, Error,
    ErrorCode,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, add))
        .routes(routes!(reorder))
        .routes(routes!(limits, set_limits))
        .routes(routes!(busy))
        .routes(routes!(change, remove))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "automatic replies");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

fn kept(kept: anyhow::Result<Keeping>, created: bool) -> Response {
    match kept {
        Ok(Keeping::Kept(reply)) if created => (StatusCode::CREATED, Json(reply)).into_response(),
        Ok(Keeping::Kept(reply)) => Json(reply).into_response(),
        Ok(Keeping::Invalid) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InvalidAutoReply,
        ),
        Ok(Keeping::NotFound) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Automatic replies
///
/// Every automatic reply, in the order they are tried in: a message two of
/// them would answer is answered by the first.
#[utoipa::path(
    get,
    path = "/auto-replies",
    tag = "auto-replies",
    responses(
        (status = 200, description = "The automatic replies", body = Vec<AutoReply>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn list(State(state): State<AppState>) -> Response {
    match state.radios.auto_replies().await {
        Ok(replies) => Json(replies).into_response(),
        Err(err) => failed(&err),
    }
}

/// Add an automatic reply
///
/// The last of them. A channel among its places is given by its slot.
#[utoipa::path(
    post,
    path = "/auto-replies",
    tag = "auto-replies",
    request_body = AutoReplySettings,
    responses(
        (status = 201, description = "Added", body = AutoReply),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 422, description = "Not an automatic reply: `invalid_auto_reply`", body = Error),
    )
)]
async fn add(State(state): State<AppState>, Json(settings): Json<AutoReplySettings>) -> Response {
    kept(state.radios.add_auto_reply(&settings).await, true)
}

/// Change an automatic reply
///
/// Replaces it, places and all. A channel among its places is given by its
/// slot; one in no slot now, by its name, as it was told, to keep it.
#[utoipa::path(
    put,
    path = "/auto-replies/{auto_reply}",
    tag = "auto-replies",
    params(("auto_reply" = AutoReplyId, Path, description = "The automatic reply's id")),
    request_body = AutoReplySettings,
    responses(
        (status = 200, description = "Changed", body = AutoReply),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such automatic reply", body = Error),
        (status = 422, description = "Not an automatic reply: `invalid_auto_reply`", body = Error),
    )
)]
async fn change(
    State(state): State<AppState>,
    Path(id): Path<AutoReplyId>,
    Json(settings): Json<AutoReplySettings>,
) -> Response {
    kept(state.radios.set_auto_reply(id, &settings).await, false)
}

/// Remove an automatic reply
#[utoipa::path(
    delete,
    path = "/auto-replies/{auto_reply}",
    tag = "auto-replies",
    params(("auto_reply" = AutoReplyId, Path, description = "The automatic reply's id")),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such automatic reply", body = Error),
    )
)]
async fn remove(State(state): State<AppState>, Path(id): Path<AutoReplyId>) -> Response {
    match state.radios.remove_auto_reply(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Order the automatic replies
///
/// Puts them in the order they are tried in.
#[utoipa::path(
    put,
    path = "/auto-replies/order",
    tag = "auto-replies",
    request_body = AutoReplyOrder,
    responses(
        (status = 204, description = "Ordered"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 422, description = "The order does not name every one once: `invalid_order`", body = Error),
    )
)]
async fn reorder(State(state): State<AppState>, Json(order): Json<AutoReplyOrder>) -> Response {
    match state.radios.set_auto_reply_order(&order.auto_replies).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidOrder),
        Err(err) => failed(&err),
    }
}

/// Their limits
///
/// What holds every automatic reply back, whatever it is set to.
#[utoipa::path(
    get,
    path = "/auto-replies/limits",
    tag = "auto-replies",
    responses(
        (status = 200, description = "The limits", body = AutoReplyLimits),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn limits(State(state): State<AppState>) -> Response {
    match state.radios.auto_reply_limits().await {
        Ok(limits) => Json(limits).into_response(),
        Err(err) => failed(&err),
    }
}

/// Change their limits
#[utoipa::path(
    put,
    path = "/auto-replies/limits",
    tag = "auto-replies",
    request_body = AutoReplyLimits,
    responses(
        (status = 200, description = "The limits, as kept", body = AutoReplyLimits),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 422, description = "Not limits: a threshold past 1 to 100, `invalid_auto_reply`", body = Error),
    )
)]
async fn set_limits(
    State(state): State<AppState>,
    Json(limits): Json<AutoReplyLimits>,
) -> Response {
    if !limits.is_valid() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InvalidAutoReply,
        );
    }
    match state.radios.set_auto_reply_limits(&limits).await {
        Ok(()) => Json(limits).into_response(),
        Err(err) => failed(&err),
    }
}

/// How busy the channels are
///
/// How busy each radio hears its channel, as it last said, if lately: what
/// the limit on a busy channel goes by.
#[utoipa::path(
    get,
    path = "/auto-replies/busy",
    tag = "auto-replies",
    responses(
        (status = 200, description = "Each radio's, in their order", body = Vec<ChannelBusy>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
    )
)]
async fn busy(State(state): State<AppState>) -> Response {
    match state.radios.channels_busy().await {
        Ok(busy) => Json(busy).into_response(),
        Err(err) => failed(&err),
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{ask, get, json, radios};
    use crate::web::{AppState, Auth, router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use meshctl_api::{
        AutoReply, AutoReplyLimits, AutoReplyPlace, ChannelBusy, RadioKind, RadioSettings,
    };

    fn send(method: &str, uri: &str, body: String) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn their_limits_are_kept() {
        let app = router(AppState {
            auth: Auth::new(None),
            radios: radios().await,
        });
        let (_, body) = ask(app.clone(), get("/api/v1/auto-replies/limits")).await;
        assert_eq!(json::<AutoReplyLimits>(&body), AutoReplyLimits::default());
        let uri = "/api/v1/auto-replies/limits";
        let (status, _) = ask(
            app.clone(),
            send("PUT", uri, r#"{"busy_channel":null}"#.into()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = ask(app.clone(), get(uri)).await;
        assert_eq!(json::<AutoReplyLimits>(&body).busy_channel, None);
        for not in [r#"{"busy_channel":0}"#, r#"{"busy_channel":101}"#] {
            let (status, _) = ask(app.clone(), send("PUT", uri, not.into())).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{not}");
        }
        // Nothing said yet: none known.
        let (_, body) = ask(app, get("/api/v1/auto-replies/busy")).await;
        let busy: Vec<ChannelBusy> = json(&body);
        assert!(busy.iter().all(|b| b.percent.is_none()) && !busy.is_empty());
    }

    #[tokio::test]
    async fn are_added_changed_ordered_and_removed() {
        let radios = radios().await;
        let radio = radios
            .add(RadioSettings {
                name: "Casa".into(),
                kind: RadioKind::Meshtastic,
                address: "192.0.2.1".into(),
                enabled: false,
                shown: true,
                badge: Default::default(),
            })
            .await
            .unwrap()
            .id;
        let app = router(AppState {
            auth: Auth::new(None),
            radios,
        });
        let ping = format!(
            r#"{{"name":"Ping","enabled":true,"triggers":["ping","test"],"matching":"whole",
            "react":true,"reply":"🤖 Pong, {{HOPS}} 🦘","direct_reply":null,
            "places":[{{"kind":"directs","radio":{radio}}}]}}"#
        );
        let (status, body) = ask(app.clone(), send("POST", "/api/v1/auto-replies", ping)).await;
        assert_eq!(status, StatusCode::CREATED);
        let first: AutoReply = json(&body);
        assert_eq!(first.settings.places, [AutoReplyPlace::Directs { radio }]);

        // Nothing to answer with, a place on no radio, a channel the radio
        // has not: none is one.
        for not in [
            r#"{"name":"Hola","enabled":true,"triggers":["hola"],"matching":"word","react":false,"reply":null,"direct_reply":null,"places":[]}"#.to_owned(),
            r#"{"name":"Hola","enabled":true,"triggers":["hola"],"matching":"word","react":true,"reply":null,"direct_reply":null,"places":[{"kind":"directs","radio":999}]}"#.to_owned(),
            format!(r#"{{"name":"Hola","enabled":true,"triggers":["hola"],"matching":"word","react":true,"reply":null,"direct_reply":null,"places":[{{"kind":"channel","radio":{radio},"channel":0}}]}}"#),
            r#"{"name":" ","enabled":true,"triggers":["hola"],"matching":"word","react":true,"reply":null,"direct_reply":null,"places":[]}"#.to_owned(),
            r#"{"name":"Hola","enabled":true,"triggers":["!"],"matching":"word","react":true,"reply":null,"direct_reply":null,"places":[]}"#.to_owned(),
        ] {
            let (status, _) = ask(app.clone(), send("POST", "/api/v1/auto-replies", not.clone())).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{not}");
        }

        let hola = format!(
            r#"{{"name":"Hola","enabled":true,"triggers":["hola"],"matching":"word",
            "react":false,"reply":"¡Hola, {{LONG_NAME}}!","direct_reply":null,
            "places":[{{"kind":"node","radio":{radio},"node":1234}}]}}"#
        );
        let (_, body) = ask(app.clone(), send("POST", "/api/v1/auto-replies", hola)).await;
        let second: AutoReply = json(&body);

        let order = format!(r#"{{"auto_replies":[{},{}]}}"#, second.id, first.id);
        let (status, _) = ask(
            app.clone(),
            send("PUT", "/api/v1/auto-replies/order", order),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let bad = format!(r#"{{"auto_replies":[{}]}}"#, second.id);
        let (status, _) = ask(app.clone(), send("PUT", "/api/v1/auto-replies/order", bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (_, body) = ask(app.clone(), get("/api/v1/auto-replies")).await;
        let listed: Vec<AutoReply> = json(&body);
        assert_eq!(
            listed.iter().map(|r| r.id).collect::<Vec<_>>(),
            [second.id, first.id]
        );

        let mut changed = first.settings.clone();
        changed.enabled = false;
        changed.places.clear();
        let uri = format!("/api/v1/auto-replies/{}", first.id);
        let (status, body) = ask(
            app.clone(),
            send("PUT", &uri, serde_json::to_string(&changed).unwrap()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json::<AutoReply>(&body).settings, changed);

        let (status, _) = ask(app.clone(), send("DELETE", &uri, String::new())).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = ask(app.clone(), send("DELETE", &uri, String::new())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = ask(
            app,
            send("PUT", &uri, serde_json::to_string(&changed).unwrap()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
