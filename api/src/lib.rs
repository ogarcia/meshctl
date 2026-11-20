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
    /// The radio is not connected, so nothing can be sent through it now.
    RadioNotConnected,
    /// A message needs some text, of up to 200 bytes, and either a channel the
    /// radio has or a node, not both.
    InvalidMessage,
    /// Not a node that can be traced: everyone, or the radio itself.
    InvalidNode,
    /// The radio traced a route too recently; try again in a moment.
    TooSoon,
    /// Not settings the radio can have; the reference says what each takes.
    InvalidSettings,
    /// The radio did not answer the request a change of settings starts with.
    RadioDidNotAnswer,
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
    /// Whether meshctl connects to it on its own, when it starts and whenever
    /// the link drops.
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

/// How a message is identified in the API.
pub type MessageId = i64;

/// The node number that stands for everyone: what a channel message is
/// addressed to.
pub const BROADCAST: u32 = 0xffff_ffff;

/// Where a message sent is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    /// Heard from someone else.
    Received,
    /// Handed to the radio, with no answer yet.
    Sending,
    /// Heard relayed by another node, which for a channel message is as far
    /// as anyone can tell.
    Sent,
    /// Acknowledged by the node it was sent to.
    Delivered,
    /// The radio gave up on it; `error` says why.
    Failed,
}

/// A text message, heard or sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Message {
    pub id: MessageId,
    /// The packet's id on the mesh, which replies and reactions refer to.
    pub packet_id: u32,
    /// The node it came from; 0 when that is not known, as for a MeshCore
    /// channel message whose sender's name is no contact's.
    pub from: u32,
    /// The sender's name as the message itself gives it: on MeshCore
    /// channels, the only thing that says who wrote.
    pub sender_name: Option<String>,
    /// [`BROADCAST`] (4294967295) for a channel message, a node for a direct
    /// one.
    pub to: u32,
    /// The channel's slot on the radio.
    pub channel: u32,
    pub text: String,
    /// When it was heard or sent, in seconds since the epoch.
    pub at: i64,
    /// Whether this radio sent it: from meshctl, or from another client of
    /// the same radio.
    pub outgoing: bool,
    pub status: MessageStatus,
    /// Why it failed, as the protobufs name it.
    #[cfg_attr(feature = "openapi", schema(example = "MAX_RETRANSMIT"))]
    pub error: Option<String>,
    /// The packet it answers, if any.
    pub reply_id: Option<u32>,
    /// Whether it is a reaction (an emoji) to `reply_id`.
    pub emoji: bool,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub hops_away: Option<u32>,
    pub via_mqtt: bool,
}

/// A message to send: to a channel, or to a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewMessage {
    /// Up to 200 bytes of it.
    #[cfg_attr(feature = "openapi", schema(example = "hola"))]
    pub text: String,
    /// The channel's slot, for a channel message.
    #[serde(default)]
    pub channel: Option<u32>,
    /// The node's number, for a direct message.
    #[serde(default)]
    pub node: Option<u32>,
    /// The packet it answers, if any.
    #[serde(default)]
    pub reply_id: Option<u32>,
}

/// Where messages are exchanged: a channel, or a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Peer {
    Channel { channel: u32 },
    Node { node: u32 },
}

/// A conversation, and where it was left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Conversation {
    pub peer: Peer,
    /// The newest message in it, if any.
    pub last: Option<Message>,
}

/// How a traceroute is identified in the API.
pub type TracerouteId = i64;

/// Where a traceroute is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TracerouteStatus {
    /// Sent, with no answer yet.
    Pending,
    /// Answered: the routes there and back are known.
    Done,
    /// No answer: `error` says why, `TIMEOUT` if none came in time.
    Failed,
}

/// A node on a route, and how well it heard the hop before it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Hop {
    /// None for a relay that could not be told.
    pub node: Option<u32>,
    /// The SNR it heard the hop before it at, in dB. None for the first node
    /// of a route, and where it could not be told.
    pub snr: Option<f32>,
}

/// A traceroute from a radio to a node: the route there and the route back,
/// each from its first node to its last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Traceroute {
    pub id: TracerouteId,
    /// The node traced.
    pub target: u32,
    /// When it was sent, in seconds since the epoch.
    pub at: i64,
    pub status: TracerouteStatus,
    /// Why it failed, as the protobufs name it, or `TIMEOUT`.
    #[cfg_attr(feature = "openapi", schema(example = "NO_RESPONSE"))]
    pub error: Option<String>,
    /// From the radio to the node traced, once answered.
    pub towards: Vec<Hop>,
    /// From the node traced back to the radio, once answered.
    pub back: Vec<Hop>,
}

/// A node to trace the route to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewTraceroute {
    /// The node's number.
    pub node: u32,
}

/// Who the radio says it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OwnerSettings {
    /// Up to 39 bytes.
    #[cfg_attr(feature = "openapi", schema(example = "Faro de Hércules"))]
    pub long_name: String,
    /// Up to 4 bytes: four letters, or one emoji.
    #[cfg_attr(feature = "openapi", schema(example = "FARO"))]
    pub short_name: String,
}

/// What the radio does in the mesh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceSettings {
    /// One of `choices.roles`.
    #[cfg_attr(feature = "openapi", schema(example = "CLIENT"))]
    pub role: String,
}

/// How the radio transmits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoraSettings {
    /// One of `choices.regions`.
    #[cfg_attr(feature = "openapi", schema(example = "EU_868"))]
    pub region: String,
    /// Whether it uses `modem_preset`. A radio set up by hand with its own
    /// bandwidth, spreading factor and coding rate keeps them, but they are
    /// not changed here.
    pub use_preset: bool,
    /// One of `choices.presets`.
    #[cfg_attr(feature = "openapi", schema(example = "LONG_FAST"))]
    pub modem_preset: String,
    /// How many times a packet it sends may be relayed: 1 to 7.
    pub hop_limit: u32,
    /// Whether it transmits at all.
    pub tx_enabled: bool,
}

/// What a channel's key is. The key itself never leaves the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChannelKey {
    /// No encryption.
    None,
    /// The well known default key, which everyone has.
    Default,
    /// One of the well known variants of the default key.
    Simple,
    /// A key of its own, 128 bits.
    Aes128,
    /// A key of its own, 256 bits.
    Aes256,
}

/// One of the radio's channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelSettings {
    /// Its slot, 0 to 7; 0 is the primary.
    pub index: u32,
    /// `PRIMARY`, `SECONDARY` or `DISABLED`.
    pub role: String,
    /// Up to 11 bytes. Empty for a primary channel that takes the modem
    /// preset's name.
    pub name: String,
    pub key: ChannelKey,
    /// Whether what it hears goes up to MQTT.
    pub uplink: bool,
    /// Whether what MQTT has comes down to it.
    pub downlink: bool,
}

/// The codes each setting with a fixed set of values takes, as the
/// protobufs this daemon was built with name them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingsChoices {
    pub roles: Vec<String>,
    pub regions: Vec<String>,
    pub presets: Vec<String>,
}

/// The radio's own settings, as it last sent them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeSettings {
    pub owner: OwnerSettings,
    pub device: DeviceSettings,
    pub lora: LoraSettings,
    /// All eight slots, in order.
    pub channels: Vec<ChannelSettings>,
    pub choices: SettingsChoices,
}

/// What to do with a channel's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum KeyChange {
    /// Leave it as it is.
    #[default]
    Keep,
    /// No encryption.
    None,
    /// The well known default key.
    Default,
    /// A new random key of 128 bits.
    NewAes128,
    /// A new random key of 256 bits.
    NewAes256,
}

/// A channel as it is to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelChange {
    pub index: u32,
    pub role: String,
    pub name: String,
    #[serde(default)]
    pub key: KeyChange,
    pub uplink: bool,
    pub downlink: bool,
}

/// What to change of the radio's settings: each part given is set whole,
/// each part left out stays as it is.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeSettingsChange {
    #[serde(default)]
    pub owner: Option<OwnerSettings>,
    #[serde(default)]
    pub device: Option<DeviceSettings>,
    #[serde(default)]
    pub lora: Option<LoraSettings>,
    /// Only the channels to change.
    #[serde(default)]
    pub channels: Vec<ChannelChange>,
}

/// What a setting holds, and so how it is shown and what it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// `true` or `false`.
    Bool,
    /// A whole number, within `min` and `max` when given.
    Integer,
    /// A number, within `min` and `max` when given.
    Number,
    Text,
    /// One of `choices`, by its code.
    Choice,
}

/// One setting of a section, as the protobufs describe it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingsField {
    /// Its path in the section, with dots into what it nests.
    #[cfg_attr(feature = "openapi", schema(example = "position_broadcast_secs"))]
    pub name: String,
    /// What the protobufs call it, in English.
    #[cfg_attr(feature = "openapi", schema(example = "Broadcast Interval"))]
    pub label: Option<String>,
    /// What the protobufs say of it, in English.
    pub description: Option<String>,
    #[cfg_attr(feature = "openapi", schema(example = "seconds"))]
    pub unit: Option<String>,
    pub kind: FieldKind,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// For a choice, the codes it takes.
    pub choices: Vec<String>,
    /// A password or a key: never shown, only whether it is set, and only
    /// ever written.
    pub secret: bool,
    /// Whether it is only for radios built by hand.
    pub diy_only: bool,
    /// What it is now; none for a secret.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub value: Option<serde_json::Value>,
    /// Whether it holds something: for a secret, whether one is set.
    pub set: bool,
}

/// A part of the radio's configuration, or of one of its modules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingsSection {
    /// `config.` or `module.` and its name in the protobufs.
    #[cfg_attr(feature = "openapi", schema(example = "module.telemetry"))]
    pub id: String,
    /// Its name in the protobufs.
    #[cfg_attr(feature = "openapi", schema(example = "telemetry"))]
    pub name: String,
    /// Whether it is a module rather than the radio's own configuration.
    pub module: bool,
    pub fields: Vec<SettingsField>,
}

/// New values for some settings of a section, by their names. What is not
/// given stays as it is.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SectionChange {
    #[cfg_attr(feature = "openapi", schema(value_type = Object, example = json!({"device_update_interval": 1800})))]
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
}

/// What a change of settings set off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingsApplied {
    /// Whether the radio reboots to apply them, and so drops its link for a
    /// moment.
    pub reboots: bool,
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
