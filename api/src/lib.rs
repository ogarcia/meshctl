// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What travels between the daemon and its clients: the web interface today,
//! a CLI or a TUI tomorrow. Every client reads the same types the daemon
//! writes, so a change here is a compile error wherever it matters.
//!
//! Nothing here is text for people. Errors, states, roles and models travel as
//! codes, and each client puts them in its own words and language.

pub mod events;

use serde::{Deserialize, Serialize};

/// Where version 1 of the API is served.
pub const API_V1: &str = "/api/v1";

/// The header that carries the access token, when the daemon is given one.
///
/// Not `Authorization`, so that it can sit behind a proxy that uses that one
/// for its own authentication (basic auth in nginx, say) without the two
/// stepping on each other.
pub const TOKEN_HEADER: &str = "x-meshctl-token";

/// What is answering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Version {
    /// The program's name.
    #[cfg_attr(feature = "openapi", schema(example = "meshctl"))]
    pub name: String,
    /// Its version, as Cargo writes it.
    #[cfg_attr(feature = "openapi", schema(example = "0.1.0"))]
    pub version: String,
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The access token is missing or wrong.
    Unauthorized,
    /// There is nothing at that address.
    NotFound,
    /// The request does not say what it should.
    BadRequest,
    /// A radio needs a name.
    InvalidName,
    /// That is not where a radio can be: neither `host`, `host:port`, nor the
    /// path of a serial device.
    InvalidAddress,
    /// This meshctl cannot talk to that kind of radio yet.
    UnsupportedKind,
}

/// The body of every error the API answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Error {
    pub error: ErrorCode,
}

/// How a radio is identified in the API.
pub type RadioId = i64;

/// Which mesh a radio speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RadioKind {
    Meshtastic,
    Meshcore,
}

/// A radio meshctl knows of, and how its link is doing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Radio {
    pub id: RadioId,
    #[cfg_attr(feature = "openapi", schema(example = "Home"))]
    pub name: String,
    pub kind: RadioKind,
    /// `host`, `host:port`, or the path of a serial device with an optional
    /// `@speed`.
    #[cfg_attr(feature = "openapi", schema(example = "192.168.1.10:4403"))]
    pub address: String,
    /// Whether meshctl keeps connected to it.
    pub enabled: bool,
    pub link: LinkState,
    /// When it was last connected and in sync, in seconds since the epoch;
    /// none if it never was. What meshctl shows of a radio that is not
    /// connected is what it knew then.
    pub last_connected: Option<i64>,
}

/// What it takes to add a radio, or to change one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RadioSettings {
    #[cfg_attr(feature = "openapi", schema(example = "Home"))]
    pub name: String,
    pub kind: RadioKind,
    #[cfg_attr(feature = "openapi", schema(example = "192.168.1.10:4403"))]
    pub address: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

/// How the link to a radio is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Turned off: meshctl does not try to connect.
    Disabled,
    /// Not connected, and waiting to try again.
    Disconnected,
    /// Trying to connect.
    Connecting,
    /// Connected, and receiving the radio's configuration and node list.
    Syncing,
    /// Connected and up to date.
    Ready,
}

/// The daemon's link to the radio, and the radio itself once it has said who
/// it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Status {
    pub link: LinkState,
    pub local: Option<LocalNode>,
}

/// The radio meshctl is connected to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LocalNode {
    /// Its node number.
    #[cfg_attr(feature = "openapi", schema(example = 825_542_997))]
    pub num: u32,
    /// Its node id, `!` and eight hex digits.
    #[cfg_attr(feature = "openapi", schema(example = "!3134c955"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(example = "2.8.2.d5f1619"))]
    pub firmware_version: Option<String>,
    /// Its hardware model, as the protobufs name it.
    #[cfg_attr(feature = "openapi", schema(example = "HELTEC_V3"))]
    pub hw_model: Option<String>,
    /// Its device role, as the protobufs name it.
    #[cfg_attr(feature = "openapi", schema(example = "CLIENT_BASE"))]
    pub role: Option<String>,
    /// Its LoRa region, as the protobufs name it.
    #[cfg_attr(feature = "openapi", schema(example = "EU_868"))]
    pub region: Option<String>,
    /// Its modem preset, as the protobufs name it, or none if it uses custom
    /// LoRa settings.
    #[cfg_attr(feature = "openapi", schema(example = "LONG_FAST"))]
    pub modem_preset: Option<String>,
}

/// A channel configured on the radio. Its key is never sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Channel {
    /// Its slot on the radio, 0 to 7.
    pub index: u32,
    /// Its name. Empty for a primary channel that takes the modem preset's.
    #[cfg_attr(feature = "openapi", schema(example = "Galicia"))]
    pub name: String,
    /// `PRIMARY`, `SECONDARY` or `DISABLED`.
    #[cfg_attr(feature = "openapi", schema(example = "SECONDARY"))]
    pub role: String,
}

/// Where a node says it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Position {
    /// Degrees north.
    #[cfg_attr(feature = "openapi", schema(example = 43.3623))]
    pub latitude: f64,
    /// Degrees east.
    #[cfg_attr(feature = "openapi", schema(example = -8.4115))]
    pub longitude: f64,
    /// Metres above sea level.
    pub altitude: Option<i32>,
}

/// A node of the mesh, as heard from the radio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Node {
    #[cfg_attr(feature = "openapi", schema(example = 2_429_395_573_u32))]
    pub num: u32,
    #[cfg_attr(feature = "openapi", schema(example = "!90cda275"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(example = "StaLocaia"))]
    pub long_name: Option<String>,
    #[cfg_attr(feature = "openapi", schema(example = "stlo"))]
    pub short_name: Option<String>,
    /// Its hardware model, as the protobufs name it.
    #[cfg_attr(feature = "openapi", schema(example = "RAK4631"))]
    pub hw_model: Option<String>,
    /// Its device role, as the protobufs name it.
    #[cfg_attr(feature = "openapi", schema(example = "ROUTER"))]
    pub role: Option<String>,
    pub position: Option<Position>,
    /// When it was last heard, in seconds since the epoch.
    pub last_heard: Option<u32>,
    /// Signal to noise ratio of the last packet heard straight from it, in dB.
    pub snr: Option<f32>,
    /// Received signal strength of the last packet heard from it, in dBm.
    pub rssi: Option<i32>,
    /// How many hops away it was last heard.
    pub hops_away: Option<u32>,
    /// Whether it was last heard through MQTT rather than over the air.
    pub via_mqtt: bool,
    /// Whether it is a favorite on the radio.
    pub favorite: bool,
    /// Battery level, in percent; over 100 means it is powered.
    pub battery_level: Option<u32>,
    /// Battery voltage, in volts.
    pub voltage: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_travel_as_codes() {
        let body = Error {
            error: ErrorCode::NotFound,
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"not_found"}"#
        );
    }
}
