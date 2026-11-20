// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's own settings: what it is called, what it does, how it
//! transmits, and its channels.

use super::AppState;
use crate::radios::SettingsFailure;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use meshctl_api::{
    Error, ErrorCode, NodeSettings, NodeSettingsChange, RadioId, SectionChange, SettingsApplied,
    SettingsSection,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show, change))
        .routes(routes!(sections))
        .routes(routes!(change_section))
}

fn error(status: StatusCode, error: ErrorCode) -> Response {
    (status, Json(Error { error })).into_response()
}

fn refused(failure: SettingsFailure) -> Response {
    match failure {
        SettingsFailure::NoSuchRadio => error(StatusCode::NOT_FOUND, ErrorCode::NotFound),
        SettingsFailure::NotConnected => error(StatusCode::CONFLICT, ErrorCode::RadioNotConnected),
        SettingsFailure::Invalid(why) => {
            tracing::debug!(setting = %why, "invalid settings");
            error(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::InvalidSettings)
        }
        SettingsFailure::NoAnswer => {
            error(StatusCode::GATEWAY_TIMEOUT, ErrorCode::RadioDidNotAnswer)
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
    match state.radios.settings(radio) {
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
///   unless said otherwise.
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
        (status = 422, description = "Not settings the radio can have: `invalid_settings`", body = Error),
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

/// Change a section
///
/// Sets the fields given, by their names in the section, and leaves the rest
/// as the radio has them; a secret is written only when given. The radio
/// reboots for nearly any of them, and drops its link for a moment.
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
        (status = 422, description = "No such section or field, or a value it does not take: `invalid_settings`", body = Error),
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
        .change_section(radio, &section, &change.values)
        .await
    {
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
