// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Talking to the daemon: the API for reading and changing, the event stream
//! for following along, and the token when it asks for one.

use crate::traces::{Traceroute, TracerouteId};
use futures_util::{SinkExt, StreamExt};
use gloo_net::http::{Request, RequestBuilder, Response};
use gloo_net::websocket::Message as Frame;
use gloo_net::websocket::futures::WebSocket;
use meshctl_api::events::{EVENTS_PATH, Hello, ServerMessage};
use meshctl_api::{
    API_V1, Channel, ChannelHistory, ChannelOrder, ChannelSecret, Conversation, Error, ErrorCode,
    MapView, Message, NewMessage, Node, NodeSettings, NodeSettingsChange, Packet, PacketFilter,
    Peer, Preferences, Radio, RadioId, RadioOrder, RadioSettings, SectionChange, SectionSecret,
    SettingsApplied, SettingsSection, SightingId, Status, TOKEN_HEADER,
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

/// Whether the daemon takes this token: asked of it before it is kept,
/// with a call that needs one. Unreachable when it cannot be told.
pub async fn check_token(token: &str) -> Result<bool, Failure> {
    let response = Request::get(&url("/version"))
        .header(TOKEN_HEADER, token)
        .send()
        .await
        .map_err(|_| Failure::Unreachable)?;
    match response.status() {
        200..=299 => Ok(true),
        401 => Ok(false),
        _ => Err(Failure::Unreachable),
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
        // The radio would not, or did not answer: the daemon says which.
        502 | 504 => Err(response
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

/// The link that shares the radio's own node, as its kind's apps open it.
pub async fn contact(radio: RadioId) -> Result<meshctl_api::Contact, Failure> {
    get(&format!("/radios/{radio}/contact")).await
}

/// The link that shares a node the radio knows of, as its kind's apps open
/// it.
pub async fn node_contact(radio: RadioId, node: u32) -> Result<meshctl_api::Contact, Failure> {
    get(&format!("/radios/{radio}/nodes/{node}/contact")).await
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

/// What a link holds, against what the radio has.
pub async fn read_link(
    radio: RadioId,
    address: &str,
) -> Result<meshctl_api::LinkContents, Failure> {
    let link = meshctl_api::Link {
        url: address.to_owned(),
        mode: meshctl_api::LinkMode::Add,
        lora: false,
    };
    let request = with_token(Request::post(&url(&format!("/radios/{radio}/links/read"))))
        .json(&link)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Adds what a link holds to the radio, or for channels puts them in place
/// of its own, as the link says.
pub async fn add_link(radio: RadioId, link: &meshctl_api::Link) -> Result<(), Failure> {
    let request = with_token(Request::post(&url(&format!("/radios/{radio}/links"))))
        .json(link)
        .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// Reads a conversation up to a message, never back.
pub async fn mark_read(
    radio: RadioId,
    peer: meshctl_api::Peer,
    up_to: meshctl_api::MessageId,
) -> Result<(), Failure> {
    let (channel, node) = match peer {
        meshctl_api::Peer::Channel { channel } => (Some(channel), None),
        meshctl_api::Peer::Node { node } => (None, Some(node)),
    };
    let read = meshctl_api::ConversationRead {
        channel,
        node,
        up_to,
    };
    let request = with_token(Request::post(&url(&format!(
        "/radios/{radio}/conversations/read"
    ))))
    .json(&read)
    .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// The packet that carried a message, as the radio heard or sent it.
pub async fn message_packet(
    radio: RadioId,
    id: meshctl_api::MessageId,
) -> Result<meshctl_api::Packet, Failure> {
    get(&format!("/radios/{radio}/messages/{id}/packet")).await
}

/// The ways a message came: each copy heard through a MeshCore radio.
pub async fn message_routes(
    radio: RadioId,
    id: meshctl_api::MessageId,
) -> Result<Vec<meshctl_api::MessageRoute>, Failure> {
    get(&format!("/radios/{radio}/messages/{id}/routes")).await
}

/// The ways a packet a MeshCore radio heard came, each copy.
pub async fn packet_routes(
    radio: RadioId,
    id: meshctl_api::SightingId,
) -> Result<Vec<meshctl_api::MessageRoute>, Failure> {
    get(&format!("/radios/{radio}/packets/{id}/routes")).await
}

/// Forgets a message the radio keeps, and the reactions to it: only in
/// meshctl, not on the mesh.
pub async fn forget_message(radio: RadioId, id: meshctl_api::MessageId) -> Result<(), Failure> {
    let request = with_token(Request::delete(&url(&format!(
        "/radios/{radio}/messages/{id}"
    ))));
    answer(request.send().await).await.map(|_| ())
}

/// The radio's login to a room.
pub async fn node_login(radio: RadioId, node: u32) -> Result<meshctl_api::NodeLogin, Failure> {
    get(&format!("/radios/{radio}/nodes/{node}/login")).await
}

/// Logs the radio in to a room: with a password, or with the one kept.
pub async fn log_in_node(
    radio: RadioId,
    node: u32,
    password: Option<String>,
) -> Result<meshctl_api::NodeLogin, Failure> {
    let request = with_token(Request::post(&url(&format!(
        "/radios/{radio}/nodes/{node}/login"
    ))))
    .json(&meshctl_api::NewNodeLogin { password })
    .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Sends a command to a node's console, as its admin.
pub async fn node_command(radio: RadioId, node: u32, command: &str) -> Result<(), Failure> {
    let request = with_token(Request::post(&url(&format!(
        "/radios/{radio}/nodes/{node}/console"
    ))))
    .json(&meshctl_api::NewCommand {
        command: command.to_owned(),
    })
    .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// Forgets the password kept for a room.
pub async fn forget_node_password(
    radio: RadioId,
    node: u32,
) -> Result<meshctl_api::NodeLogin, Failure> {
    let request = with_token(Request::delete(&url(&format!(
        "/radios/{radio}/nodes/{node}/login"
    ))));
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Sends again one of ours on a MeshCore channel no repeater was heard
/// passing on: the same packet, kept as it was.
pub async fn resend_message(radio: RadioId, id: meshctl_api::MessageId) -> Result<(), Failure> {
    let request = with_token(Request::post(&url(&format!(
        "/radios/{radio}/messages/{id}/resend"
    ))));
    answer(request.send().await).await.map(|_| ())
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

/// A secret of a section of the settings: asked for only to be shown, and
/// not kept.
pub async fn section_secret(
    radio: RadioId,
    section: &str,
    field: &str,
) -> Result<SectionSecret, Failure> {
    get(&format!(
        "/radios/{radio}/secrets/sections/{section}/{field}"
    ))
    .await
}

/// A channel's key itself: asked for only to be shown, and not kept.
pub async fn channel_secret(radio: RadioId, index: u32) -> Result<ChannelSecret, Failure> {
    get(&format!("/radios/{radio}/secrets/channels/{index}")).await
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

/// A radio's latest traceroutes, to any node: requests of their kind.
pub async fn traceroutes(radio: RadioId) -> Result<Vec<Traceroute>, Failure> {
    let requests: Vec<meshctl_api::Request> = get(&format!(
        "/radios/{radio}/requests?kind=traceroute&limit=100"
    ))
    .await?;
    Ok(requests.iter().filter_map(Traceroute::of).collect())
}

/// Traces the route to a node, on a channel: a request of its kind.
pub async fn trace(radio: RadioId, node: u32, channel: u32) -> Result<Traceroute, Failure> {
    let new = meshctl_api::NewRequest {
        node,
        kind: meshctl_api::RequestKind::Traceroute,
        channel,
        seconds: None,
    };
    let request = ask(radio, &new).await?;
    Traceroute::of(&request).ok_or(Failure::Unreachable)
}

/// What a node was asked for through a radio, the newest first.
pub async fn requests(radio: RadioId, node: u32) -> Result<Vec<meshctl_api::Request>, Failure> {
    get(&format!("/radios/{radio}/requests?node={node}&limit=50")).await
}

/// Marks a node a favourite of a radio, or no longer.
pub async fn set_favorite(radio: RadioId, node: u32, favorite: bool) -> Result<(), Failure> {
    let request = with_token(Request::put(&url(&format!(
        "/radios/{radio}/nodes/{node}/favorite"
    ))))
    .json(&meshctl_api::NodeFavorite { favorite })
    .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// Has the radio forget a node, and with `everything`, meshctl all it kept
/// of it besides; once the radio says it did.
pub async fn remove_node(radio: RadioId, node: u32, everything: bool) -> Result<(), Failure> {
    let request = with_token(Request::delete(&url(&format!(
        "/radios/{radio}/nodes/{node}?everything={everything}"
    ))));
    answer(request.send().await).await.map(|_| ())
}

/// A node's telemetry over the latest so many seconds.
pub async fn node_telemetry(
    radio: RadioId,
    node: u32,
    window: u32,
) -> Result<meshctl_api::NodeTelemetry, Failure> {
    get(&format!(
        "/radios/{radio}/nodes/{node}/telemetry?window={window}"
    ))
    .await
}

/// What a node last told a radio of each kind.
pub async fn node_latest(radio: RadioId, node: u32) -> Result<meshctl_api::NodeLatest, Failure> {
    get(&format!("/radios/{radio}/nodes/{node}/latest")).await
}

/// What is amiss with a node a radio knows of.
pub async fn node_checks(
    radio: RadioId,
    node: u32,
) -> Result<Vec<meshctl_api::NodeCheck>, Failure> {
    get(&format!("/radios/{radio}/nodes/{node}/checks")).await
}

/// A page of the traceroutes to a node, the newest first: those before one,
/// for older pages.
pub async fn traceroutes_to(
    radio: RadioId,
    node: u32,
    before: Option<TracerouteId>,
) -> Result<Vec<Traceroute>, Failure> {
    let before = before.map_or_else(String::new, |id| format!("&before={id}"));
    let requests: Vec<meshctl_api::Request> = get(&format!(
        "/radios/{radio}/requests?node={node}&kind=traceroute&limit={TRACES_PAGE}{before}"
    ))
    .await?;
    Ok(requests.iter().filter_map(Traceroute::of).collect())
}

/// A page of what a node was asked for, the newest first: those before one,
/// for older pages.
pub async fn requests_page(
    radio: RadioId,
    node: u32,
    before: Option<meshctl_api::RequestId>,
) -> Result<Vec<meshctl_api::Request>, Failure> {
    let before = before.map_or_else(String::new, |id| format!("&before={id}"));
    get(&format!(
        "/radios/{radio}/requests?node={node}&limit={REQUESTS_PAGE}{before}"
    ))
    .await
}

/// Forgets what a node was asked for, but what still waits.
pub async fn forget_requests(radio: RadioId, node: u32) -> Result<(), Failure> {
    let request = with_token(Request::delete(&url(&format!(
        "/radios/{radio}/requests?node={node}"
    ))));
    answer(request.send().await).await.map(|_| ())
}

/// Forgets the routes traced to a node but the latest, and those waiting.
pub async fn forget_traceroutes(radio: RadioId, node: u32) -> Result<(), Failure> {
    let request = with_token(Request::delete(&url(&format!(
        "/radios/{radio}/requests?node={node}&kind=traceroute"
    ))));
    answer(request.send().await).await.map(|_| ())
}

/// How many requests a page of a node's has.
pub const REQUESTS_PAGE: usize = 20;

/// How many traceroutes a page of a node's has.
pub const TRACES_PAGE: usize = 20;

/// How long until a radio sends each paced kind of thing again.
pub async fn waits(radio: RadioId) -> Result<meshctl_api::Waits, Failure> {
    get(&format!("/radios/{radio}/waits")).await
}

pub async fn ask(
    radio: RadioId,
    new: &meshctl_api::NewRequest,
) -> Result<meshctl_api::Request, Failure> {
    let request = with_token(Request::post(&url(&format!("/radios/{radio}/requests"))))
        .json(new)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// How many packets a page of them has: fewer, and there are no more.
pub const PACKETS_PAGE: usize = 200;

/// The packets a radio heard that a filter lets through, the newest first:
/// before one, for older pages.
pub async fn packets(
    radio: RadioId,
    before: Option<SightingId>,
    filter: &PacketFilter,
) -> Result<Vec<Packet>, Failure> {
    // An enum as the query has it: its name, as JSON gives it.
    let name = |value: serde_json::Value| value.as_str().unwrap_or_default().to_owned();
    let mut query = format!("limit={PACKETS_PAGE}");
    let mut add = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            query.push_str(&format!("&{key}={value}"));
        }
    };
    add("before", before.map(|id| id.to_string()));
    add(
        "kind",
        filter
            .kind
            .and_then(|k| serde_json::to_value(k).ok())
            .map(name),
    );
    add("channel", filter.channel.map(|c| c.to_string()));
    add(
        "reach",
        filter
            .reach
            .and_then(|r| serde_json::to_value(r).ok())
            .map(name),
    );
    add("node", filter.node.map(|n| n.to_string()));
    add("not_from", filter.not_from.map(|n| n.to_string()));
    get(&format!("/radios/{radio}/packets?{query}")).await
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

/// The emojis picked in the interface, the most used first.
pub async fn emojis() -> Result<Vec<String>, Failure> {
    get("/preferences/emojis").await
}

/// Counts an emoji picked; the emojis most used, as they are now.
pub async fn use_emoji(emoji: &str) -> Result<Vec<String>, Failure> {
    let used = meshctl_api::EmojiUsed {
        emoji: emoji.to_owned(),
    };
    let request = with_token(Request::post(&url("/preferences/emojis")))
        .json(&used)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// A MeshCore radio's own settings.
pub async fn meshcore_settings(radio: RadioId) -> Result<meshctl_api::MeshcoreSettings, Failure> {
    get(&format!("/radios/{radio}/meshcore/settings")).await
}

/// Changes a MeshCore radio's own settings, whole.
pub async fn set_meshcore_settings(
    radio: RadioId,
    settings: &meshctl_api::MeshcoreSettings,
) -> Result<(), Failure> {
    let request = with_token(Request::put(&url(&format!(
        "/radios/{radio}/meshcore/settings"
    ))))
    .json(settings)
    .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// A MeshCore radio's figures, asked of it now.
pub async fn meshcore_status(radio: RadioId) -> Result<meshctl_api::MeshcoreStatus, Failure> {
    get(&format!("/radios/{radio}/meshcore/status")).await
}

/// A MeshCore radio's channels, by their kind.
pub async fn meshcore_channels(radio: RadioId) -> Result<meshctl_api::MeshcoreChannels, Failure> {
    get(&format!("/radios/{radio}/meshcore/channels")).await
}

/// Puts a channel in a MeshCore radio's slot.
pub async fn set_meshcore_channel(
    radio: RadioId,
    index: u32,
    change: &meshctl_api::MeshcoreChannelChange,
) -> Result<(), Failure> {
    let request = with_token(Request::put(&url(&format!(
        "/radios/{radio}/meshcore/channels/{index}"
    ))))
    .json(change)
    .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// Puts a MeshCore radio's channels in another order, by their slots now.
pub async fn reorder_meshcore_channels(radio: RadioId, order: &[u32]) -> Result<(), Failure> {
    let order = ChannelOrder {
        order: order.to_vec(),
    };
    let request = with_token(Request::put(&url(&format!(
        "/radios/{radio}/meshcore/channels/order"
    ))))
    .json(&order)
    .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// Keeps a MeshCore channel's region, or none for the radio's.
pub async fn set_channel_scope(
    radio: RadioId,
    index: u32,
    scope: Option<String>,
) -> Result<(), Failure> {
    let request = with_token(Request::put(&url(&format!(
        "/radios/{radio}/meshcore/channels/{index}/scope"
    ))))
    .json(&meshctl_api::MeshcoreChannelScope { scope })
    .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// Empties a MeshCore radio's slot.
pub async fn remove_meshcore_channel(radio: RadioId, index: u32) -> Result<(), Failure> {
    let request = with_token(Request::delete(&url(&format!(
        "/radios/{radio}/meshcore/channels/{index}"
    ))));
    answer(request.send().await).await.map(|_| ())
}

/// A MeshCore channel's link, its secret in it.
pub async fn meshcore_channel_link(
    radio: RadioId,
    index: u32,
) -> Result<meshctl_api::Contact, Failure> {
    get(&format!("/radios/{radio}/meshcore/channels/{index}/link")).await
}

/// The regions a MeshCore radio's repeaters said they repeat on.
pub async fn meshcore_regions(radio: RadioId) -> Result<meshctl_api::MeshcoreRegions, Failure> {
    get(&format!("/radios/{radio}/meshcore/regions")).await
}

/// Has a MeshCore radio ask its repeaters their regions.
pub async fn search_regions(radio: RadioId) -> Result<meshctl_api::MeshcoreRegions, Failure> {
    let request = with_token(Request::post(&url(&format!(
        "/radios/{radio}/meshcore/regions/search"
    ))));
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Has a MeshCore radio forget its path to a node.
pub async fn reset_path(radio: RadioId, node: u32) -> Result<(), Failure> {
    let request = with_token(Request::post(&url(&format!(
        "/radios/{radio}/nodes/{node}/reset-path"
    ))));
    answer(request.send().await).await.map(|_| ())
}

/// Has a MeshCore radio say who it is: nearby, or to the whole mesh.
pub async fn advert(radio: RadioId, flood: bool) -> Result<(), Failure> {
    let request = with_token(Request::post(&url(&format!("/radios/{radio}/advert"))))
        .json(&meshctl_api::Advert { flood })
        .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// What meshctl keeps, and how much room it takes.
pub async fn kept() -> Result<meshctl_api::Storage, Failure> {
    get("/storage").await
}

/// How long what meshctl keeps is kept.
pub async fn retention() -> Result<meshctl_api::Retention, Failure> {
    get("/storage/retention").await
}

/// Changes how long what meshctl keeps is kept; the daemon cleans at once.
pub async fn set_retention(
    retention: &meshctl_api::Retention,
) -> Result<meshctl_api::Retention, Failure> {
    let request = with_token(Request::put(&url("/storage/retention")))
        .json(retention)
        .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Asks the daemon for a cleaning without a body, at `path`; what went.
async fn cleaning(path: &str) -> Result<meshctl_api::Cleaned, Failure> {
    let request = with_token(Request::post(&url(path)));
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Cleans away now what is kept for longer than it is kept.
pub async fn clean() -> Result<meshctl_api::Cleaned, Failure> {
    cleaning("/storage/clean").await
}

/// Forgets the conversations of channels no radio has any more.
pub async fn forget_gone_channels() -> Result<meshctl_api::Cleaned, Failure> {
    cleaning("/storage/gone-channels").await
}

/// Forgets the direct conversations with nodes meshctl no longer keeps.
pub async fn forget_gone_directs() -> Result<meshctl_api::Cleaned, Failure> {
    cleaning("/storage/gone-directs").await
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

/// Where the map was left for a radio, if it ever was moved.
pub async fn map_view(radio: RadioId) -> Result<Option<MapView>, Failure> {
    get(&format!("/radios/{radio}/map-view")).await
}

/// Keeps where the map was left for a radio.
pub async fn save_map_view(radio: RadioId, view: &MapView) -> Result<(), Failure> {
    let request = with_token(Request::put(&url(&format!("/radios/{radio}/map-view"))))
        .json(view)
        .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

/// Puts the radios in this order, which names each of them once.
pub async fn order_radios(radios: &[RadioId]) -> Result<(), Failure> {
    let order = RadioOrder {
        radios: radios.to_vec(),
    };
    let request = with_token(Request::put(&url("/radios/order")))
        .json(&order)
        .map_err(|_| Failure::Unreachable)?;
    answer(request.send().await).await.map(|_| ())
}

pub async fn remove_radio(radio: RadioId) -> Result<(), Failure> {
    let request = with_token(Request::delete(&url(&format!("/radios/{radio}"))));
    answer(request.send().await).await.map(|_| ())
}

/// Puts a radio's channels in another order, by the slots they have now.
pub async fn reorder_channels(radio: RadioId, order: &[u32]) -> Result<SettingsApplied, Failure> {
    let order = ChannelOrder {
        order: order.to_vec(),
    };
    let request = with_token(Request::put(&url(&format!(
        "/radios/{radio}/channels/order"
    ))))
    .json(&order)
    .map_err(|_| Failure::Unreachable)?;
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
}

/// Takes a channel off a radio, its messages kept aside or forgotten.
pub async fn remove_channel(
    radio: RadioId,
    index: u32,
    history: ChannelHistory,
) -> Result<SettingsApplied, Failure> {
    let history = match history {
        ChannelHistory::Keep => "keep",
        ChannelHistory::Forget => "forget",
    };
    let request = with_token(Request::delete(&url(&format!(
        "/radios/{radio}/channels/{index}?history={history}"
    ))));
    let response = answer(request.send().await).await?;
    response.json().await.map_err(|_| Failure::Unreachable)
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
