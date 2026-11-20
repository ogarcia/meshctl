// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Talking to the daemon: the API for reading, the event stream for following
//! along, and the token when it asks for one.

use futures_util::{SinkExt, StreamExt};
use gloo_net::http::Request;
use gloo_net::websocket::Message;
use gloo_net::websocket::futures::WebSocket;
use meshctl_api::events::{EVENTS_PATH, Hello, ServerMessage};
use meshctl_api::{API_V1, ErrorCode, Node, Status, TOKEN_HEADER};
use serde::de::DeserializeOwned;

/// Where the token is kept in the browser, once given.
const TOKEN_KEY: &str = "meshctl.token";

/// Why talking to the daemon stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// It asks for a token, and the one we have (if any) is not it.
    Unauthorized,
    /// It cannot be reached, or stopped answering.
    Unreachable,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

pub fn token() -> Option<String> {
    storage()?
        .get_item(TOKEN_KEY)
        .ok()?
        .filter(|t| !t.is_empty())
}

pub fn set_token(token: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(TOKEN_KEY, token);
    }
}

/// Reads something from the API.
pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, Failure> {
    let mut request = Request::get(&format!("{API_V1}{path}"));
    if let Some(token) = token() {
        request = request.header(TOKEN_HEADER, &token);
    }
    let response = request.send().await.map_err(|_| Failure::Unreachable)?;
    match response.status() {
        200 => response.json().await.map_err(|_| Failure::Unreachable),
        401 => Err(Failure::Unauthorized),
        _ => Err(Failure::Unreachable),
    }
}

pub async fn status() -> Result<Status, Failure> {
    get("/status").await
}

pub async fn nodes() -> Result<Vec<Node>, Failure> {
    get("/nodes").await
}

/// The event stream, already past hello and ready.
pub struct Events {
    socket: WebSocket,
}

impl Events {
    pub async fn open() -> Result<Self, Failure> {
        let location = web_sys::window().ok_or(Failure::Unreachable)?.location();
        let scheme = match location.protocol().as_deref() {
            Ok("https:") => "wss",
            _ => "ws",
        };
        let host = location.host().map_err(|_| Failure::Unreachable)?;
        let url = format!("{scheme}://{host}{API_V1}{EVENTS_PATH}");

        let mut socket = WebSocket::open(&url).map_err(|_| Failure::Unreachable)?;
        let hello = serde_json::to_string(&Hello { token: token() }).expect("hello serializes");
        socket
            .send(Message::Text(hello))
            .await
            .map_err(|_| Failure::Unreachable)?;

        let mut events = Self { socket };
        match events.next().await {
            Some(ServerMessage::Ready) => Ok(events),
            Some(ServerMessage::Error {
                error: ErrorCode::Unauthorized,
            }) => Err(Failure::Unauthorized),
            _ => Err(Failure::Unreachable),
        }
    }

    /// The next message, or `None` once the stream is closed.
    pub async fn next(&mut self) -> Option<ServerMessage> {
        loop {
            match self.socket.next().await? {
                Ok(Message::Text(text)) => {
                    if let Ok(message) = serde_json::from_str(&text) {
                        return Some(message);
                    }
                }
                Ok(Message::Bytes(_)) => {}
                Err(_) => return None,
            }
        }
    }
}
