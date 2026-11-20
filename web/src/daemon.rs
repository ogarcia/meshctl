// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Talking to the daemon: the API for reading and changing, the event stream
//! for following along, and the token when it asks for one.

use futures_util::{SinkExt, StreamExt};
use gloo_net::http::{Request, RequestBuilder, Response};
use gloo_net::websocket::Message as Frame;
use gloo_net::websocket::futures::WebSocket;
use meshctl_api::events::{EVENTS_PATH, Hello, ServerMessage};
use meshctl_api::{
    API_V1, Channel, Conversation, Error, ErrorCode, Message, NewMessage, NewTraceroute, Node,
    NodeSettings, NodeSettingsChange, Packet, Peer, Preferences, Radio, RadioId, RadioSettings,
    SectionChange, SettingsApplied, SettingsSection, SightingId, Status, TOKEN_HEADER, Traceroute,
};
use serde::de::DeserializeOwned;

/// Where the token is kept in the browser, once given.
const TOKEN_KEY: &str = "meshctl.token";

/// Where the radio last looked at is kept, to come back to it.
const RADIO_KEY: &str = "meshctl.radio";

/// Why the daemon did not do what it was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// It asks for a token, and the one we have (if any) is not it.
    Unauthorized,
    /// It cannot be reached, or stopped answering, or failed.
    Unreachable,
    /// It refused, and said why.
    Rejected(ErrorCode),
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn stored(key: &str) -> Option<String> {
    storage()?.get_item(key).ok()?.filter(|v| !v.is_empty())
}

fn store(key: &str, value: &str) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, value);
    }
}

pub fn token() -> Option<String> {
    stored(TOKEN_KEY)
}

pub fn set_token(token: &str) {
    store(TOKEN_KEY, token);
}

pub fn last_radio() -> Option<RadioId> {
    stored(RADIO_KEY)?.parse().ok()
}

pub fn set_last_radio(radio: RadioId) {
    store(RADIO_KEY, &radio.to_string());
}

fn with_token(request: RequestBuilder) -> RequestBuilder {
    match token() {
        Some(token) => request.header(TOKEN_HEADER, &token),
        None => request,
    }
}

fn url(path: &str) -> String {
    format!("{API_V1}{path}")
}

/// What an answer means: its body when it is a success, why not otherwise.
async fn answer(response: Result<Response, gloo_net::Error>) -> Result<Response, Failure> {
    let response = response.map_err(|_| Failure::Unreachable)?;
    match response.status() {
        200..=299 => Ok(response),
        401 => Err(Failure::Unauthorized),
        400..=499 => Err(response
            .json::<Error>()
            .await
            .map_or(Failure::Unreachable, |e| Failure::Rejected(e.error))),
        _ => Err(Failure::Unreachable),
    }
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T, Failure> {
    let response = answer(with_token(Request::get(&url(path))).send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

pub async fn radios() -> Result<Vec<Radio>, Failure> {
    get("/radios").await
}

pub async fn status(radio: RadioId) -> Result<Status, Failure> {
    get(&format!("/radios/{radio}/status")).await
}

pub async fn nodes(radio: RadioId) -> Result<Vec<Node>, Failure> {
    get(&format!("/radios/{radio}/nodes")).await
}

pub async fn channels(radio: RadioId) -> Result<Vec<Channel>, Failure> {
    get(&format!("/radios/{radio}/channels")).await
}

pub async fn conversations(radio: RadioId) -> Result<Vec<Conversation>, Failure> {
    get(&format!("/radios/{radio}/conversations")).await
}

/// The newest messages of a conversation, oldest first.
pub async fn messages(radio: RadioId, peer: Peer) -> Result<Vec<Message>, Failure> {
    let which = match peer {
        Peer::Channel { channel } => format!("channel={channel}"),
        Peer::Node { node } => format!("node={node}"),
    };
    get(&format!("/radios/{radio}/messages?{which}&limit=200")).await
}

pub async fn send_message(radio: RadioId, message: &NewMessage) -> Result<Message, Failure> {
    let request = with_token(Request::post(&url(&format!("/radios/{radio}/messages"))))
        .json(message)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

pub async fn settings(radio: RadioId) -> Result<NodeSettings, Failure> {
    get(&format!("/radios/{radio}/settings")).await
}

pub async fn change_settings(
    radio: RadioId,
    change: &NodeSettingsChange,
) -> Result<SettingsApplied, Failure> {
    let request = with_token(Request::put(&url(&format!("/radios/{radio}/settings"))))
        .json(change)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

pub async fn sections(radio: RadioId) -> Result<Vec<SettingsSection>, Failure> {
    get(&format!("/radios/{radio}/sections")).await
}

pub async fn change_section(
    radio: RadioId,
    section: &str,
    values: std::collections::BTreeMap<String, serde_json::Value>,
) -> Result<SettingsApplied, Failure> {
    let request = with_token(Request::put(&url(&format!(
        "/radios/{radio}/sections/{section}"
    ))))
    .json(&SectionChange { values })
    .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

pub async fn traceroutes(radio: RadioId) -> Result<Vec<Traceroute>, Failure> {
    get(&format!("/radios/{radio}/traceroutes?limit=100")).await
}

pub async fn trace(radio: RadioId, node: u32) -> Result<Traceroute, Failure> {
    let request = with_token(Request::post(&url(&format!("/radios/{radio}/traceroutes"))))
        .json(&NewTraceroute { node })
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// The packets a radio heard, the newest first: before one, for older pages.
pub async fn packets(radio: RadioId, before: Option<SightingId>) -> Result<Vec<Packet>, Failure> {
    let before = before.map_or_else(String::new, |id| format!("&before={id}"));
    get(&format!("/radios/{radio}/packets?limit=200{before}")).await
}

pub async fn preferences() -> Result<Preferences, Failure> {
    get("/preferences").await
}

pub async fn save_preferences(preferences: &Preferences) -> Result<Preferences, Failure> {
    let request = with_token(Request::put(&url("/preferences")))
        .json(preferences)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Adds a radio, or changes one when given its id.
pub async fn save_radio(
    radio: Option<RadioId>,
    settings: &RadioSettings,
) -> Result<Radio, Failure> {
    let request = match radio {
        Some(id) => Request::put(&url(&format!("/radios/{id}"))),
        None => Request::post(&url("/radios")),
    };
    let request = with_token(request)
        .json(settings)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

pub async fn remove_radio(radio: RadioId) -> Result<(), Failure> {
    let request = with_token(Request::delete(&url(&format!("/radios/{radio}"))));
    answer(request.send().await).await.map(|_| ())
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
            .send(Frame::Text(hello))
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
                Ok(Frame::Text(text)) => {
                    if let Ok(message) = serde_json::from_str(&text) {
                        return Some(message);
                    }
                }
                Ok(Frame::Bytes(_)) => {}
                Err(_) => return None,
            }
        }
    }
}
