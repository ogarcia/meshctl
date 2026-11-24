// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What only MeshCore radios have: their own settings, which are not
//! Meshtastic's, and their figures, asked for now.

use super::AppState;
use super::settings::refused;
use crate::radios::LoginFailure;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    Advert, ChannelOrder, Contact, Error, ErrorCode, MeshcoreChannelChange, MeshcoreChannelScope,
    MeshcoreChannels, MeshcoreRegions, MeshcoreSettings, MeshcoreStatus, NewNodeLogin, NodeLogin,
    RadioId,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(settings, change_settings))
        .routes(routes!(status))
        .routes(routes!(channels))
        .routes(routes!(reorder_channels))
        .routes(routes!(set_channel, remove_channel))
        .routes(routes!(channel_link))
        .routes(routes!(channel_scope))
        .routes(routes!(reset_path))
        .routes(routes!(advert))
        .routes(routes!(regions))
        .routes(routes!(search_regions))
        .routes(routes!(node_login, log_in_node, forget_node_password))
}

pub(super) fn login_failed(failure: LoginFailure) -> Response {
    let (status, error) = match failure {
        LoginFailure::NoSuchRadio => (StatusCode::NOT_FOUND, ErrorCode::NotFound),
        LoginFailure::NoLogin => (StatusCode::NOT_FOUND, ErrorCode::NoLogin),
        LoginFailure::NotOnRadio => (StatusCode::CONFLICT, ErrorCode::NotOnRadio),
        LoginFailure::NotConnected => (StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        LoginFailure::WrongPassword => (StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::WrongPassword),
        LoginFailure::NoAnswer => (StatusCode::GATEWAY_TIMEOUT, ErrorCode::RadioDidNotAnswer),
        LoginFailure::Refused(why) => {
            tracing::info!(why, "a login refused by the radio");
            (StatusCode::BAD_GATEWAY, ErrorCode::RadioRefused)
        }
        LoginFailure::Failed(err) => {
            tracing::error!(error = format_args!("{err:#}"), "login");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    (status, Json(Error { error })).into_response()
}

/// A node's login
///
/// The radio's login to a MeshCore node that takes one: a room, which sends
/// its posts only to those logged in, or a repeater or a sensor, which tell
/// what they are asked only to them. What it was let in as since it last
/// connected, if it logged in, and whether a password is kept for it.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/login",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    responses(
        (status = 200, description = "The login", body = NodeLogin),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows no such node that takes logins: `no_login`", body = Error),
    )
)]
async fn node_login(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.node_login(radio, node).await {
        Ok(login) => Json(login).into_response(),
        Err(failure) => login_failed(failure),
    }
}

/// Log in to a node
///
/// Logs the radio in to a MeshCore room, repeater or sensor, over the air:
/// with the password given, kept for the next login once known to be right;
/// with none, with the one kept, or as a guest. A room then sends the radio
/// the posts it does not have, which arrive as messages from the room, each
/// with its `author`. A room that does not take the password says no, or, if
/// it lets nobody in to read, nothing, as a repeater or a sensor does; and
/// then the radio stops waiting.
#[utoipa::path(
    post,
    path = "/radios/{radio}/nodes/{node}/login",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    request_body = NewNodeLogin,
    responses(
        (status = 200, description = "Logged in", body = NodeLogin),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows no such node that takes logins: `no_login`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`; or no longer has the node, which meshctl alone keeps: `not_on_radio`, with nothing sent", body = Error),
        (status = 422, description = "The node did not take the password: `wrong_password`", body = Error),
        (status = 502, description = "The radio would not send it: `radio_refused`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn log_in_node(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
    Json(login): Json<NewNodeLogin>,
) -> Response {
    match state.radios.log_in_node(radio, node, login.password).await {
        Ok(login) => Json(login).into_response(),
        Err(failure) => login_failed(failure),
    }
}

/// Forget a node's password
///
/// Forgets the password kept for a MeshCore node; a login made with it
/// stays.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/nodes/{node}/login",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    responses(
        (status = 200, description = "Forgotten", body = NodeLogin),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or it knows no such node that takes logins: `no_login`", body = Error),
    )
)]
async fn forget_node_password(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.forget_node_password(radio, node).await {
        Ok(login) => Json(login).into_response(),
        Err(failure) => login_failed(failure),
    }
}

/// MeshCore regions
///
/// The regions a MeshCore radio's repeaters and rooms said they repeat
/// floods scoped to, each as it last said: every region with how many serve
/// it, and how many of those the radio hears straight, which are what
/// anything it floods reaches first; and whether it is asking them now.
#[utoipa::path(
    get,
    path = "/radios/{radio}/meshcore/regions",
    tag = "nodes",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The regions", body = MeshcoreRegions),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
    )
)]
async fn regions(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.meshcore_regions(radio).await {
        Ok(regions) => Json(regions).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Search for regions
///
/// Asks a MeshCore radio's repeaters and rooms which regions they repeat
/// on: those it knows a path to, up to two hops away, the nearest first,
/// one at a time and spaced out, in the background. Each answer comes as
/// the request it is, and the regions are read again as it ends, told as
/// `synced`. Those that answered in the last ten minutes are not asked
/// again. This goes on the air, a direct request to each.
#[utoipa::path(
    post,
    path = "/radios/{radio}/meshcore/regions/search",
    tag = "nodes",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 202, description = "Searching, or nothing left to ask", body = MeshcoreRegions),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
    )
)]
async fn search_regions(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.search_regions(radio).await {
        Ok(regions) => (StatusCode::ACCEPTED, Json(regions)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// MeshCore channels
///
/// A MeshCore radio's channels, each by its kind: `Public`, every radio's;
/// a `#name`, whose secret its name gives; or one with a secret of its own.
/// Secrets are never shown here: a channel's link has its own.
#[utoipa::path(
    get,
    path = "/radios/{radio}/meshcore/channels",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The channels", body = MeshcoreChannels),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
    )
)]
async fn channels(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.meshcore_channels(radio).await {
        Ok(channels) => Json(channels).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Reorder MeshCore channels
///
/// Puts the radio's channels in another order, given as the slots they have
/// now, every one once: to the first slots, the rest emptied. Secrets and
/// all go with each channel, and so does its history in meshctl. Read back
/// from the radio after, and told as `synced`.
#[utoipa::path(
    put,
    path = "/radios/{radio}/meshcore/channels/order",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = ChannelOrder,
    responses(
        (status = 204, description = "Reordered, and read back"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not every channel the radio has, once each: `invalid_settings`; not a MeshCore radio: `unsupported_kind`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn reorder_channels(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(order): Json<ChannelOrder>,
) -> Response {
    match state
        .radios
        .reorder_meshcore_channels(radio, &order.order)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// Set a MeshCore channel
///
/// Puts a channel in a slot, a new one or in place of the one there. Its
/// secret, 16 bytes in hex, or left out: a `#name` takes its name's, and
/// any other a new random one, unless it keeps the name it had, and with it
/// its secret. Read back from the radio after, and told as `synced`.
#[utoipa::path(
    put,
    path = "/radios/{radio}/meshcore/channels/{index}",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("index" = u32, Path, description = "The channel's slot"),
    ),
    request_body = MeshcoreChannelChange,
    responses(
        (status = 204, description = "Set, and read back"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a channel the radio can have: `invalid_settings`; the very same as another, name and secret: `duplicate_channel`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn set_channel(
    State(state): State<AppState>,
    Path((radio, index)): Path<(RadioId, u32)>,
    Json(wanted): Json<MeshcoreChannelChange>,
) -> Response {
    match state
        .radios
        .set_meshcore_channel(radio, index, &wanted)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// Remove a MeshCore channel
///
/// Empties its slot. Its conversation stays, as a channel gone.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/meshcore/channels/{index}",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("index" = u32, Path, description = "The channel's slot"),
    ),
    responses(
        (status = 204, description = "Removed, and read back"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "No channel in that slot: `invalid_settings`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn remove_channel(
    State(state): State<AppState>,
    Path((radio, index)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.remove_meshcore_channel(radio, index).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// Set a MeshCore channel's region
///
/// The region what is sent on a channel floods in, or `*` for none, or none
/// for the radio's default: kept by meshctl for the channel, whichever slot
/// it is in, as the radio keeps none per channel. Nothing is written to the
/// radio. Told as `synced`.
#[utoipa::path(
    put,
    path = "/radios/{radio}/meshcore/channels/{index}/scope",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("index" = u32, Path, description = "The channel's slot"),
    ),
    request_body = MeshcoreChannelScope,
    responses(
        (status = 204, description = "Kept"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "No channel in that slot, or not a region: `invalid_settings`; not a MeshCore radio: `unsupported_kind`", body = Error),
    )
)]
async fn channel_scope(
    State(state): State<AppState>,
    Path((radio, index)): Path<(RadioId, u32)>,
    Json(wanted): Json<MeshcoreChannelScope>,
) -> Response {
    match state
        .radios
        .set_meshcore_channel_scope(radio, index, wanted.scope.as_deref())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// MeshCore channel link
///
/// A channel's link, as the MeshCore apps read it, its secret in it: to
/// share it, in a QR or as text.
#[utoipa::path(
    get,
    path = "/radios/{radio}/meshcore/channels/{index}/link",
    tag = "secrets",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("index" = u32, Path, description = "The channel's slot"),
    ),
    responses(
        (status = 200, description = "The link", body = Contact),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "No channel in that slot: `invalid_settings`", body = Error),
    )
)]
async fn channel_link(
    State(state): State<AppState>,
    Path((radio, index)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.meshcore_channel_link(radio, index).await {
        Ok(url) => Json(Contact { url }).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Reset a node's path
///
/// Has a MeshCore radio forget the path it knows to a node: the next message
/// to it floods to find one. Nothing goes on the air now.
#[utoipa::path(
    post,
    path = "/radios/{radio}/nodes/{node}/reset-path",
    tag = "nodes",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = String, Path, description = "The node's id or number", example = "!90cda275"),
    ),
    responses(
        (status = 204, description = "Forgotten"),
        (status = 400, description = "That is neither a node id nor a number", body = Error),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`; not one of its contacts: `invalid_settings`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn reset_path(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, String)>,
) -> Response {
    let Some(num) = super::radios::node_num(&node) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(Error {
                error: ErrorCode::BadRequest,
            }),
        )
            .into_response();
    };
    match state.radios.reset_path(radio, num).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// Advertise
///
/// Has a MeshCore radio say who it is: to those it hears directly, or with
/// `flood`, to the whole mesh. This goes on the air.
#[utoipa::path(
    post,
    path = "/radios/{radio}/advert",
    tag = "nodes",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = Advert,
    responses(
        (status = 204, description = "Sent"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn advert(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(advert): Json<Advert>,
) -> Response {
    match state.radios.advert(radio, advert.flood).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// MeshCore settings
///
/// A MeshCore radio's own settings, as it last said them. Only while it is
/// connected.
#[utoipa::path(
    get,
    path = "/radios/{radio}/meshcore/settings",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The settings", body = MeshcoreSettings),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
    )
)]
async fn settings(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.meshcore_settings(radio) {
        Ok(settings) => Json(settings).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change MeshCore settings
///
/// Replaces them. Each that changes is sent as the radio takes it, and
/// applied at once, its LoRa settings too, with no reboot; then they are read
/// back from it, and arrive as `synced` on the event stream. A refusal
/// stops there: what was changed before it stays changed.
///
/// - `name`: up to 31 bytes.
/// - `frequency`: 150000 to 2500000 kHz; `bandwidth`: 7000 to 500000 Hz;
///   `spreading_factor`: 5 to 12; `coding_rate`: 5 to 8.
/// - `tx_power`: -9 to `max_tx_power` dBm.
/// - `repeat`: only on firmware that has it, and only on 433.000, 869.495
///   or 918.000 MHz.
#[utoipa::path(
    put,
    path = "/radios/{radio}/meshcore/settings",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = MeshcoreSettings,
    responses(
        (status = 204, description = "Changed, and read back"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not settings the radio can have: `invalid_settings`; not a MeshCore radio: `unsupported_kind`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change_settings(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(wanted): Json<MeshcoreSettings>,
) -> Response {
    match state.radios.change_meshcore_settings(radio, &wanted).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => refused(failure),
    }
}

/// MeshCore status
///
/// A MeshCore radio's battery, storage and figures, asked of it now: how
/// long it has been on, how busy and noisy it hears the air, what it has
/// sent and received. Each is none if its firmware does not say it.
#[utoipa::path(
    get,
    path = "/radios/{radio}/meshcore/status",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The figures", body = MeshcoreStatus),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not a MeshCore radio: `unsupported_kind`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn status(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.meshcore_status(radio).await {
        Ok(status) => Json(status).into_response(),
        Err(failure) => refused(failure),
    }
}
