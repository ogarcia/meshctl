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
    /// Not a node that can be traced or asked: everyone, or the radio itself.
    InvalidNode,
    /// The radio traced a route too recently, or the same request is still
    /// waiting for its answer; try again in a moment.
    TooSoon,
    /// Not settings the radio can have; the reference says what each takes.
    InvalidSettings,
    /// The radio did not answer the request a change of settings starts with.
    RadioDidNotAnswer,
    /// The radio would not do it.
    RadioRefused,
    /// The radio has no such channel, or it is disabled.
    InvalidChannel,
    /// Not a button a radio can have: a colour that is not `#rrggbb`, text of
    /// other than up to four bytes of text, or an emoji that is not one.
    InvalidBadge,
    /// Not an order of the radios: every radio's id, each once, and no other.
    InvalidOrder,
    /// Not a place on the map: a latitude past ±90, a longitude past ±180, or
    /// a zoom past what maps have.
    InvalidMapView,
    /// A channel with the very name and key of another the radio has: on
    /// the air, and to the radio, it would be that same channel.
    DuplicateChannel,
    /// A position more precise than a public channel takes: the radio would
    /// cut it down.
    PublicChannelPrecision,
    /// Not a link meshctl reads: a Meshtastic contact or channels.
    InvalidLink,
    /// The radio has not the free slots the link's channels take.
    NoFreeSlots,
    /// A node the radio knows no path to, for a request that goes only
    /// direct, as MeshCore's anonymous ones: once its path is known, from
    /// its advert or a message, it can be asked.
    NoPath,
    /// A room, repeater or sensor did not take the password.
    WrongPassword,
    /// Not a node the radio knows that takes logins: a room, a repeater or a
    /// sensor.
    NoLogin,
    /// A node the radio no longer has in its own list: meshctl alone keeps
    /// it, as heard before.
    NotOnRadio,
    /// What only a node's admin may do: the radio is logged in to it as
    /// less.
    NotAdmin,
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
    /// Whether it has a button on the interface's rail.
    pub shown: bool,
    /// How its button looks.
    pub badge: Badge,
    /// Its node's short name, for a Meshtastic radio that told it: what its
    /// button shows when nothing else was picked for it.
    #[cfg_attr(feature = "openapi", schema(example = "G3L0"))]
    pub short_name: Option<String>,
    pub link: LinkState,
    /// When it was last connected and in sync, in seconds since the epoch;
    /// none if it never was. What meshctl shows of a radio that is not
    /// connected is what it knew then.
    pub last_connected: Option<i64>,
    /// How many messages others wrote in its conversations that were not
    /// read yet, all of them.
    #[serde(default)]
    pub unread: u32,
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
    /// Whether it has a button on the interface's rail. A radio hidden from
    /// it is still kept, and connected to if enabled.
    #[serde(default = "enabled")]
    pub shown: bool,
    /// How its button looks.
    #[serde(default)]
    pub badge: Badge,
}

fn enabled() -> bool {
    true
}

/// The colours of the interface's palette for radios' buttons, by name: each
/// one is shown in a tone of its own on each theme, light on the light one and
/// deep on the dark one, as the rail's own buttons are.
pub const BADGE_COLORS: [&str; 10] = [
    "amber", "coral", "pink", "grape", "violet", "blue", "cyan", "teal", "lime", "gray",
];

/// How a radio's button looks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct Badge {
    /// Its colour: one of [`BADGE_COLORS`] by its name, or any other as
    /// `#rrggbb`; none for the rail's own.
    #[cfg_attr(feature = "openapi", schema(example = "amber"))]
    pub color: Option<String>,
    pub content: BadgeContent,
}

/// What a radio's button shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BadgeContent {
    /// The first letters of its name.
    #[default]
    Initials,
    /// Up to four bytes of text, as Meshtastic's short names: four letters,
    /// fewer with accents, or one emoji.
    Text { text: String },
    /// One emoji.
    Emoji { emoji: String },
    /// One of the interface's icons.
    Icon { icon: BadgeIcon },
}

/// The icons a radio's button can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum BadgeIcon {
    Meshtastic,
    Meshcore,
    Home,
    Building,
    Tower,
    Antenna,
    Router,
    Satellite,
    Car,
    Truck,
    Motorbike,
    Bike,
    Walk,
    Boat,
    Plane,
    Caravan,
    Mountain,
    Tent,
    Tree,
    Sun,
    SolarPanel,
    Lighthouse,
    Anchor,
    Compass,
    Backpack,
    Briefcase,
    Flask,
    Walkie,
    Flag,
    Star,
    Bolt,
    Heart,
}

impl BadgeIcon {
    /// Every one, in the order the interface offers them: eight a row, the
    /// radios and places, the ways of going, the outdoors, and things.
    pub const ALL: [Self; 32] = [
        Self::Meshtastic,
        Self::Meshcore,
        Self::Home,
        Self::Building,
        Self::Tower,
        Self::Antenna,
        Self::Router,
        Self::Satellite,
        Self::Car,
        Self::Truck,
        Self::Motorbike,
        Self::Bike,
        Self::Walk,
        Self::Boat,
        Self::Plane,
        Self::Caravan,
        Self::Mountain,
        Self::Tent,
        Self::Tree,
        Self::Sun,
        Self::SolarPanel,
        Self::Lighthouse,
        Self::Anchor,
        Self::Compass,
        Self::Backpack,
        Self::Briefcase,
        Self::Flask,
        Self::Walkie,
        Self::Flag,
        Self::Star,
        Self::Bolt,
        Self::Heart,
    ];
}

/// Where the interface's map was left for a radio: its centre and its zoom,
/// to go back to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MapView {
    #[cfg_attr(feature = "openapi", schema(example = 43.36))]
    pub latitude: f64,
    #[cfg_attr(feature = "openapi", schema(example = -8.41))]
    pub longitude: f64,
    #[cfg_attr(feature = "openapi", schema(example = 11.0))]
    pub zoom: f64,
}

impl MapView {
    /// The most a map zooms in.
    pub const ZOOM_MAX: f64 = 22.0;

    /// Whether it is a place on the map.
    pub fn is_valid(&self) -> bool {
        (-90.0..=90.0).contains(&self.latitude)
            && (-180.0..=180.0).contains(&self.longitude)
            && (0.0..=Self::ZOOM_MAX).contains(&self.zoom)
    }
}

/// The order of the radios, as the interface lists them: every radio's id,
/// each once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RadioOrder {
    pub radios: Vec<RadioId>,
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
    /// Since when the link is as it is, in seconds since the epoch, while
    /// meshctl runs; none until it first changes.
    #[serde(default)]
    pub since: Option<i64>,
    pub local: Option<LocalNode>,
}

/// How far a conversation was read: up to a message, the newest seen. A
/// conversation is read on, never back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConversationRead {
    /// The channel's slot, for a channel's conversation.
    #[serde(default)]
    pub channel: Option<u32>,
    /// The node's number, for the direct messages exchanged with it.
    #[serde(default)]
    pub node: Option<u32>,
    pub up_to: MessageId,
}

/// A link to add a node or channels from, as the apps share them: a
/// Meshtastic contact (`https://meshtastic.org/v/#…`) or channels
/// (`https://meshtastic.org/e/#…`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Link {
    #[cfg_attr(
        feature = "openapi",
        schema(example = "https://meshtastic.org/v/#CM2rNBISCglDYXNh")
    )]
    pub url: String,
    /// For channels: added to the free slots, or in place of every one.
    #[serde(default)]
    pub mode: LinkMode,
    /// For channels replacing, whether the link's LoRa settings come too:
    /// the radio reboots, and hears only those on them.
    #[serde(default)]
    pub lora: bool,
}

/// How a link's channels go to the radio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinkMode {
    /// To the free slots, those the radio has left out.
    #[default]
    Add,
    /// In place of every one: the first the primary, the slots left over
    /// emptied.
    Replace,
}

/// What a link holds, read before anything is added from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkContents {
    /// A node, as its node info says it.
    Contact {
        node: u32,
        /// `!` and eight hex digits.
        id: String,
        long_name: String,
        short_name: String,
        /// As the protobufs name it.
        hw_model: Option<String>,
        /// Whether it brings its public key, which private messages take.
        key: bool,
        /// Whether the radio knows of it already.
        known: bool,
    },
    /// Channels, each added to a free slot unless the radio has it.
    Channels {
        channels: Vec<LinkChannel>,
        /// How many slots the radio has free.
        free_slots: u32,
        /// The modem preset the link's channels are on, by its name, or
        /// `Custom`; none if it says nothing of it.
        preset: Option<String>,
        /// Whether that, or the region, is not the radio's: the channels
        /// would be heard only by those on the link's settings, which change
        /// only when asked.
        lora_differs: bool,
        /// The region the link's settings are for, as the protobufs name
        /// it; none if it says none.
        region: Option<String>,
        /// Whether the link is meant to replace the radio's channels, as the
        /// apps share a whole configuration, rather than to add to them.
        replaces: bool,
        /// The radio's channels a replacing would take away, by name.
        removed: Vec<String>,
    },
}

/// A channel a link holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LinkChannel {
    /// Its name; an unnamed one by its preset's.
    pub name: String,
    pub key: ChannelKey,
    /// Whether the radio has it already, name and key.
    pub present: bool,
}

/// A MeshCore radio's own settings: what it advertises, where, and how it
/// transmits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreSettings {
    /// What it advertises itself as: up to 31 bytes.
    #[cfg_attr(feature = "openapi", schema(example = "Casa"))]
    pub name: String,
    /// Where it says it is; none for nowhere.
    pub position: Option<MeshcorePosition>,
    /// In kHz: 150000 to 2500000.
    #[cfg_attr(feature = "openapi", schema(example = 869618))]
    pub frequency: u32,
    /// In Hz: 7000 to 500000.
    #[cfg_attr(feature = "openapi", schema(example = 62500))]
    pub bandwidth: u32,
    /// 5 to 12.
    #[cfg_attr(feature = "openapi", schema(example = 8))]
    pub spreading_factor: u8,
    /// 5 to 8.
    #[cfg_attr(feature = "openapi", schema(example = 8))]
    pub coding_rate: u8,
    /// In dBm: -9 to `max_tx_power`.
    #[cfg_attr(feature = "openapi", schema(example = 22))]
    pub tx_power: i8,
    /// The most its board allows, in dBm; read only.
    #[serde(default)]
    pub max_tx_power: u8,
    /// Whether it repeats what it hears as well; none on firmware without
    /// it. Only on 433.000, 869.495 or 918.000 MHz.
    pub repeat: Option<bool>,
    /// Whether nodes heard advertising are added to its contacts only by
    /// hand.
    pub manual_add_contacts: bool,
    /// Whether its adverts say where it is.
    pub share_position: bool,
    /// Who it answers telemetry to.
    pub telemetry: MeshcoreTelemetry,
    /// Whether it sends its acknowledgements more than once.
    pub multi_acks: bool,
    /// How many bytes each hop's hash takes in what it sends: 1 to 3. More
    /// tell repeaters apart better, and make each hop longer. None on
    /// firmware without it.
    #[serde(default)]
    pub path_hash_bytes: Option<u8>,
    /// The region what it floods is scoped to, by its name (without `#`),
    /// heard only by the repeaters that serve it; none floods everywhere.
    #[serde(default)]
    pub default_scope: Option<String>,
    /// Whether its firmware has a default scope; read only.
    #[serde(default)]
    pub scopes: bool,
    /// Which nodes it adds to its contacts by itself; none on firmware
    /// without it.
    #[serde(default)]
    pub auto_add: Option<MeshcoreAutoAdd>,
    /// How it paces what it passes on; none on firmware without it.
    #[serde(default)]
    pub tuning: Option<MeshcoreTuning>,
    /// The frequencies it can repeat on, in kHz, both ends included; read
    /// only.
    #[serde(default)]
    pub repeat_frequencies: Vec<MeshcoreFrequencies>,
}

/// The regions a MeshCore radio's repeaters and rooms said they repeat on,
/// each as it last said; and whether it is asking them now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreRegions {
    /// Whether it is asking them now, one at a time.
    pub searching: bool,
    /// How many are still to be asked in the search going on.
    pub left: u32,
    /// Each region said, the most served nearby first.
    pub regions: Vec<MeshcoreRegion>,
    /// How many repeat floods with no region, and how many of those it
    /// hears straight.
    pub unscoped: u32,
    pub unscoped_direct: u32,
    /// Each repeater or room that answered, as it last did.
    pub repeaters: Vec<MeshcoreRepeaterRegions>,
}

/// A region, and how many repeaters serve it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreRegion {
    /// Without `#`.
    #[cfg_attr(feature = "openapi", schema(example = "galicia"))]
    pub name: String,
    pub repeaters: u32,
    /// Those of them the radio hears straight, which are what anything it
    /// floods reaches first.
    pub direct: u32,
}

/// What a repeater or room said of its regions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreRepeaterRegions {
    pub node: u32,
    pub name: Option<String>,
    /// Hops away, as its path says; none if no path is known now.
    pub hops: Option<u32>,
    pub regions: Vec<String>,
    pub unscoped: bool,
    /// When it answered, in seconds since the epoch.
    pub answered_at: i64,
}

/// Which nodes heard advertising a MeshCore radio adds by itself. While
/// `manual_add_contacts` is off it adds every kind; on, only those here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreAutoAdd {
    pub companions: bool,
    pub repeaters: bool,
    pub rooms: bool,
    pub sensors: bool,
    /// Whether, full, it overwrites the oldest contact that is not a
    /// favourite.
    pub overwrite_oldest: bool,
    /// Up to how many hops away: 0 only those heard straight, up to 63;
    /// none for any.
    pub max_hops: Option<u8>,
}

/// How a MeshCore radio paces what it passes on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreTuning {
    /// The base of how long it waits before passing on what it hears: 0
    /// to 20.
    #[cfg_attr(feature = "openapi", schema(example = 0.0))]
    pub rx_delay: f64,
    /// How much of the air it leaves free after sending, against what it
    /// took: 0 to 9.
    #[cfg_attr(feature = "openapi", schema(example = 1.0))]
    pub airtime_factor: f64,
}

/// A range of frequencies, in kHz, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreFrequencies {
    pub from: u32,
    pub to: u32,
}

/// A MeshCore radio's channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreChannels {
    /// How many slots it has, set or not.
    pub slots: u32,
    /// Those set, in slot order.
    pub channels: Vec<MeshcoreChannel>,
}

/// A MeshCore radio's channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreChannel {
    pub index: u32,
    #[cfg_attr(feature = "openapi", schema(example = "galicia"))]
    pub name: String,
    pub kind: MeshcoreChannelKind,
    /// The region what is sent on it floods in, without `#`, or `*` for
    /// none: kept by meshctl, as the radio keeps none per channel. None
    /// for the radio's default.
    #[serde(default)]
    pub scope: Option<String>,
}

/// A MeshCore channel's region, alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreChannelScope {
    /// Without `#`, or `*` for none; left out, the radio's default.
    #[cfg_attr(feature = "openapi", schema(example = "galicia"))]
    #[serde(default)]
    pub scope: Option<String>,
}

/// What a MeshCore channel's secret is: the one every radio has, one its
/// name gives, or one of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MeshcoreChannelKind {
    /// The `Public` channel every MeshCore radio comes with.
    Public,
    /// A `#name`, whose secret is the name's, as the apps make them: anyone
    /// who knows the name is in.
    Hashtag,
    /// One with a secret of its own, to be shared.
    Private,
}

/// A MeshCore channel as it is to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreChannelChange {
    /// Up to 31 bytes.
    #[cfg_attr(feature = "openapi", schema(example = "#galicia"))]
    pub name: String,
    /// Its secret, 16 bytes in hex. Left out, a `#name` takes its name's,
    /// and any other a new random one; a channel kept by its name keeps
    /// its own.
    #[serde(default)]
    pub secret: Option<String>,
    /// The region what is sent on it floods in, without `#`, or `*` for
    /// none; left out, the radio's default.
    #[serde(default)]
    pub scope: Option<String>,
}

/// To whom a radio says who it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Advert {
    /// Flooded to the whole mesh, rather than to those it hears directly.
    pub flood: bool,
}

/// A place, in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcorePosition {
    #[cfg_attr(feature = "openapi", schema(example = 43.3623))]
    pub latitude: f64,
    #[cfg_attr(feature = "openapi", schema(example = -8.4115))]
    pub longitude: f64,
}

/// Who a MeshCore radio answers each kind of telemetry to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreTelemetry {
    /// Its battery and the like.
    pub base: TelemetryShare,
    pub location: TelemetryShare,
    /// Its sensors.
    pub environment: TelemetryShare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TelemetryShare {
    Nobody,
    /// The contacts allowed to, each by its own flag.
    Allowed,
    Anyone,
}

/// A MeshCore radio's figures, asked for now: each none if its firmware
/// does not give it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MeshcoreStatus {
    pub battery_mv: Option<u16>,
    /// Its storage, in KB.
    pub storage_used_kb: Option<u32>,
    pub storage_total_kb: Option<u32>,
    /// Since it started, in seconds.
    pub uptime: Option<u32>,
    /// What waits to be sent.
    pub queue: Option<u8>,
    /// In dBm.
    pub noise_floor: Option<i16>,
    pub last_rssi: Option<i8>,
    pub last_snr: Option<f32>,
    /// Seconds spent sending and receiving since it started.
    pub tx_air_secs: Option<u32>,
    pub rx_air_secs: Option<u32>,
    pub packets_received: Option<u32>,
    pub packets_sent: Option<u32>,
    pub sent_flood: Option<u32>,
    pub sent_direct: Option<u32>,
    pub received_flood: Option<u32>,
    pub received_direct: Option<u32>,
    pub receive_errors: Option<u32>,
}

/// How long what meshctl keeps is kept, each in days: `null` for ever.
/// Whatever is left out takes its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct Retention {
    /// The packets heard: the live view, the charts, what answered a
    /// request. Each node's latest of each kind stays, however old, and
    /// the requests and traces go with them.
    #[cfg_attr(feature = "openapi", schema(example = 30, minimum = 1))]
    pub packets: Option<u32>,
    /// The nodes meshctl alone keeps, not heard for so long. Never one the
    /// radio has, nor a favourite, nor one it is not known of yet.
    #[cfg_attr(feature = "openapi", schema(example = 90, minimum = 1))]
    pub nodes: Option<u32>,
    /// The messages, in channels and direct.
    #[cfg_attr(feature = "openapi", schema(example = json!(null), minimum = 1))]
    pub messages: Option<u32>,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            packets: Some(30),
            nodes: Some(90),
            messages: None,
        }
    }
}

impl Retention {
    /// The longest a time may be, in days: a hundred years.
    pub const LONGEST: u32 = 36_500;

    /// Whether every time is one there can be.
    pub fn valid(&self) -> bool {
        [self.packets, self.nodes, self.messages]
            .into_iter()
            .flatten()
            .all(|days| (1..=Self::LONGEST).contains(&days))
    }
}

/// What meshctl keeps, and how much room it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Storage {
    /// The database's size, in bytes, room freed and not given back yet
    /// left out.
    pub bytes: u64,
    pub packets: u64,
    /// When the oldest packet kept was heard, in seconds since the epoch.
    pub oldest_packet: Option<i64>,
    pub messages: u64,
    pub nodes: u64,
    /// The messages of channels no radio has any more.
    pub gone_channels: Gone,
    /// The direct messages with nodes meshctl no longer keeps.
    pub gone_directs: Gone,
}

/// Conversations left behind, and their messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Gone {
    pub conversations: u64,
    pub messages: u64,
}

/// What a cleaning took away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Cleaned {
    pub packets: u64,
    pub nodes: u64,
    pub messages: u64,
    /// The room given back, in bytes.
    pub bytes: u64,
}

/// An emoji picked in the interface, to count among the most used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EmojiUsed {
    #[cfg_attr(feature = "openapi", schema(example = "👍"))]
    pub emoji: String,
}

/// How to share a radio's own node with others: a link its kind's apps
/// open, in a QR or as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Contact {
    /// For Meshtastic, `https://meshtastic.org/v/#` and the contact; for
    /// MeshCore, `meshcore://contact/add` with its name, key and kind.
    #[cfg_attr(
        feature = "openapi",
        schema(example = "https://meshtastic.org/v/#CM2rNBISCglDYXNh")
    )]
    pub url: String,
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
    /// How far, in metres, it may be from where the node is, when the node
    /// shares it rounded, as most do; none when it is exact or does not say.
    #[cfg_attr(feature = "openapi", schema(example = 2918))]
    pub radius: Option<u32>,
    /// How the node came by it, when it says.
    pub source: Option<PositionSource>,
}

/// How a node came by its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PositionSource {
    /// Set by hand, as a fixed position.
    Manual,
    /// From its own GPS.
    Internal,
    /// From a GPS outside it: a phone, say.
    External,
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
    /// Its public key, in base64, when known. Meshtastic sends a node what
    /// is for it alone (but node info, positions and traceroutes) encrypted
    /// to this key, whatever the channel; MeshCore names nodes by it.
    #[cfg_attr(
        feature = "openapi",
        schema(example = "K3xOcv5PxFq1aGv0Yd0m9q8tYp2bR8sWkVc4nZ1uJ0E=")
    )]
    pub public_key: Option<String>,
    /// Whether its owner is a licensed radio amateur, who sends nothing
    /// encrypted and is sent nothing encrypted either.
    pub licensed: bool,
    /// Whether it tells that nobody reads messages sent to it, as a
    /// repeater or a sensor of the mesh's own: they can be sent all the same.
    pub unmessagable: bool,
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
    /// How busy it hears the channel, as a percentage of the time.
    pub channel_utilization: Option<f32>,
    /// Whether the radio has it in its own list (`false`: meshctl alone
    /// keeps it, as heard before). None until the radio has been read in
    /// full since meshctl started, as only then is it known.
    #[serde(default)]
    pub on_radio: Option<bool>,
    /// What it says of itself, as it last sent it (`NODE_STATUS_APP`); the
    /// radio's own, as its settings have it.
    #[serde(default)]
    pub status: Option<String>,
    /// When it said it, in seconds since the epoch; none for the radio's own.
    #[serde(default)]
    pub status_at: Option<u32>,
}

/// What a node last told of each kind, as the radio heard it: whether it
/// sent it of its own accord or answering a request. None of a kind it
/// never sent, or not since it was last heard of.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeLatest {
    /// Who it is.
    pub node_info: Option<Packet>,
    pub position: Option<Packet>,
    /// Its battery, voltage, how busy it hears the channel, how much it
    /// sends and how long it has been on.
    pub device: Option<Packet>,
    /// What its environment sensors read.
    pub environment: Option<Packet>,
    /// What its power sensors read.
    pub power: Option<Packet>,
    /// How it sees the mesh: nodes online, packets, noise.
    pub local_stats: Option<Packet>,
    /// The nodes it hears directly.
    pub neighbors: Option<Packet>,
}

/// A node's telemetry over a while, as series of points to chart: what it
/// sent of its own accord or answering, and the signal it was heard at.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeTelemetry {
    /// When the oldest point the node has is, of any series and however long
    /// ago, in seconds since the epoch: how far back there is to look. None
    /// when it has none.
    pub oldest: Option<i64>,
    /// Each measure with any point in the while, in no particular order.
    pub series: Vec<TelemetrySeries>,
}

/// One measure over a while, the oldest point first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TelemetrySeries {
    /// What it measures, as its kind and its field in the protobufs say it:
    /// `device.battery_level`, `environment.temperature`,
    /// `power.ch1_voltage`; `signal.snr` and `signal.rssi` for how the radio
    /// heard it directly, and `signal.hops` for how many hops each packet
    /// took, only when any took more than none.
    #[cfg_attr(feature = "openapi", schema(example = "device.voltage"))]
    pub metric: String,
    /// At most a few hundred: a long while's are averaged into as many.
    pub points: Vec<TelemetryPoint>,
}

/// A value at a time, in seconds since the epoch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TelemetryPoint {
    pub at: i64,
    pub value: f64,
}

/// How many of a history were forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Forgotten {
    pub count: u64,
}

/// Whether a node is to be a favourite of the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeFavorite {
    pub favorite: bool,
}

/// Something amiss with a node, found in what it sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeCheck {
    /// Its public key is the same as that of other nodes heard in the latest
    /// week: each of them can read what is sent to the others alone. It
    /// happens with radios that came with the same key, or whose settings
    /// were copied from another; making a new key on the node ends it.
    PublicKeyShared {
        /// The other nodes' numbers, the most recently heard first.
        nodes: Vec<u32>,
    },
    /// Its latest node info carries more than one public key. Either two
    /// radios share its id, or the key was made anew, as on reflashing it:
    /// until the others take the new one, what is sent to it fails. A copy
    /// damaged on the air is not taken for another key.
    PublicKeyConflict {
        /// The keys, in base64, the latest first.
        keys: Vec<String>,
    },
    /// Its latest node info names more than one hardware model: either two
    /// radios share its id, or the radio was replaced.
    HardwareChanged {
        /// As the protobufs name them.
        models: Vec<String>,
    },
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
    /// The region it went flooded in, without `#`: sent scoped to one from
    /// meshctl, or heard in one through a MeshCore radio; `?` for a region
    /// whose key is not known; none otherwise.
    #[serde(default)]
    pub scope: Option<String>,
    /// How many copies of it a MeshCore radio heard: each by a way of its
    /// own, as its routes tell.
    #[serde(default)]
    pub heard: u32,
    /// When its sender says it sent it, by its clock, in seconds since the
    /// epoch: on MeshCore, where replies and reactions name a message by it.
    #[serde(default)]
    pub sent_at: Option<i64>,
    /// How many repeaters it was heard through, each once whatever copies
    /// went through it: for one of ours on a MeshCore channel, which nothing
    /// acknowledges, how it is known to have gone out.
    #[serde(default)]
    pub repeaters: u32,
    /// Who wrote it, for a MeshCore room's post, which comes from the room:
    /// the node its author's key starts with, known or not.
    #[serde(default)]
    pub author: Option<u32>,
}

/// What a MeshCore room let the radio in as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum NodeAccess {
    /// A guest: in a room, reads its posts and cannot post, with no password
    /// or a wrong one where it lets them in; on a repeater or a sensor, is
    /// told the basics.
    ReadOnly,
    /// In a room, reads and posts, with its password.
    ReadWrite,
    /// With its admin's password.
    Admin,
}

/// The radio's login to a MeshCore node: a room, which sends its posts only
/// to those logged in, or a repeater or a sensor, which tell only them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeLogin {
    /// What it was let in as since the radio last connected; none if it has
    /// not logged in since. A node forgets those it cannot reach, and a
    /// guest when it restarts: when it stops answering, log in again.
    pub access: Option<NodeAccess>,
    /// When it logged in, in seconds since the epoch.
    pub at: Option<i64>,
    /// Whether a password is kept for it, which a login with none uses.
    pub remembered: bool,
}

/// One who may log in to a MeshCore node, and as what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ListedAccess {
    /// The node its key starts with, known or not.
    pub node: u32,
    pub access: NodeAccess,
}

/// A command for the console of a MeshCore node the radio is logged in to
/// as its admin, as its firmware's CLI takes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewCommand {
    #[cfg_attr(feature = "openapi", schema(example = "ver"))]
    pub command: String,
}

/// A login to a MeshCore room, repeater or sensor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewNodeLogin {
    /// Up to 15 bytes, kept for the next login once known to be right; none
    /// for the one kept, or with none kept, as a guest.
    #[serde(default)]
    pub password: Option<String>,
}

/// A copy of a message heard, and the way it came.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MessageRoute {
    /// The repeaters it went through, from its sender's side to the radio;
    /// none for one heard straight, or sent along a path.
    pub hops: Vec<RouteHop>,
    /// Whether it was flooded, its path built up as it went.
    pub flooded: bool,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    /// The region it came in, without `#`; `?` for one whose key is not
    /// known.
    pub scope: Option<String>,
    /// When it was heard, in seconds since the epoch.
    pub at: i64,
}
/// A repeater on a route: its hash, the nodes it may be, and the one it
/// most likely is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RouteHop {
    /// The start of its public key, in hex: one to three bytes.
    #[cfg_attr(feature = "openapi", schema(example = "1e35"))]
    pub hash: String,
    /// The nodes the radio knows whose key starts so: none, one, or more
    /// than one it cannot tell apart.
    pub nodes: Vec<u32>,
    /// The node it most likely is, as the route as a whole tells: the
    /// nodes that repeat over those that do not, each hop within reach of
    /// the one before, the last one heard by the radio. A guess, not a
    /// certainty; none when it is likely a node the radio does not know.
    #[serde(default)]
    pub likely: Option<u32>,
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
    /// Whether it is a reaction to `reply_id`, its text an emoji. Not on
    /// MeshCore radios, which have no reactions.
    #[serde(default)]
    pub emoji: bool,
    /// Only on MeshCore radios: the region it floods in, without `#`, or
    /// `*` for none, for this message alone. Left out, its channel's, or
    /// the radio's default.
    #[serde(default)]
    pub scope: Option<String>,
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
    /// How many messages others wrote in it that were not read yet; one's
    /// own and reactions are not counted.
    #[serde(default)]
    pub unread: u32,
    /// The newest message read in it, if any was.
    #[serde(default)]
    pub read_up_to: Option<MessageId>,
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
    /// The channel's slot to trace on, 0 if not said. The node has to have
    /// that channel too.
    #[serde(default)]
    pub channel: u32,
}

/// What a node can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// Who it is. A node answers once a minute at most.
    NodeInfo,
    /// Where it is, at the precision of the channel asked on. Once in three
    /// minutes at most.
    Position,
    /// Its battery, voltage and how busy the channel is.
    DeviceTelemetry,
    /// What its environment sensors read; only a node with some answers.
    EnvironmentTelemetry,
    /// What its power sensors read; only a node with some answers.
    PowerTelemetry,
    /// How it sees the mesh: nodes online, packets, noise.
    LocalStats,
    /// The nodes it hears directly. Once in three minutes at most, from a
    /// Meshtastic radio; from a MeshCore one, logged in to, as for `status`.
    Neighbors,
    /// The regions a MeshCore repeater or room repeats floods scoped to, and
    /// whether it repeats those scoped to none: with no login, only direct,
    /// so to a node the radio knows a path to.
    Regions,
    /// How a MeshCore repeater or room is doing: its battery, the noise it
    /// hears, its packets, how long it has been up. Logged in to, as a guest
    /// at least, which meshctl does first if it has not since the radio
    /// connected.
    Status,
    /// What a MeshCore node's sensors read: its own to a guest, all to its
    /// admin. Logged in to, as for `status`.
    Telemetry,
    /// Who runs a MeshCore node: its name and what its owner says, and its
    /// firmware when logged in to. Not logged in to since the radio
    /// connected, it goes with no login where its path is known, and comes
    /// with no firmware; with no path known, it logs in as for `status`.
    Owner,
    /// Who may log in to a MeshCore node, and as what: only its admin is
    /// told, logged in to as for `status`, with the password kept.
    AccessList,
    /// A MeshCore node heard straight, pinged, with no login: how long there
    /// and back, and how each end heard the other.
    Ping,
    /// The way to a MeshCore node and back, found by a request flooded to
    /// it: each hop by its hash, and the node it most likely is. With no
    /// login to a companion; to a repeater, a room or a sensor, logged in to
    /// as for `status`, as they read requests only from those. The radio
    /// keeps the path it had.
    Route,
}

impl RequestKind {
    /// The pace the radio sends it at, whoever it is for; none when the
    /// radio sends it whenever asked.
    pub const fn pace(self) -> Option<Pace> {
        match self {
            Self::NodeInfo
            | Self::Neighbors
            | Self::Regions
            | Self::Status
            | Self::Telemetry
            | Self::Owner
            | Self::AccessList
            | Self::Ping
            | Self::Route => None,
            Self::Position => Some(Pace::Position),
            Self::DeviceTelemetry
            | Self::EnvironmentTelemetry
            | Self::PowerTelemetry
            | Self::LocalStats => Some(Pace::Telemetry),
        }
    }
}

/// What a radio sends no more often than once in so long, whichever node it
/// is for: the firmware drops what comes sooner without a word, so meshctl
/// does not send it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Pace {
    /// Positions, and asking for them.
    Position,
    /// Telemetry of any kind, and asking for it: device, environment, power
    /// and local stats go at one pace.
    Telemetry,
    Traceroute,
}

impl Pace {
    /// How long, in seconds, the radio takes before it sends the same again.
    pub const fn interval(self) -> i64 {
        match self {
            Self::Position | Self::Telemetry => 10,
            Self::Traceroute => 30,
        }
    }
}

/// How long, in seconds, until a radio sends each kind of paced thing
/// again: 0 when it can now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Waits {
    pub position: i64,
    pub telemetry: i64,
    pub traceroute: i64,
}

impl Waits {
    pub const fn of(&self, pace: Pace) -> i64 {
        match pace {
            Pace::Position => self.position,
            Pace::Telemetry => self.telemetry,
            Pace::Traceroute => self.traceroute,
        }
    }
}

/// Where a request is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    /// Sent, and waiting for the answer.
    Pending,
    Answered,
    /// No answer, and why: `NO_RESPONSE` when the node had nothing to tell,
    /// `TIMEOUT` when nothing came at all, which a node that answered
    /// recently does on purpose.
    Failed,
}

/// Something to ask a node for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewRequest {
    /// The node's number.
    pub node: u32,
    pub kind: RequestKind,
    /// The channel's slot to ask on, 0 if not said. The node has to have that
    /// channel too, and answers on it.
    #[serde(default)]
    pub channel: u32,
}

pub type RequestId = i64;

/// Something a node was asked for, and what it answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Request {
    pub id: RequestId,
    pub node: u32,
    pub kind: RequestKind,
    pub channel: u32,
    pub status: RequestStatus,
    /// Why it failed, as the protobufs name it, or `TIMEOUT`.
    pub error: Option<String>,
    /// When it was sent, in seconds since the epoch.
    pub at: i64,
    pub answered_at: Option<i64>,
    /// The packet that answered it, as heard.
    pub answer: Option<Packet>,
    /// What it answered, for a node that answers with no packet, as
    /// MeshCore nodes do.
    #[serde(default)]
    pub reply: Option<RequestReply>,
}

/// What a node answered a request with, by what it was asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestReply {
    Regions {
        /// The regions it repeats floods scoped to, by their names, without
        /// `#`.
        #[cfg_attr(feature = "openapi", schema(example = json!(["europe", "galicia"])))]
        regions: Vec<String>,
        /// Whether it repeats floods scoped to no region.
        unscoped: bool,
        /// Its clock as it answered, in seconds since the epoch.
        clock: Option<i64>,
    },
    /// How a MeshCore repeater or room is doing.
    Status(NodeStatus),
    /// What a MeshCore node's sensors read: its own, on channel 1, and to
    /// its admin those of the rest.
    Telemetry { readings: Vec<SensorReading> },
    /// The nodes a MeshCore node hears straight: how many in all, and those
    /// it told, the strongest first.
    Neighbors {
        total: u32,
        nodes: Vec<HeardNeighbor>,
    },
    /// Who runs a MeshCore node, as it says.
    Owner {
        #[cfg_attr(feature = "openapi", schema(example = "v1.18.0"))]
        firmware: String,
        name: String,
        /// What its owner wrote of themselves; empty for nothing.
        owner: String,
    },
    /// Who may log in to a MeshCore node, and as what, as its admin is told.
    AccessList { nodes: Vec<ListedAccess> },
    /// A MeshCore node heard straight, pinged: how long there and back, and
    /// how each end heard the other.
    Ping {
        round_trip_ms: u32,
        /// The SNR it heard the radio at, in dB.
        snr_there: f32,
        /// The SNR the radio heard it back at, in dB.
        snr_back: f32,
    },
    /// The way to a MeshCore node and back, as a path discovery found them:
    /// each hop by its hash, with the nodes it may be and the one it most
    /// likely is; there from the radio's side, back from the node's.
    Route {
        there: Vec<RouteHop>,
        back: Vec<RouteHop>,
    },
}

/// How a MeshCore repeater or room says it is doing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodeStatus {
    /// Its battery, in volts.
    pub voltage: f32,
    /// Packets waiting to go out.
    pub queue: u32,
    /// The noise it hears, in dBm.
    pub noise_floor: i32,
    /// The signal of the last packet it heard, in dBm and dB.
    pub last_rssi: i32,
    pub last_snr: f32,
    pub received: u32,
    pub sent: u32,
    pub received_flood: u32,
    pub received_direct: u32,
    pub sent_flood: u32,
    pub sent_direct: u32,
    pub flood_duplicates: u32,
    pub direct_duplicates: u32,
    /// Seconds it spent sending, and since it started.
    pub airtime: u32,
    pub uptime: u32,
    /// Its error flags; 0 for none.
    pub errors: u32,
    /// A repeater's: seconds it spent receiving, and packets it could not
    /// read.
    pub receive_airtime: Option<u32>,
    pub receive_errors: Option<u32>,
    /// A room's: posts it took, and pushed to those logged in.
    pub posted: Option<u32>,
    pub pushed: Option<u32>,
}

/// What a sensor of a node reads, as Cayenne LPP gives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SensorReading {
    /// 1 the node's own; the rest, its sensors.
    pub channel: u32,
    /// What it reads, by Cayenne LPP's type: `voltage` (V), `temperature`
    /// (°C), `humidity` (%), `pressure` (hPa), `current` (A), `power` (W),
    /// `illuminance` (lux), `altitude` (m), `percentage`, `energy` (kWh),
    /// `distance` (m), `place` (latitude, longitude, altitude), and others
    /// by their LPP number.
    #[cfg_attr(feature = "openapi", schema(example = "voltage"))]
    pub kind: String,
    /// One value, or three for a place, an acceleration, a rotation or a
    /// colour.
    pub values: Vec<f64>,
}

/// A node another hears straight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HeardNeighbor {
    /// The node its key starts with, known or not.
    pub node: u32,
    /// How long ago it last heard it, in seconds.
    pub seconds_ago: u32,
    /// In dB.
    pub snr: f32,
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
    /// Whether the owner is a licensed radio amateur: the radio may then go
    /// past its region's power and band, and sends nothing encrypted, as
    /// their licence has it. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licensed: Option<bool>,
    /// Whether it tells others nobody reads messages sent to it, as for a
    /// node of the mesh's own. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmessagable: Option<bool>,
    /// A word on how it is, up to 79 bytes, which the radio sends now and
    /// then; empty for none. Only from firmware 2.7.20, which shows it; left
    /// out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// The most bytes a status message takes.
pub const STATUS_MAX: usize = 79;

/// What the radio does in the mesh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceSettings {
    /// One of `choices.roles`.
    #[cfg_attr(feature = "openapi", schema(example = "CLIENT"))]
    pub role: String,
}

/// How the radio transmits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoraSettings {
    /// One of `choices.regions`.
    #[cfg_attr(feature = "openapi", schema(example = "EU_868"))]
    pub region: String,
    /// Whether it uses `modem_preset`, or the bandwidth, spreading factor and
    /// coding rate set by hand.
    pub use_preset: bool,
    /// One of `choices.presets_by_region`'s for its region.
    #[cfg_attr(feature = "openapi", schema(example = "LONG_FAST"))]
    pub modem_preset: String,
    /// Without a preset: one of `choices.bandwidths` (or
    /// `choices.wide_bandwidths` on 2.4 GHz), as the radio keeps it: kHz, but
    /// for a few codes that stand for a fraction (8 is 7.8, 10 is 10.4, 16 is
    /// 15.6, 21 is 20.8, 31 is 31.25, 42 is 41.7, 62 is 62.5, 200 is 203.125,
    /// 400 is 406.25, 800 is 812.5, 1600 is 1625). Left as it is with one.
    #[serde(default)]
    pub bandwidth: u32,
    /// Without a preset: 5 to 12.
    #[serde(default)]
    pub spread_factor: u32,
    /// Without a preset: the n of 4/n, 5 to 8.
    #[serde(default)]
    pub coding_rate: u32,
    /// How many times a packet it sends may be relayed: 1 to 7.
    pub hop_limit: u32,
    /// Whether it transmits at all.
    pub tx_enabled: bool,
    /// What few need to touch. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advanced: Option<LoraAdvanced>,
}

/// How the radio transmits, beyond what most need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoraAdvanced {
    /// In dBm, 0 for the most its region allows, which is at most that
    /// (`choices.region_limits`), unless the owner is licensed. The radio
    /// says back what it uses.
    pub tx_power: i32,
    /// Which of its region's frequency slots, from 1; 0 for the one the
    /// primary channel's name picks. As many as its region and bandwidth
    /// make room for.
    pub frequency_slot: u32,
    /// A frequency of its own, in MHz, over the slot: 0 for none. Within its
    /// region's band, unless the owner is licensed.
    pub override_frequency: f32,
    /// Added to the frequency, in MHz, to make up for an off crystal: up to
    /// ±1.
    pub frequency_offset: f32,
    /// Whether it transmits past its region's duty cycle: only where the law
    /// lets it.
    pub override_duty_cycle: bool,
    /// Whether an SX126x chip receives with more gain, for more power.
    pub rx_boosted_gain: bool,
    /// Whether it lets others' gateways put what it sends on MQTT.
    pub ok_to_mqtt: bool,
    /// Whether it leaves aside what reaches it through MQTT.
    pub ignore_mqtt: bool,
    /// Whether its amplifier's fan, on boards with one, is off.
    pub pa_fan_disabled: bool,
}

/// What a region allows, as the firmware has it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RegionLimits {
    /// Its band, in MHz.
    pub frequency_start: f64,
    pub frequency_end: f64,
    /// The share of the time it may transmit, in %: 100 for no limit.
    pub duty_cycle: f64,
    /// The most power it may transmit with, in dBm.
    pub power_limit: u32,
    /// How its frequency slots are laid out, in MHz: between them, and at
    /// each side of one. Each takes these and its bandwidth; it has as many
    /// as fit, rounded.
    pub spacing: f64,
    pub padding: f64,
    /// The slot it uses when none is set, from 1; 0 when the primary
    /// channel's name picks it: the djb2 hash of that name (its preset's, if
    /// unnamed), modulo how many slots there are, plus one.
    pub default_slot: u32,
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
    /// How precisely the radio sends its position on it, in bits of each
    /// coordinate: 0 not at all, 32 exactly, fewer for a place only roughly
    /// where it is; the apps offer 10 (some 23 km off) to 19 (some 46 m). On
    /// a public channel, at most [`PUBLIC_POSITION_PRECISION`].
    #[serde(default)]
    pub position_precision: u32,
    /// Whether anyone can read it: no key, the default one or a simple one,
    /// or, for a secondary without a key of its own, a public primary's.
    #[serde(default)]
    pub public: bool,
    /// Whether what comes on it is to be told of quietly, as for a busy
    /// public channel one keeps all the same.
    #[serde(default)]
    pub muted: bool,
}

/// The most bits of precision the radio sends its position with on a public
/// channel, from firmware 2.8: some 730 m off. It cuts more down to this.
pub const PUBLIC_POSITION_PRECISION: u32 = 15;

/// The codes each setting with a fixed set of values takes, as the
/// protobufs this daemon was built with name them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingsChoices {
    pub roles: Vec<String>,
    pub regions: Vec<String>,
    pub presets: Vec<String>,
    /// The presets each region takes, its default first: no other is used.
    pub presets_by_region: std::collections::BTreeMap<String, Vec<String>>,
    /// What each preset stands for, below 1 GHz: to start from when going
    /// without one.
    pub preset_params: std::collections::BTreeMap<String, LoraParams>,
    /// The bandwidths that can be set by hand, as [`LoraSettings::bandwidth`]
    /// takes them; and on 2.4 GHz (`LORA_24`).
    pub bandwidths: Vec<u32>,
    pub wide_bandwidths: Vec<u32>,
    /// What each region allows.
    pub region_limits: std::collections::BTreeMap<String, RegionLimits>,
}

/// How a radio transmits without a preset, or what a preset stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoraParams {
    pub bandwidth: u32,
    pub spread_factor: u32,
    pub coding_rate: u32,
}

/// The radio's own settings, as it last sent them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// One's own, given in `custom_key`.
    Custom,
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
    /// With `key` at `custom`: the key, in base64 as the apps show it, of 16
    /// or 32 bytes (AES-128 or AES-256), or of 1 for one of the simple ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(example = "1PG7OiApB1nwvP+rz05pAQ=="))]
    pub custom_key: Option<String>,
    pub uplink: bool,
    pub downlink: bool,
    /// As [`ChannelSettings::position_precision`]: 0 to 32, and at most
    /// [`PUBLIC_POSITION_PRECISION`] on a public channel. Left out, it stays
    /// as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_precision: Option<u32>,
    /// As [`ChannelSettings::muted`]. Left out, it stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
}

/// The order of a radio's channels: every slot it has a channel in, each
/// once, in the order they are to be. The first becomes the primary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelOrder {
    #[cfg_attr(feature = "openapi", schema(example = json!([0, 2, 1])))]
    pub order: Vec<u32>,
}

/// What becomes of the messages of a channel taken off its radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChannelHistory {
    /// Kept aside, in no conversation: given back if the very same channel
    /// comes back, name and key.
    #[default]
    Keep,
    /// Gone with it.
    Forget,
}

/// A secret of a section of the radio's settings, as a Wi-Fi password, asked
/// for on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SectionSecret {
    #[cfg_attr(feature = "openapi", schema(example = "config.network"))]
    pub section: String,
    #[cfg_attr(feature = "openapi", schema(example = "wifi_psk"))]
    pub field: String,
    pub value: String,
}

/// A channel's key itself, asked for on its own: the one secret of the radio
/// meshctl hands out, and only when asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelSecret {
    /// Its slot, 0 to 7.
    pub index: u32,
    pub kind: ChannelKey,
    /// In base64, as the apps show it; empty when it has none.
    #[cfg_attr(feature = "openapi", schema(example = "1PG7OiApB1nwvP+rz05pAQ=="))]
    pub key: String,
}

/// What to change of the radio's settings: each part given is set whole,
/// each part left out stays as it is.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
    /// Any of `choices`, each a flag the radio keeps as a bit: as a list of
    /// their codes.
    Flags,
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

/// One radio's hearing of a packet, in the order they are kept.
pub type SightingId = i64;

/// A packet a radio heard, whatever it carries: what the radio sees of the
/// mesh, as it sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Packet {
    /// This hearing of it, by which pages of them are read.
    pub id: SightingId,
    /// Its id on the mesh, which with `from` names it.
    pub packet_id: u32,
    pub from: u32,
    /// 4294967295 for everyone.
    pub to: u32,
    /// The channel's slot on the radio.
    pub channel: u32,
    /// What it carries, as the protobufs name it, or its number when they
    /// do not know it; none while it is encrypted.
    #[cfg_attr(feature = "openapi", schema(example = "TEXT_MESSAGE_APP"))]
    pub port: Option<String>,
    /// What it carries, read: a text as a string, a protobuf as its JSON. None
    /// when it cannot be read, or is kept back, as the radios' administration.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub payload: Option<serde_json::Value>,
    pub request_id: Option<u32>,
    pub reply_id: Option<u32>,
    pub want_response: bool,
    /// Encrypted to the radio's own key, not a channel's.
    pub pki: bool,
    /// When it was received, in seconds since the epoch.
    pub at: i64,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    /// How many hops it took, when it says.
    pub hops_away: Option<u32>,
    pub hop_limit: u32,
    pub hop_start: u32,
    /// The last byte of the node that relayed it last.
    pub relay_node: Option<u32>,
    pub via_mqtt: bool,
}

/// What kind of thing a packet carries, as the live view sorts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PacketKind {
    Text,
    Position,
    NodeInfo,
    Routing,
    Telemetry,
    Traceroute,
    Neighbors,
    /// Still encrypted: no port known.
    Encrypted,
    /// Any other port.
    Other,
}

impl PacketKind {
    pub const ALL: [Self; 9] = [
        Self::Text,
        Self::Position,
        Self::NodeInfo,
        Self::Routing,
        Self::Telemetry,
        Self::Traceroute,
        Self::Neighbors,
        Self::Encrypted,
        Self::Other,
    ];

    /// The ports of it, as the protobufs name them, and the kinds of payload
    /// of MeshCore's, as its firmware names them: what is sealed between two
    /// nodes is encrypted; the others go by what they are not.
    pub fn ports(self) -> &'static [&'static str] {
        match self {
            Self::Text => &["TEXT_MESSAGE_APP", "TEXT_MESSAGE_COMPRESSED_APP", "GRP_TXT"],
            Self::Position => &["POSITION_APP"],
            Self::NodeInfo => &["NODEINFO_APP", "ADVERT"],
            Self::Routing => &["ROUTING_APP", "ACK", "PATH"],
            Self::Telemetry => &["TELEMETRY_APP"],
            Self::Traceroute => &["TRACEROUTE_APP", "TRACE"],
            Self::Neighbors => &["NEIGHBORINFO_APP"],
            Self::Encrypted => &["TXT_MSG", "REQ", "RESPONSE", "ANON_REQ", "GRP_DATA"],
            Self::Other => &[],
        }
    }

    /// The kind of a packet by its port, none while it is encrypted.
    pub fn of(port: Option<&str>) -> Self {
        let Some(port) = port else {
            return Self::Encrypted;
        };
        Self::ALL
            .into_iter()
            .find(|kind| kind.ports().contains(&port))
            .unwrap_or(Self::Other)
    }
}

/// How a packet reached the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PacketReach {
    /// Straight from its sender, no hops.
    Direct,
    /// Through other nodes, hop by hop.
    Mesh,
    Mqtt,
}

/// Which packets the live view shows: all of them with nothing set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketFilter {
    pub kind: Option<PacketKind>,
    /// The channel's slot.
    pub channel: Option<u32>,
    pub reach: Option<PacketReach>,
    /// Only those from or to this node.
    pub node: Option<u32>,
    /// None from this node: the radio's own, to leave them out.
    pub not_from: Option<u32>,
}

impl PacketFilter {
    /// Whether a packet is one of those it lets through.
    pub fn lets(&self, packet: &Packet) -> bool {
        self.kind
            .is_none_or(|kind| PacketKind::of(packet.port.as_deref()) == kind)
            && self.channel.is_none_or(|channel| packet.channel == channel)
            && self.reach.is_none_or(|reach| match reach {
                PacketReach::Direct => packet.hops_away == Some(0),
                PacketReach::Mesh => packet.hops_away.is_some_and(|hops| hops > 0),
                PacketReach::Mqtt => packet.via_mqtt,
            })
            && self
                .node
                .is_none_or(|node| packet.from == node || packet.to == node)
            && self.not_from.is_none_or(|num| packet.from != num)
    }
}

/// How the interface looks and speaks, the same in every browser that opens
/// it: kept by the daemon, not by each browser.
///
/// Whatever is missing when it is written takes its default, so a client that
/// does not know of a newer preference leaves it as it was meant to be.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct Preferences {
    pub theme: Theme,
    pub accent: Accent,
    /// The maps' own theme, which may differ from the rest.
    pub map_theme: MapTheme,
    /// Whether the map's legend is folded away, for more map.
    pub map_legend_folded: bool,
    /// The interface's language; none for the browser's.
    pub language: Option<Language>,
    /// How the nodes list is sorted; none for favourites first, then the
    /// most recently heard.
    pub nodes_sort: Option<NodesSort>,
    /// Over how long a node's telemetry is charted, in seconds; none for a
    /// day.
    pub telemetry_window: Option<u32>,
    /// Which groups of a node's telemetry charts are folded away, a bit each:
    /// device 1, signal 2, environment 4, power 8.
    pub telemetry_folded: u8,
    /// Whether the live view shows what the radio itself sends too: by
    /// default it leaves it out.
    pub packets_with_own: bool,
    /// Which of the conversations' sections are folded away, in the sidebar
    /// and on a phone, a bit each: channels 1, rooms 2, direct 4.
    pub sections_folded: u8,
}

/// A column of the nodes list to sort by, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NodesSort {
    pub column: NodesColumn,
    /// The other way from its most useful one: names from Z, the least
    /// recently heard first, the farthest first, the emptiest battery first.
    #[serde(default)]
    pub reversed: bool,
}

/// A column the nodes list can be sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum NodesColumn {
    Name,
    Heard,
    Reach,
    Power,
    Hardware,
}

/// Light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Whichever the system of each browser prefers.
    #[default]
    System,
    Light,
    Dark,
}

/// The colour that marks what is active and what can be done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Amber,
}

/// The maps' theme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MapTheme {
    /// The same as the rest of the interface.
    #[default]
    Page,
    Light,
    Dark,
}

/// A language the interface speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Language {
    En,
    Es,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_badge_says_what_it_shows() {
        let badge = Badge {
            color: Some("#f0a63a".into()),
            content: BadgeContent::Icon {
                icon: BadgeIcon::Meshtastic,
            },
        };
        assert_eq!(
            serde_json::to_string(&badge).unwrap(),
            r##"{"color":"#f0a63a","content":{"kind":"icon","icon":"meshtastic"}}"##
        );
        // A radio added without one: the rail's colour, and its initials.
        let settings: RadioSettings =
            serde_json::from_str(r#"{"name":"Home","kind":"meshtastic","address":"192.168.1.10"}"#)
                .unwrap();
        assert!(settings.shown);
        assert_eq!(settings.badge, Badge::default());
        assert_eq!(settings.badge.content, BadgeContent::Initials);
    }

    #[test]
    fn preferences_fill_in_what_is_missing() {
        // What an older meshctl kept, and this one no longer has, is left out.
        let preferences: Preferences =
            serde_json::from_str(r#"{"theme":"dark","map_routes":true}"#).unwrap();
        assert_eq!(
            preferences,
            Preferences {
                theme: Theme::Dark,
                ..Default::default()
            }
        );
        assert_eq!(
            serde_json::to_string(&Preferences::default()).unwrap(),
            r#"{"theme":"system","accent":"amber","map_theme":"page","map_legend_folded":false,"language":null,"nodes_sort":null,"telemetry_window":null,"telemetry_folded":0,"packets_with_own":false,"sections_folded":0}"#
        );
    }

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
