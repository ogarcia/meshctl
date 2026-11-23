// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The chat: each radio's conversations, the messages in them, and sending.

use super::AppState;
use crate::radios::SendFailure;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    Conversation, ConversationRead, Error, ErrorCode, Message, MessageId, MessageRoute, NewMessage,
    Packet, Peer, RadioId,
};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// How many messages a page has when the client does not say.
const PAGE: u32 = 50;

/// The most a page can have.
const PAGE_MAX: u32 = 500;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(conversations))
        .routes(routes!(read))
        .routes(routes!(messages, send))
        .routes(routes!(forget))
        .routes(routes!(message_packet))
        .routes(routes!(message_routes))
        .routes(routes!(resend))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn failed(err: &anyhow::Error) -> Response {
    tracing::error!(error = format_args!("{err:#}"), "chat");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Conversations
///
/// The radio's channels, whether or not anything was said in them yet, then
/// every node it exchanged direct messages with, the most recent first. Each
/// with the newest message in it.
#[utoipa::path(
    get,
    path = "/radios/{radio}/conversations",
    tag = "chat",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The conversations", body = Vec<Conversation>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn conversations(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.conversations(radio).await {
        Ok(Some(conversations)) => Json(conversations).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Read a conversation
///
/// Reads a conversation up to a message, the newest seen: what came before
/// it is no longer unread. A conversation is read on, never back. Every
/// client connected hears of it on the event stream, as `conversation_read`.
#[utoipa::path(
    post,
    path = "/radios/{radio}/conversations/read",
    tag = "chat",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = ConversationRead,
    responses(
        (status = 204, description = "Read"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or channel", body = Error),
        (status = 422, description = "Neither a channel nor a node, or both", body = Error),
    )
)]
async fn read(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(read): Json<ConversationRead>,
) -> Response {
    let peer = match (read.channel, read.node) {
        (Some(channel), None) => Peer::Channel { channel },
        (None, Some(node)) => Peer::Node { node },
        _ => return error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::BadRequest),
    };
    match state.radios.mark_read(radio, peer, read.up_to).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Which messages to read: those of a channel or of a node, one page of them.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Which {
    /// The channel's slot, for a channel's messages.
    channel: Option<u32>,
    /// The node's number, for the direct messages exchanged with it.
    node: Option<u32>,
    /// Only messages older than this one, to read further back.
    before: Option<MessageId>,
    /// How many, 50 if not said and at most 500.
    limit: Option<u32>,
}

/// Messages
///
/// The messages of a channel (`channel`) or exchanged with a node (`node`),
/// the newest page of them, oldest first. Pass the first one's id as `before`
/// for the page before it.
#[utoipa::path(
    get,
    path = "/radios/{radio}/messages",
    tag = "chat",
    params(("radio" = RadioId, Path, description = "The radio's id"), Which),
    responses(
        (status = 200, description = "The messages", body = Vec<Message>),
        (status = 400, description = "Neither a channel nor a node, or both", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
    )
)]
async fn messages(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Query(which): Query<Which>,
) -> Response {
    let peer = match (which.channel, which.node) {
        (Some(channel), None) => Peer::Channel { channel },
        (None, Some(node)) => Peer::Node { node },
        _ => return error(StatusCode::BAD_REQUEST, ErrorCode::BadRequest),
    };
    let limit = which.limit.unwrap_or(PAGE).clamp(1, PAGE_MAX);
    match state
        .radios
        .messages(radio, peer, which.before, limit)
        .await
    {
        Ok(Some(messages)) => Json(messages).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Send a message
///
/// Sends a message to a channel (`channel`) or to a node (`node`), asking for
/// an acknowledgement. It is answered at once, as `sending`; what becomes of
/// it arrives on the event stream as `message_status`.
#[utoipa::path(
    post,
    path = "/radios/{radio}/messages",
    tag = "chat",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = NewMessage,
    responses(
        (status = 202, description = "Handed to the radio", body = Message),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a message that can be sent: `invalid_message`", body = Error),
    )
)]
async fn send(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(message): Json<NewMessage>,
) -> Response {
    match state.radios.send(radio, message).await {
        Ok(message) => (StatusCode::ACCEPTED, Json(message)).into_response(),
        Err(SendFailure::NoSuchRadio) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(SendFailure::NotConnected) => error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        Err(SendFailure::Invalid) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidMessage)
        }
        Err(SendFailure::Failed(err)) => failed(&err),
    }
}

/// Send a message again
///
/// Sends again one of ours on a MeshCore channel, as it first went: the same
/// packet, by the same clock, in the same region. For when no repeater was
/// heard passing it on (`repeaters` is 0), as nothing else says whether it
/// went out. It is kept as it was, and what repeaters pass on now arrives as
/// `message_heard`, as for the first time.
#[utoipa::path(
    post,
    path = "/radios/{radio}/messages/{id}/resend",
    tag = "chat",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = MessageId, Path, description = "The message's id"),
    ),
    responses(
        (status = 202, description = "Handed to the radio", body = Message),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or message", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not ours on a MeshCore channel: `invalid_message`", body = Error),
    )
)]
async fn resend(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, MessageId)>,
) -> Response {
    match state.radios.resend(radio, id).await {
        Ok(Some(message)) => (StatusCode::ACCEPTED, Json(message)).into_response(),
        Ok(None) | Err(SendFailure::NoSuchRadio) => {
            error(StatusCode::NOT_FOUND, ErrorCode::NotFound)
        }
        Err(SendFailure::NotConnected) => error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        Err(SendFailure::Invalid) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidMessage)
        }
        Err(SendFailure::Failed(err)) => failed(&err),
    }
}
/// A message's packet
///
/// The packet that carried a message, as the radio heard or sent it: how
/// many hops it came through out of how many it could, the last relay, its
/// signal. Not for a message whose packet was not kept.
#[utoipa::path(
    get,
    path = "/radios/{radio}/messages/{id}/packet",
    tag = "chat",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = MessageId, Path, description = "The message's id"),
    ),
    responses(
        (status = 200, description = "The packet", body = Packet),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or message, or its packet was not kept", body = Error),
    )
)]
async fn message_packet(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, MessageId)>,
) -> Response {
    match state.radios.message_packet(radio, id).await {
        Ok(Some(packet)) => Json(packet).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// A message's routes
///
/// Each copy of a message a MeshCore radio heard, the first first, and the
/// way it came: the repeaters it went through, from its sender's side, each
/// by its hash and the nodes the radio knows that it may be; its signal, and
/// the region it came in. Ours too, as repeaters repeated them. Empty for a
/// message none was heard of.
#[utoipa::path(
    get,
    path = "/radios/{radio}/messages/{id}/routes",
    tag = "chat",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = MessageId, Path, description = "The message's id"),
    ),
    responses(
        (status = 200, description = "The routes", body = Vec<MessageRoute>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or message", body = Error),
    )
)]
async fn message_routes(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, MessageId)>,
) -> Response {
    match state.radios.routes(radio, id).await {
        Ok(Some(routes)) => Json(routes).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

/// Forget a message
///
/// Forgets a message the radio keeps, and the reactions to it: only in
/// meshctl, as what went out on the mesh cannot be taken back. Every client
/// connected hears of it on the event stream, as `messages_forgotten`.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/messages/{id}",
    tag = "chat",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("id" = MessageId, Path, description = "The message's id"),
    ),
    responses(
        (status = 204, description = "Forgotten"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio or message", body = Error),
    )
)]
async fn forget(
    State(state): State<AppState>,
    Path((radio, id)): Path<(RadioId, MessageId)>,
) -> Response {
    match state.radios.forget_message(radio, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        Err(err) => failed(&err),
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{RADIO, app, ask, get, json};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use meshctl_api::{Conversation, Error, ErrorCode, Message, Peer};

    fn post(body: &str) -> Request<Body> {
        Request::post(format!("/api/v1/radios/{RADIO}/messages"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn every_channel_is_a_conversation_from_the_start() {
        let (status, body) = ask(
            app(None).await,
            get(&format!("/api/v1/radios/{RADIO}/conversations")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let conversations: Vec<Conversation> = json(&body);
        let peers: Vec<_> = conversations.iter().map(|c| c.peer).collect();
        assert_eq!(
            peers,
            [Peer::Channel { channel: 0 }, Peer::Channel { channel: 4 }]
        );
        assert!(conversations.iter().all(|c| c.last.is_none()));
    }

    #[tokio::test]
    async fn an_empty_conversation_reads_empty() {
        let (status, body) = ask(
            app(None).await,
            get(&format!("/api/v1/radios/{RADIO}/messages?channel=4")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(json::<Vec<Message>>(&body).is_empty());
    }

    #[tokio::test]
    async fn reading_needs_one_peer() {
        for query in ["", "?channel=0&node=7"] {
            let (status, _) = ask(
                app(None).await,
                get(&format!("/api/v1/radios/{RADIO}/messages{query}")),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        }
    }

    #[tokio::test]
    async fn what_cannot_be_sent_says_why() {
        // The test radio is in sync but has no link to write to.
        let (status, body) = ask(app(None).await, post(r#"{"text":"hola","channel":4}"#)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json::<Error>(&body).error, ErrorCode::RadioNotConnected);

        for body in [
            r#"{"text":"hola","channel":6}"#,
            r#"{"text":"hola"}"#,
            r#"{"text":"hola","channel":4,"node":7}"#,
            // Reactions: to nothing, and not an emoji.
            r#"{"text":"👍","channel":4,"emoji":true}"#,
            r#"{"text":"hola","channel":4,"reply_id":7,"emoji":true}"#,
        ] {
            let (status, answer) = ask(app(None).await, post(body)).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert_eq!(json::<Error>(&answer).error, ErrorCode::InvalidMessage);
        }
    }

    #[tokio::test]
    async fn only_a_message_kept_is_sent_again() {
        let resend = |radio: &str| {
            Request::post(format!("/api/v1/radios/{radio}/messages/7/resend"))
                .body(Body::empty())
                .unwrap()
        };
        for radio in [RADIO.to_string(), "99".to_owned()] {
            let (status, body) = ask(app(None).await, resend(&radio)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{radio}");
            assert_eq!(json::<Error>(&body).error, ErrorCode::NotFound);
        }
    }
}
