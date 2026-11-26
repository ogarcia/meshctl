// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's own settings: what it is called, what it does, how it
//! transmits, and its channels.

use super::AppState;
use crate::radios::SettingsFailure;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    AdminKeys, ChannelHistory, ChannelOrder, Error, ErrorCode, NodeSettings, NodeSettingsChange,
    RadioId, SectionChange, SettingsApplied, SettingsSection,
};
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show, change))
        .routes(routes!(sections))
        .routes(routes!(admin_keys, change_admin_keys))
        .routes(routes!(node_sections))
        .routes(routes!(node_section))
        .routes(routes!(change_section))
        .routes(routes!(reorder_channels))
        .routes(routes!(remove_channel))
        .routes(routes!(channel_link))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

pub(super) fn refused(failure: SettingsFailure) -> Response {
    match failure {
        SettingsFailure::NoSuchRadio => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        SettingsFailure::NotConnected => error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        SettingsFailure::Invalid(why) => {
            tracing::debug!(setting = %why, "invalid settings");
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidSettings)
        }
        SettingsFailure::PublicPrecision => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::PublicChannelPrecision,
        ),
        SettingsFailure::Duplicate => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::DuplicateChannel,
        ),
        SettingsFailure::Unsupported => {
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::UnsupportedKind)
        }
        SettingsFailure::NoAnswer => {
            error(StatusCode::GATEWAY_TIMEOUT, ErrorCode::RadioDidNotAnswer)
        }
        SettingsFailure::UnknownNode => error(StatusCode::NOT_FOUND, ErrorCode::InvalidNode),
        SettingsFailure::NotAdmin => error(StatusCode::FORBIDDEN, ErrorCode::NotAdmin),
        SettingsFailure::Refused(reason) => {
            tracing::debug!(%reason, "refused");
            error(StatusCode::BAD_GATEWAY, ErrorCode::RadioRefused)
        }
        SettingsFailure::Failed(err) => {
            tracing::error!(error = format_args!("{err:#}"), "database");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Settings
///
/// The radio's own settings, as it sent them when it last synced. Only while
/// it is connected: meshctl does not keep them. Channel keys and other
/// secrets are never shown, only what kind of key each channel has.
#[utoipa::path(
    get,
    path = "/radios/{radio}/settings",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The settings", body = NodeSettings),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
    )
)]
async fn show(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.settings(radio).await {
        Ok(settings) => Json(settings).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change settings
///
/// Changes the parts given, each whole, and leaves the rest as it is; within
/// a part, what the API does not show is kept as the radio has it. All at
/// once: the radio saves them together and, if any needs it, reboots once,
/// dropping its link for a moment. The settings are read back from it a few
/// seconds later, and arrive as `synced` on the event stream.
///
/// - `owner.long_name`: up to 39 bytes; `owner.short_name`: up to 4.
/// - `device.role`, `lora.region`, `lora.modem_preset`: one of the codes in
///   `choices`.
/// - `lora.hop_limit`: 1 to 7.
/// - `channels`: only those to change. Slot 0 is the primary and only it; a
///   secondary needs a name; names take up to 11 bytes. `key` is `keep`
///   unless said otherwise; with `custom`, `custom_key` carries the key in
///   base64, of 16 or 32 bytes, or 1 for a simple one. The keys themselves
///   are asked for under `secrets`.
#[utoipa::path(
    put,
    path = "/radios/{radio}/settings",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = NodeSettingsChange,
    responses(
        (status = 202, description = "Sent to the radio", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not settings the radio can have: `invalid_settings`; a channel the very same as another, name and key: `duplicate_channel`; a position more precise than a public channel takes: `public_channel_precision`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(wanted): Json<NodeSettingsChange>,
) -> Response {
    match state.radios.change_settings(radio, &wanted).await {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Sections
///
/// The rest of the radio's configuration and every module it has, section by
/// section, each field as the protobufs describe it: label, description,
/// unit, limits and choices, in English. Secrets show only whether they are
/// set. Only while the radio is connected. Left out: LoRa and the role, which
/// `settings` covers, and security, where a slip locks the owner out.
#[utoipa::path(
    get,
    path = "/radios/{radio}/sections",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "The sections", body = Vec<SettingsSection>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
    )
)]
async fn sections(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.sections(radio) {
        Ok(sections) => Json(sections).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Admin keys
///
/// The public keys the radio takes admin requests from, over the mesh and
/// sealed to it, which let their radios read and change its settings: up
/// to three. And whether it is managed, changed only so.
#[utoipa::path(
    get,
    path = "/radios/{radio}/admin-keys",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    responses(
        (status = 200, description = "Its admin keys", body = AdminKeys),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
    )
)]
async fn admin_keys(State(state): State<AppState>, Path(radio): Path<RadioId>) -> Response {
    match state.radios.admin_keys(radio) {
        Ok(keys) => Json(keys).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change admin keys
///
/// Sets the radio's admin keys, all of them, and leaves the rest of its
/// security as it is: its own keys, and whether it is managed. Each in
/// base64, of 32 bytes, once; none at all for a managed radio, which
/// nothing but a cable could change then. The radio reboots.
#[utoipa::path(
    put,
    path = "/radios/{radio}/admin-keys",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = AdminKeys,
    responses(
        (status = 202, description = "Sent to the radio", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "More than three keys, one not of 32 bytes or twice, or none for a managed radio: `invalid_settings`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change_admin_keys(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(change): Json<AdminKeys>,
) -> Response {
    match state.radios.change_admin_keys(radio, &change.keys).await {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// A node's sections
///
/// The sections of another node's settings that can be read from it over
/// the mesh, by their ids, as its admin: the radio's public key has to be
/// one of the node's admin keys. Asks the node what it has, which takes a
/// few seconds, up to a minute, and tells whether it takes the radio's key.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/sections",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
    ),
    responses(
        (status = 200, description = "The sections' ids", body = Vec<String>),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 502, description = "The node, or the radio, would not: `radio_refused`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn node_sections(
    State(state): State<AppState>,
    Path((radio, node)): Path<(RadioId, u32)>,
) -> Response {
    match state.radios.node_sections(radio, node).await {
        Ok(sections) => Json(sections).into_response(),
        Err(failure) => refused(failure),
    }
}

/// A node's section
///
/// One section of another node's settings, read from it over the mesh as
/// its admin, as the radio's own are shown. Each is asked of it as it is
/// wanted: a few seconds, up to a minute.
#[utoipa::path(
    get,
    path = "/radios/{radio}/nodes/{node}/sections/{section}",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("node" = u32, Path, description = "The node's number"),
        ("section" = String, Path, description = "The section's id", example = "module.telemetry"),
    ),
    responses(
        (status = 200, description = "The section", body = SettingsSection),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 403, description = "The node does not take the radio's key as an admin key: `not_admin`", body = Error),
        (status = 404, description = "There is no such radio, or node: `invalid_node`", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "No such section: `invalid_settings`", body = Error),
        (status = 502, description = "The node, or the radio, would not: `radio_refused`", body = Error),
        (status = 504, description = "The node did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn node_section(
    State(state): State<AppState>,
    Path((radio, node, section)): Path<(RadioId, u32, String)>,
) -> Response {
    match state.radios.node_section(radio, node, &section).await {
        Ok(section) => Json(section).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Change a section
///
/// Sets the fields given, by their names in the section, and leaves the rest
/// as the radio has them; a secret is written only when given. The radio
/// reboots for nearly any of them, and drops its link for a moment.
///
/// `config.position` takes `fixed_position` too, where the radio is, with
/// its position fixed, already or along with it; that alone needs no
/// reboot. Turning `fixed_position` off lets the one it had go.
#[utoipa::path(
    put,
    path = "/radios/{radio}/sections/{section}",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("section" = String, Path, description = "The section's id", example = "module.telemetry"),
    ),
    request_body = SectionChange,
    responses(
        (status = 202, description = "Sent to the radio", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "No such section or field, a value it does not take, or a fixed position not fixed or off the globe: `invalid_settings`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn change_section(
    State(state): State<AppState>,
    Path((radio, section)): Path<(RadioId, String)>,
    Json(change): Json<SectionChange>,
) -> Response {
    match state
        .radios
        .change_section(
            radio,
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

/// Reorder channels
///
/// Puts the radio's channels in another order, given as the slots they have
/// now, every one once. The first becomes the primary, which the radio sends
/// its position and its node's info on; the rest follow it with no gap. Keys
/// and all go with each channel, and so does its history in meshctl. An
/// unnamed channel stays unnamed in any slot: the radio names it after its
/// modem preset, on the air too.
#[utoipa::path(
    put,
    path = "/radios/{radio}/channels/order",
    tag = "settings",
    params(("radio" = RadioId, Path, description = "The radio's id")),
    request_body = ChannelOrder,
    responses(
        (status = 202, description = "Sent to the radio", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "Not every channel the radio has, once each: `invalid_settings`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn reorder_channels(
    State(state): State<AppState>,
    Path(radio): Path<RadioId>,
    Json(order): Json<ChannelOrder>,
) -> Response {
    match state.radios.reorder_channels(radio, &order.order).await {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

/// What becomes of a channel's messages.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Removal {
    /// `keep`, if not said, or `forget`.
    #[serde(default)]
    history: ChannelHistory,
}

/// Channel link
///
/// A channel's link, as the apps of its radio's mesh read it, its key in
/// it: a Meshtastic one, after `meshtastic.org/e/?add=true#`, to be added
/// rather than in place of the channels there, with the radio's LoRa; a
/// MeshCore one, as `meshcore/channels/{index}/link` gives it.
#[utoipa::path(
    get,
    path = "/radios/{radio}/channels/{index}/link",
    tag = "secrets",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("index" = u32, Path, description = "The channel's slot"),
    ),
    responses(
        (status = 200, description = "The link", body = meshctl_api::Contact),
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
    match state.radios.channel_link(radio, index).await {
        Ok(url) => Json(meshctl_api::Contact { url }).into_response(),
        Err(failure) => refused(failure),
    }
}

/// Remove a channel
///
/// Takes a channel off the radio, but the primary; those after it move up a
/// slot, their conversations with them. Its own conversation waits for it,
/// in no slot, and is back should the very same channel come back; or, with
/// `history=forget`, is forgotten, unless another slot has the very same
/// channel too.
#[utoipa::path(
    delete,
    path = "/radios/{radio}/channels/{index}",
    tag = "settings",
    params(
        ("radio" = RadioId, Path, description = "The radio's id"),
        ("index" = u32, Path, description = "The channel's slot, 1 to 7"),
        Removal,
    ),
    responses(
        (status = 202, description = "Sent to the radio", body = SettingsApplied),
        (status = 401, description = "The access token is missing or wrong", body = Error),
        (status = 404, description = "There is no such radio", body = Error),
        (status = 409, description = "The radio is not connected: `radio_not_connected`", body = Error),
        (status = 422, description = "The primary, or a slot without a channel: `invalid_settings`", body = Error),
        (status = 504, description = "The radio did not answer: `radio_did_not_answer`", body = Error),
    )
)]
async fn remove_channel(
    State(state): State<AppState>,
    Path((radio, index)): Path<(RadioId, u32)>,
    Query(removal): Query<Removal>,
) -> Response {
    let forget = removal.history == ChannelHistory::Forget;
    match state.radios.remove_channel(radio, index, forget).await {
        Ok(applied) => (StatusCode::ACCEPTED, Json(applied)).into_response(),
        Err(failure) => refused(failure),
    }
}

#[cfg(test)]
mod tests {
    use crate::web::tests::{RADIO, app, ask, get, json};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use meshctl_api::{Error, ErrorCode};

    #[tokio::test]
    async fn settings_need_the_radio_to_have_sent_them() {
        // The test radio is in sync, but sent no settings.
        let (status, body) = ask(
            app(None).await,
            get(&format!("/api/v1/radios/{RADIO}/settings")),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json::<Error>(&body).error, ErrorCode::RadioNotConnected);
    }

    #[tokio::test]
    async fn a_change_the_radio_cannot_have_is_refused() {
        let request = Request::put(format!("/api/v1/radios/{RADIO}/settings"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"device":{"role":"PIRATE"}}"#.to_owned()))
            .unwrap();
        let (status, body) = ask(app(None).await, request).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json::<Error>(&body).error, ErrorCode::InvalidSettings);
    }
}
