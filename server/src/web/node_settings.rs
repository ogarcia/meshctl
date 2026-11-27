// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Another Meshtastic node's settings, read and changed over the mesh as its
//! admin, through the radio: the radio's public key has to be one of the
//! node's admin keys. The same as the radio's own, under the node: each
//! read asked of the node the first time, kept in memory after, and each
//! change sent to it, sealed, step by step until it acknowledges them.
//! Every request goes over the air: seconds each, up to a minute.

use super::AppState;
use super::settings::{Removal, refused};
use crate::radios::Whose;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    ActionRequest, AdminKeys, ChannelHistory, ChannelOrder, ChannelSecret, Contact, Error,
    NodeSettings, NodeSettingsChange, NodeStatusMessage, RadioId, SectionChange, SectionSecret,
    SettingsApplied,
};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show, change))
        .routes(routes!(status))
        .routes(routes!(change_section))
        .routes(routes!(admin_keys, change_admin_keys))
        .routes(routes!(act))
        .routes(routes!(reorder_channels))
        .routes(routes!(remove_channel))
        .routes(routes!(channel_link))
        .routes(routes!(channel_key))
        .routes(routes!(section_secret))
}

/// Whether to ask the node anew.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Reading {
    /// Ask the node anew for all of it, rather than take what meshctl has
    /// of it from before.
    #[serde(default)]
    fresh: bool,
}

/// A node's settings
///
/// Another node's own settings, as the radio's are shown: its owner and
/// role, its LoRa and its eight channels, asked of it over the mesh as its
/// admin, eleven requests one after another, the first time or when
/// `fresh`. Secrets are never shown, only what kind of key each channel
/// has.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/settings",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
        Reading,
    ),
    responses(
        (status = 200, description = "The settings", body = NodeSettings),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 502, description = "The node, or the radio, would not: `radio_refused`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn show(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
    Query(reading): Query<Reading>,
) -> Response {
    match state
        .radios
        .settings(radio, Whose::Node(node), reading.fresh)
        .await
    {
        Ok(settings) => Json(settings).into_response(),
        Err(failure) => refused(failure),
    }
}

/// A node's status message
///
/// What another node says of itself, as part of its owner's settings,
/// asked of it over the mesh apart from them: a node whose firmware has no
/// status message may not answer at all, and would hold up the rest. Asked
/// once; kept after, as the rest is. `status` is none if it has none, or
/// did not say.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/settings/status",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    responses(
        (status = 200, description = "Its status message, if it has one", body = NodeStatusMessage),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
    )
)]
async fn status(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.node_status(radio, node).await {
        Ok(status) => Json(NodeStatusMessage { status }).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change a node's settings
///
/// Changes another node's settings over the mesh, as its admin, as the
/// radio's own are changed. LoRa and channels too: a change there may leave
/// the node out of reach of the radio, which nothing over the mesh can
/// undo then. The node reboots for most of them.
#[utoipa::path(
    put,
    path = "/radios/{radio}/nodes/{node}/settings",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    request_body = NodeSettingsChange,
    responses(
        (status = 202, description = "Taken by the node", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Settings the node cannot have: `invalid_settings`", body = Error),
        (status = 502, description = "The node, or the radio, would not: `radio_refused`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
    Json(wanted): Json<NodeSettingsChange>,
) -> Response {
    match state
        .radios
        .change_settings(radio, Whose::Node(node), &wanted)
        .await
    {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change a node's section
///
/// Sets the fields given of one section of another node's settings, over
/// the mesh as its admin, as the radio's own are set.
#[utoipa::path(
    put,
    path = "/radios/{radio}/nodes/{node}/sections/{section}",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
        ("section" = String, Path, description = "The section's id", example = "module.telemetry"),
    ),
    request_body = SectionChange,
    responses(
        (status = 202, description = "Taken by the node", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "No such section or field, or a value it does not take: `invalid_settings`", body = Error),
        (status = 502, description = "The node, or the radio, would not: `radio_refused`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change_section(
    State(state): State<AppState>,
    Path((radio, node, section)): Path<(RadioId, u32, String)>,
    Json(change): Json<SectionChange>,
) -> Response {
    match state
        .radios
        .change_section(
            radio,
            Whose::Node(node),
            &section,
            &change.values,
            change.fixed_position.as_ref(),
        )
        .await
    {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Have a node do something
///
/// Has another node reboot, shut down, forget the nodes it knows but its
/// favourites, or take its factory configuration again, over the mesh as
/// its admin. The last leaves it with no admin keys, and so out of reach.
#[utoipa::path(
    post,
    path = "/radios/{radio}/nodes/{node}/actions",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    request_body = ActionRequest,
    responses(
        (status = 202, description = "Taken by the node"),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 502, description = "The node, or the radio, would not: `radio_refused`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn act(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
    Json(request): Json<ActionRequest>,
) -> Response {
    match state
        .radios
        .act(radio, Whose::Node(node), request.action)
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(failure) => refused(failure),
    }
}

/// A node's admin keys
///
/// The public keys another node takes admin requests from, asked of it.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/admin-keys",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    responses(
        (status = 200, description = "Its admin keys", body = AdminKeys),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn admin_keys(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.admin_keys(radio, Whose::Node(node)).await {
        Ok(keys) => Json(keys).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change a node's admin keys
///
/// Sets another node's admin keys, all of them. Leaving the radio's own
/// out leaves it unable to administer the node from then on.
#[utoipa::path(
    put,
    path = "/radios/{radio}/nodes/{node}/admin-keys",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    request_body = AdminKeys,
    responses(
        (status = 202, description = "Taken by the node", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "More than three keys, one not of 32 bytes or twice, or none for a managed node: `invalid_settings`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change_admin_keys(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
    Json(change): Json<AdminKeys>,
) -> Response {
    match state
        .radios
        .change_admin_keys(radio, Whose::Node(node), &change.keys)
        .await
    {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Reorder a node's channels
///
/// Puts another node's channels in another order, as the radio's are.
#[utoipa::path(
    put,
    path = "/radios/{radio}/nodes/{node}/channels/order",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    request_body = ChannelOrder,
    responses(
        (status = 202, description = "Taken by the node", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not every channel once, by its slot: `invalid_settings`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn reorder_channels(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
    Json(order): Json<ChannelOrder>,
) -> Response {
    match state
        .radios
        .reorder_channels(radio, Whose::Node(node), &order.order)
        .await
    {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Remove a node's channel
///
/// Takes a channel off another node, but its primary: those after it move
/// up a slot. Nothing of it is kept here, as it has no conversation here.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/nodes/{node}/channels/{index}",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
        ("index" = u32, Path, description = "The channel's slot, 1 to 7"),
    ),
    responses(
        (status = 202, description = "Taken by the node", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "The primary, or a slot with no channel: `invalid_settings`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn remove_channel(
    State(state): State<AppState>,
    Path((radio, node, index)): Path<(RadioId, u32, u32)>,
    Query(removal): Query<Removal>,
) -> Response {
    let forget = removal.history == ChannelHistory::Forget;
    match state
        .radios
        .remove_channel(radio, Whose::Node(node), index, forget)
        .await
    {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// A node's channel link
///
/// One of another node's channels as a link to add it, as the radio's own
/// are shared.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/channels/{index}/link",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
        ("index" = u32, Path, description = "The channel's slot, 0 to 7"),
    ),
    responses(
        (status = 200, description = "The link", body = Contact),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "A slot with no channel: `invalid_settings`", body = Error),
    )
)]
async fn channel_link(
    State(state): State<AppState>,
    Path((radio, node, index)): Path<(RadioId, u32, u32)>,
) -> Response {
    match state
        .radios
        .channel_link(radio, Whose::Node(node), index)
        .await
    {
        Ok(url) => Json(Contact { url }).into_response(),
        Err(failure) => refused(failure),
    }
}

/// A node's channel key
///
/// One of another node's channel keys itself, as the radio's own are
/// handed out: asked for alone, never kept, logged.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/secrets/channels/{index}",
    tag = "secrets",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
        ("index" = u32, Path, description = "The channel's slot, 0 to 7"),
    ),
    responses(
        (status = 200, description = "The key", body = ChannelSecret),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, node, or slot", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
    )
)]
async fn channel_key(
    State(state): State<AppState>,
    Path((radio, node, index)): Path<(RadioId, u32, u32)>,
) -> Response {
    match state
        .radios
        .channel_secret(radio, Whose::Node(node), index)
        .await
    {
        Ok(Some(secret)) => ([(header::CACHE_CONTROL, "no-store")], Json(secret)).into_response(),
        Ok(None) => super::secrets::not_found(),
        Err(failure) => refused(failure),
    }
}

/// A node's setting's secret
///
/// A secret of one of the sections of another node's settings, as the
/// radio's own are handed out: asked for alone, never kept, logged.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/secrets/sections/{section}/{field}",
    tag = "secrets",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
        ("section" = String, Path, description = "The section's id, as `config.network`"),
        ("field" = String, Path, description = "The field's dotted name, as `wifi_psk`"),
    ),
    responses(
        (status = 200, description = "The secret", body = SectionSecret),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio, node, or secret", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
    )
)]
async fn section_secret(
    State(state): State<AppState>,
    Path((radio, node, section, field)): Path<(RadioId, u32, String, String)>,
) -> Response {
    match state
        .radios
        .section_secret(radio, Whose::Node(node), &section, &field)
        .await
    {
        Ok(Some(secret)) => ([(header::CACHE_CONTROL, "no-store")], Json(secret)).into_response(),
        Ok(None) => super::secrets::not_found(),
        Err(failure) => refused(failure),
    }
}
