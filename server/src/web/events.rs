// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The event stream: a WebSocket that carries every change as it happens. How
//! a client talks to it is in [`meshctl_api::events`].

use super::{AppState, convert};
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use meshctl_api::ErrorCode;
use meshctl_api::events::{CLOSE_UNAUTHORIZED, Hello, ServerMessage};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// How long a client has to say hello before it is hung up on.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The close code for a first message that is not a hello: policy violation.
const CLOSE_BAD_HELLO: u16 = 1008;

/// The close code for a daemon that is stopping.
const CLOSE_GOING_AWAY: u16 = 1001;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(events))
}

/// Event stream
///
/// A WebSocket. Send `{}`, or `{"token": "…"}` when the daemon asks for one, as
/// the first message; the daemon answers `{"type": "ready"}` and from then on
/// sends a `ServerMessage` for every change. With the wrong token it answers
/// `{"type": "error", "error": "unauthorized"}` and closes with code 4401.
#[utoipa::path(
    get,
    path = "/events",
    tag = "events",
    responses(
        (status = 101, description = "Switching to the WebSocket; messages are `ServerMessage`"),
        (status = 400, description = "Not a WebSocket request"),
    )
)]
async fn events(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream(socket, state))
}

async fn stream(mut socket: WebSocket, state: AppState) {
    let hello = tokio::time::timeout(HELLO_TIMEOUT, first_text(&mut socket))
        .await
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<Hello>(&text).ok());

    let Some(hello) = hello else {
        refuse(socket, ErrorCode::BadRequest, CLOSE_BAD_HELLO).await;
        return;
    };
    if !state.auth.allows(hello.token.as_deref().map(str::as_bytes)) {
        refuse(socket, ErrorCode::Unauthorized, CLOSE_UNAUTHORIZED).await;
        return;
    }

    // Subscribed before saying ready, so that whatever the client reads after
    // `ready` is either already in what it reads or still to come as an event.
    let mut events = state.radios.subscribe();
    if send(&mut socket, &ServerMessage::Ready).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            () = state.radios.stopped() => {
                let _ = socket.send(Message::Close(Some(CloseFrame {
                    code: CLOSE_GOING_AWAY,
                    reason: "".into(),
                }))).await;
                return;
            }
            event = events.recv() => {
                let message = match event {
                    Ok(change) => convert::change(change),
                    Err(RecvError::Lagged(_)) => ServerMessage::Lagged,
                    Err(RecvError::Closed) => return,
                };
                if send(&mut socket, &message).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                // Nothing a client says after hello means anything yet.
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// The first text message, skipping pings; `None` if the client goes first.
async fn first_text(socket: &mut WebSocket) -> Option<String> {
    loop {
        match socket.recv().await? {
            Ok(Message::Text(text)) => return Some(text.to_string()),
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            _ => return None,
        }
    }
}

async fn send(socket: &mut WebSocket, message: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).expect("server messages serialize");
    socket.send(Message::Text(text.into())).await
}

async fn refuse(mut socket: WebSocket, error: ErrorCode, code: u16) {
    let _ = send(&mut socket, &ServerMessage::Error { error }).await;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: "".into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use crate::web::tests::app;
    use futures_util::{SinkExt, StreamExt};
    use meshctl_api::events::{CLOSE_UNAUTHORIZED, ServerMessage};
    use meshctl_api::{ErrorCode, LinkState};
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message;

    type Client = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// A daemon on a port of its own, and a WebSocket to its event stream.
    async fn connect(token: Option<&str>) -> Client {
        connect_to(app(token).await).await
    }

    async fn connect_to(router: axum::Router) -> Client {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/api/v1/events"))
            .await
            .unwrap();
        client
    }

    #[tokio::test]
    async fn the_stream_ends_when_the_daemon_stops() {
        let radios = crate::web::tests::radios().await;
        let router = crate::web::router(crate::web::AppState {
            auth: crate::web::Auth::new(None),
            radios: radios.clone(),
        });
        let mut client = connect_to(router).await;
        client.send(Message::text("{}")).await.unwrap();
        assert_eq!(message(receive(&mut client).await), ServerMessage::Ready);

        radios.stop().await;
        let Some(Message::Close(Some(frame))) = receive(&mut client).await else {
            panic!("a close frame");
        };
        assert_eq!(u16::from(frame.code), 1001);
    }

    async fn receive(client: &mut Client) -> Option<Message> {
        tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("an answer in time")
            .map(Result::unwrap)
    }

    fn message(raw: Option<Message>) -> ServerMessage {
        let Some(Message::Text(text)) = raw else {
            panic!("a text message, not {raw:?}");
        };
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn hello_then_ready() {
        let mut client = connect(None).await;
        client.send(Message::text("{}")).await.unwrap();
        assert_eq!(message(receive(&mut client).await), ServerMessage::Ready);
    }

    #[tokio::test]
    async fn the_token_goes_in_the_hello() {
        let mut client = connect(Some("s3cret")).await;
        client
            .send(Message::text(r#"{"token":"s3cret"}"#))
            .await
            .unwrap();
        assert_eq!(message(receive(&mut client).await), ServerMessage::Ready);
    }

    #[tokio::test]
    async fn without_it_the_stream_closes() {
        let mut client = connect(Some("s3cret")).await;
        client.send(Message::text("{}")).await.unwrap();
        assert_eq!(
            message(receive(&mut client).await),
            ServerMessage::Error {
                error: ErrorCode::Unauthorized
            }
        );
        let Some(Message::Close(Some(frame))) = receive(&mut client).await else {
            panic!("a close frame");
        };
        assert_eq!(u16::from(frame.code), CLOSE_UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_bad_hello_is_refused() {
        let mut client = connect(None).await;
        client.send(Message::text("hola")).await.unwrap();
        assert_eq!(
            message(receive(&mut client).await),
            ServerMessage::Error {
                error: ErrorCode::BadRequest
            }
        );
    }

    #[test]
    fn link_changes_reach_the_stream_as_codes() {
        let message = super::convert::change(crate::radios::Change::Mesh(
            7,
            meshctl_core::mesh::Event::Link(meshctl_core::mesh::LinkState::Syncing),
        ));
        assert_eq!(
            message,
            ServerMessage::Link {
                radio: 7,
                link: LinkState::Syncing
            }
        );
    }
}
