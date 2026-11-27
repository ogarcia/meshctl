// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The MeshCore companion protocol, as far as meshctl speaks it: the framing,
//! the commands it sends and what the radio answers or pushes. Pure: bytes in,
//! values out, and the other way round.
//!
//! MeshCore has no schema to generate this from. The layouts are the
//! firmware's own, from its companion radio (`examples/companion_radio/
//! MyMesh.cpp`), checked against real radios. Every value is little endian.
//!
//! A frame is a header byte, `<` from the client and `>` from the radio, the
//! payload's length as a u16, and the payload: a code and what it carries.
//! Codes below 0x80 answer the command just sent; from 0x80 on, the radio
//! pushes them on its own.

use bytes::{Buf, BytesMut};

pub const FROM_CLIENT: u8 = b'<';
pub const FROM_RADIO: u8 = b'>';
const HEADER_SIZE: usize = 3;
/// More than any frame the firmware sends; a header saying otherwise is noise.
const MAX_PAYLOAD: usize = 1024;

/// Frames a command.
pub fn encode(payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let len = u16::try_from(payload.len()).ok()?;
    let mut framed = Vec::with_capacity(HEADER_SIZE + payload.len());
    framed.push(FROM_CLIENT);
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend_from_slice(payload);
    Some(framed)
}

/// Splits what the radio sends into payloads. Bytes before a header are
/// dropped: a radio on serial prints its log between frames.
#[derive(Debug, Default)]
pub struct FrameParser {
    buffer: BytesMut,
}

impl FrameParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            match self.buffer.iter().position(|&b| b == FROM_RADIO) {
                Some(start) => self.buffer.advance(start),
                None => {
                    self.buffer.clear();
                    break;
                }
            }
            if self.buffer.len() < HEADER_SIZE {
                break;
            }
            let len = usize::from(u16::from_le_bytes([self.buffer[1], self.buffer[2]]));
            if len == 0 || len > MAX_PAYLOAD {
                self.buffer.advance(1);
                continue;
            }
            if self.buffer.len() < HEADER_SIZE + len {
                break;
            }
            self.buffer.advance(HEADER_SIZE);
            frames.push(self.buffer.split_to(len).to_vec());
        }
        frames
    }
}

/// The commands meshctl sends.
pub mod command {
    const APP_START: u8 = 0x01;
    const SEND_TEXT: u8 = 0x02;
    const SEND_CHANNEL_TEXT: u8 = 0x03;
    const GET_CONTACTS: u8 = 0x04;
    const GET_DEVICE_TIME: u8 = 0x05;
    const SET_DEVICE_TIME: u8 = 0x06;
    const SEND_SELF_ADVERT: u8 = 0x07;
    const SET_ADVERT_NAME: u8 = 0x08;
    const ADD_UPDATE_CONTACT: u8 = 0x09;
    const SYNC_NEXT_MESSAGE: u8 = 0x0a;
    const SET_RADIO_PARAMS: u8 = 0x0b;
    const SET_RADIO_TX_POWER: u8 = 0x0c;
    const RESET_PATH: u8 = 0x0d;
    const SET_ADVERT_LATLON: u8 = 0x0e;
    const REMOVE_CONTACT: u8 = 0x0f;
    const GET_BATT_AND_STORAGE: u8 = 0x14;
    const DEVICE_QUERY: u8 = 0x16;
    const GET_CONTACT_BY_KEY: u8 = 0x1e;
    const GET_CHANNEL: u8 = 0x1f;
    const SET_CHANNEL: u8 = 0x20;
    const SET_OTHER_PARAMS: u8 = 0x26;
    const GET_STATS: u8 = 0x38;
    const SEND_ANON_REQ: u8 = 0x39;
    const SEND_LOGIN: u8 = 0x1a;
    const SEND_BINARY_REQ: u8 = 0x32;
    const SEND_TRACE_PATH: u8 = 0x24;
    const SEND_PATH_DISCOVERY_REQ: u8 = 0x34;
    const RUN_CLI_COMMAND: u8 = 0x42;
    const SET_FLOOD_SCOPE_KEY: u8 = 0x36;
    const SET_TUNING_PARAMS: u8 = 0x15;
    const GET_TUNING_PARAMS: u8 = 0x2b;
    const SET_AUTOADD_CONFIG: u8 = 0x3a;
    const GET_AUTOADD_CONFIG: u8 = 0x3b;
    const GET_ALLOWED_REPEAT_FREQ: u8 = 0x3c;
    const SET_PATH_HASH_MODE: u8 = 0x3d;
    const SET_DEFAULT_FLOOD_SCOPE: u8 = 0x3f;
    const GET_DEFAULT_FLOOD_SCOPE: u8 = 0x40;
    const GET_CUSTOM_VARS: u8 = 0x28;
    const SET_CUSTOM_VAR: u8 = 0x29;
    /// Plain text, as opposed to a CLI command or a signed message.
    const TEXT_PLAIN: u8 = 0;
    /// A command for a node's console, and its answer.
    const TEXT_COMMAND: u8 = 1;
    /// The protocol version the client speaks, which the radio answers to.
    const PROTOCOL_VERSION: u8 = 8;

    /// Opens a session, naming the app that connects: seven reserved bytes,
    /// then the name, which the firmware prints. Answered with `SelfInfo`.
    pub fn app_start(name: &str) -> Vec<u8> {
        let mut payload = vec![APP_START, 0, 0, 0, 0, 0, 0, 0];
        payload.extend_from_slice(name.as_bytes());
        payload
    }

    /// Answered with `DeviceInfo`.
    pub fn device_query() -> Vec<u8> {
        vec![DEVICE_QUERY, PROTOCOL_VERSION]
    }

    /// Every contact changed since `since` (0 for all), answered with
    /// `ContactStart`, a `Contact` for each, and `ContactEnd`.
    pub fn get_contacts(since: u32) -> Vec<u8> {
        let mut payload = vec![GET_CONTACTS];
        if since > 0 {
            payload.extend_from_slice(&since.to_le_bytes());
        }
        payload
    }

    /// Answered with `ChannelInfo`, or `Error` past the last slot.
    pub fn get_channel(index: u8) -> Vec<u8> {
        vec![GET_CHANNEL, index]
    }

    /// The next message queued, or `NoMoreMessages`.
    pub fn sync_next_message() -> Vec<u8> {
        vec![SYNC_NEXT_MESSAGE]
    }

    /// A direct message to the contact the key prefix names. Answered with
    /// `MessageSent`, whose tag the `Ack` that delivers it carries.
    pub fn send_text(prefix: &[u8; 6], timestamp: u32, text: &str) -> Vec<u8> {
        let mut payload = vec![SEND_TEXT, TEXT_PLAIN, 0];
        payload.extend_from_slice(&timestamp.to_le_bytes());
        payload.extend_from_slice(prefix);
        payload.extend_from_slice(text.as_bytes());
        payload
    }

    /// A command for the radio's own console, as its CLI takes them, from
    /// firmware 14 on: answered with `CliReply`, at once, sending nothing.
    pub fn run_command(text: &str) -> Vec<u8> {
        let mut payload = vec![RUN_CLI_COMMAND];
        payload.extend_from_slice(text.as_bytes());
        payload
    }

    /// A command for the console of a node logged in to as its admin, as a
    /// direct message of the console's kind: the radio dates it by its own
    /// clock, and waits for no acknowledgement. Answered with
    /// `MessageSent`; the node answers, to its admin only, with a message of
    /// the same kind.
    pub fn send_command(prefix: &[u8; 6], text: &str) -> Vec<u8> {
        let mut payload = vec![SEND_TEXT, TEXT_COMMAND, 0];
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(prefix);
        payload.extend_from_slice(text.as_bytes());
        payload
    }

    /// A message to a channel. Answered with `Ok`, and nothing else ever.
    pub fn send_channel_text(channel: u8, timestamp: u32, text: &str) -> Vec<u8> {
        let mut payload = vec![SEND_CHANNEL_TEXT, TEXT_PLAIN, channel];
        payload.extend_from_slice(&timestamp.to_le_bytes());
        payload.extend_from_slice(text.as_bytes());
        payload
    }

    /// The radio's clock, answered with `CurrentTime`.
    pub fn get_time() -> Vec<u8> {
        vec![GET_DEVICE_TIME]
    }

    /// Sets the radio's clock. Only forward: an earlier time is refused.
    pub fn set_time(secs: u32) -> Vec<u8> {
        let mut payload = vec![SET_DEVICE_TIME];
        payload.extend_from_slice(&secs.to_le_bytes());
        payload
    }

    /// The name it advertises; the firmware keeps 31 bytes of it.
    pub fn set_name(name: &str) -> Vec<u8> {
        let mut payload = vec![SET_ADVERT_NAME];
        payload.extend_from_slice(name.as_bytes());
        payload
    }

    /// Where it says it is, in millionths of a degree.
    pub fn set_position(lat: i32, lon: i32) -> Vec<u8> {
        let mut payload = vec![SET_ADVERT_LATLON];
        payload.extend_from_slice(&lat.to_le_bytes());
        payload.extend_from_slice(&lon.to_le_bytes());
        payload
    }

    /// Its transmit power, in dBm, from -9 to what the board allows.
    pub fn set_tx_power(dbm: i8) -> Vec<u8> {
        vec![SET_RADIO_TX_POWER, dbm.cast_unsigned()]
    }

    /// Its frequency in kHz, bandwidth in Hz, spreading factor and coding
    /// rate, applied at once. On firmware that has it (v9+) the last byte
    /// sets client repeat too, and leaving it out leaves it off: `None` only
    /// for firmware without it.
    pub fn set_radio_params(
        frequency: u32,
        bandwidth: u32,
        spreading_factor: u8,
        coding_rate: u8,
        repeat: Option<bool>,
    ) -> Vec<u8> {
        let mut payload = vec![SET_RADIO_PARAMS];
        payload.extend_from_slice(&frequency.to_le_bytes());
        payload.extend_from_slice(&bandwidth.to_le_bytes());
        payload.extend_from_slice(&[spreading_factor, coding_rate]);
        if let Some(repeat) = repeat {
            payload.push(u8::from(repeat));
        }
        payload
    }

    /// Whether contacts are added only by hand; who it shares telemetry
    /// with, two bits for each of base, location and environment; whether
    /// its adverts carry its position; and its extra acknowledgements.
    pub fn set_other_params(
        manual_add: bool,
        telemetry: u8,
        location_policy: u8,
        multi_acks: u8,
    ) -> Vec<u8> {
        vec![
            SET_OTHER_PARAMS,
            u8::from(manual_add),
            telemetry,
            location_policy,
            multi_acks,
        ]
    }

    /// Adds a contact, or writes back the one with its key whole, laid out
    /// as the radio lists them: its path as given, none known (`path_len`
    /// -1) unless one is.
    pub fn add_contact(contact: &super::Contact) -> Vec<u8> {
        let mut payload = vec![ADD_UPDATE_CONTACT];
        payload.extend_from_slice(&contact.public_key);
        payload.extend_from_slice(&[
            contact.kind,
            contact.flags,
            contact.path_len.cast_unsigned(),
        ]);
        let mut path = [0; 64];
        let hops = contact.path.len().min(path.len());
        path[..hops].copy_from_slice(&contact.path[..hops]);
        payload.extend_from_slice(&path);
        payload.extend_from_slice(&fixed(&contact.name, 32));
        payload.extend_from_slice(&contact.last_advert.to_le_bytes());
        payload.extend_from_slice(&contact.lat.to_le_bytes());
        payload.extend_from_slice(&contact.lon.to_le_bytes());
        payload.extend_from_slice(&contact.last_modified.to_le_bytes());
        payload
    }

    /// One contact as the radio has it now, by its whole key: answered with
    /// `Contact`, or `Error` if it has none such.
    pub fn get_contact(key: &[u8; 32]) -> Vec<u8> {
        let mut payload = vec![GET_CONTACT_BY_KEY];
        payload.extend_from_slice(key);
        payload
    }

    /// Forgets a contact, by its whole key.
    pub fn remove_contact(key: &[u8; 32]) -> Vec<u8> {
        let mut payload = vec![REMOVE_CONTACT];
        payload.extend_from_slice(key);
        payload
    }

    /// Forgets the path to a contact: the next message floods to find one.
    pub fn reset_path(key: &[u8; 32]) -> Vec<u8> {
        let mut payload = vec![RESET_PATH];
        payload.extend_from_slice(key);
        payload
    }

    /// The radio says who it is: to those it hears directly, or flooded to
    /// the whole mesh.
    pub fn send_advert(flood: bool) -> Vec<u8> {
        vec![SEND_SELF_ADVERT, u8::from(flood)]
    }

    /// Sets a channel's slot: its name, up to 31 bytes, and its 128-bit
    /// secret. An empty name and a secret of zeros empty the slot.
    pub fn set_channel(index: u8, name: &str, secret: &[u8; 16]) -> Vec<u8> {
        let mut payload = vec![SET_CHANNEL, index];
        let mut field = fixed(name, 32);
        field[31] = 0;
        payload.extend_from_slice(&field);
        payload.extend_from_slice(secret);
        payload
    }

    /// A text in a fixed field: cut or padded with zeros to `len`.
    fn fixed(text: &str, len: usize) -> Vec<u8> {
        let mut field = vec![0u8; len];
        let bytes = text.as_bytes();
        let n = bytes.len().min(len);
        field[..n].copy_from_slice(&bytes[..n]);
        field
    }

    /// Answered with `Battery`.
    pub fn get_battery() -> Vec<u8> {
        vec![GET_BATT_AND_STORAGE]
    }

    /// Answered with `Stats` of that kind: 0 core, 1 radio, 2 packets.
    pub fn get_stats(kind: u8) -> Vec<u8> {
        vec![GET_STATS, kind]
    }

    /// An anonymous request to a node, by its whole key: no login, and it
    /// goes direct, on the path the radio knows, or straight to one not a
    /// contact. Answered with `MessageSent`, whose tag the node's
    /// `BinaryResponse` carries.
    pub fn send_anon_request(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
        let mut payload = vec![SEND_ANON_REQ];
        payload.extend_from_slice(key);
        payload.extend_from_slice(data);
        payload
    }

    /// A request to a node the radio logged in to, by its whole key: what it
    /// asks, and what goes with it; the radio puts its clock before, which
    /// the node answers with as the tag. Answered with `MessageSent`, whose
    /// tag the node's `BinaryResponse` carries. The radio waits for one
    /// such answer at a time: another request forgets the one before.
    pub fn send_binary_request(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
        let mut payload = vec![SEND_BINARY_REQ];
        payload.extend_from_slice(key);
        payload.extend_from_slice(data);
        payload
    }

    /// A trace along a path of nodes, by their hashes, each `size` bytes
    /// (the low two bits of `flags`, 0 for one): each one on it passes it on
    /// with the SNR it heard it at, and the radio hears the last one pass
    /// it. Answered with `MessageSent`; then pushed `TraceData`, with the
    /// tag given.
    pub fn send_trace_path(tag: u32, flags: u8, path: &[u8]) -> Vec<u8> {
        let mut payload = vec![SEND_TRACE_PATH];
        payload.extend_from_slice(&tag.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.push(flags);
        payload.extend_from_slice(path);
        payload
    }

    /// Asks a contact, flooded, the way to it and the way back, by its whole
    /// key: answered with `MessageSent`, then pushed `PathDiscovery`, by the
    /// start of its key. The radio forgets any other request it waits for,
    /// and keeps the path it had.
    pub fn send_path_discovery(key: &[u8; 32]) -> Vec<u8> {
        let mut payload = vec![SEND_PATH_DISCOVERY_REQ, 0];
        payload.extend_from_slice(key);
        payload
    }

    /// Logs in to a room or a repeater, by its whole key, with a password of
    /// up to 15 bytes, cut there; an empty one is a guest's, or one the node
    /// knows already. To a room it says too since when to send its posts,
    /// which the radio keeps. Answered with `MessageSent`; then pushed
    /// `LoggedIn` or `NotLoggedIn`, or nothing if the node did not take it.
    pub fn send_login(key: &[u8; 32], password: &str) -> Vec<u8> {
        let mut payload = vec![SEND_LOGIN];
        payload.extend_from_slice(key);
        let mut end = password.len().min(15);
        while !password.is_char_boundary(end) {
            end -= 1;
        }
        payload.extend_from_slice(&password.as_bytes()[..end]);
        payload
    }

    /// The region what it floods next is scoped to, by its key, instead of
    /// its default; with none, back to its default.
    pub fn set_flood_scope(key: Option<&[u8; 16]>) -> Vec<u8> {
        let mut payload = vec![SET_FLOOD_SCOPE_KEY, 0];
        if let Some(key) = key {
            payload.extend_from_slice(key);
        }
        payload
    }

    /// What it floods next goes scoped to no region, whatever its default
    /// (v12+).
    pub fn set_flood_unscoped() -> Vec<u8> {
        vec![SET_FLOOD_SCOPE_KEY, 1]
    }

    /// Answered with `Tuning`.
    pub fn get_tuning() -> Vec<u8> {
        vec![GET_TUNING_PARAMS]
    }

    /// How long it waits before passing on what it hears, as a base, and
    /// its airtime factor; both in thousandths.
    pub fn set_tuning(rx_delay: u32, airtime_factor: u32) -> Vec<u8> {
        let mut payload = vec![SET_TUNING_PARAMS];
        payload.extend_from_slice(&rx_delay.to_le_bytes());
        payload.extend_from_slice(&airtime_factor.to_le_bytes());
        payload
    }

    /// Answered with `AutoAdd`.
    pub fn get_autoadd() -> Vec<u8> {
        vec![GET_AUTOADD_CONFIG]
    }

    /// Which kinds of contact it adds by itself while contacts are added by
    /// hand, and whether it overwrites the oldest when full, as bits; and
    /// up to how far: 0 any, 1 straight, N up to N - 1 hops (64 at most).
    pub fn set_autoadd(config: u8, max_hops: u8) -> Vec<u8> {
        vec![SET_AUTOADD_CONFIG, config, max_hops]
    }

    /// What the radio's sensors let be set, as `name:value`, such as its
    /// GPS. Answered with `CustomVars`.
    pub fn get_custom_vars() -> Vec<u8> {
        vec![GET_CUSTOM_VARS]
    }

    /// Sets one of what [`get_custom_vars`] tells: `gps` to `1` or `0`,
    /// or `gps_interval` in seconds, which is not told.
    pub fn set_custom_var(name: &str, value: &str) -> Vec<u8> {
        let mut payload = vec![SET_CUSTOM_VAR];
        payload.extend_from_slice(format!("{name}:{value}").as_bytes());
        payload
    }

    /// Answered with `RepeatFrequencies`.
    pub fn get_repeat_frequencies() -> Vec<u8> {
        vec![GET_ALLOWED_REPEAT_FREQ]
    }

    /// How many bytes each hop's hash takes in what it sends, less one:
    /// 0 to 2.
    pub fn set_path_hash_mode(mode: u8) -> Vec<u8> {
        vec![SET_PATH_HASH_MODE, 0, mode]
    }

    /// Answered with `DefaultScope`.
    pub fn get_default_scope() -> Vec<u8> {
        vec![GET_DEFAULT_FLOOD_SCOPE]
    }

    /// The region what it floods is scoped to: its name, up to 30 bytes,
    /// and its key; or none, flooded with no scope.
    pub fn set_default_scope(scope: Option<(&str, &[u8; 16])>) -> Vec<u8> {
        let mut payload = vec![SET_DEFAULT_FLOOD_SCOPE];
        if let Some((name, key)) = scope {
            let mut field = fixed(name, 31);
            field[30] = 0;
            payload.extend_from_slice(&field);
            payload.extend_from_slice(key);
        }
        payload
    }
}

/// A contact the radio keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub public_key: [u8; 32],
    /// 1 chat, 2 repeater, 3 room, 4 sensor.
    pub kind: u8,
    pub flags: u8,
    /// Hops on the path known to it, and each hash's size in its top two
    /// bits; negative when none is.
    pub path_len: i8,
    /// The path's hashes, each the start of a repeater's key, from the radio
    /// out: as many bytes as the path takes.
    pub path: Vec<u8>,
    pub name: String,
    /// When it last advertised, by its own clock.
    pub last_advert: u32,
    /// In millionths of a degree.
    pub lat: i32,
    pub lon: i32,
    /// When the radio last changed the contact, by the radio's clock.
    pub last_modified: u32,
}

/// The radio itself, as it answers `APP_START`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfInfo {
    pub kind: u8,
    /// In dBm, which may be below zero.
    pub tx_power: i8,
    pub max_tx_power: u8,
    pub public_key: [u8; 32],
    pub lat: i32,
    pub lon: i32,
    /// Extra acknowledgements it sends.
    pub multi_acks: u8,
    /// 0 its adverts say nothing of where it is, 1 they say.
    pub location_policy: u8,
    /// Who it shares telemetry with, two bits each: base, location (<< 2)
    /// and environment (<< 4); 0 nobody, 1 those flagged, 2 anyone.
    pub telemetry: u8,
    /// Whether contacts are added only by hand.
    pub manual_add: bool,
    /// In kHz.
    pub frequency: u32,
    /// In Hz.
    pub bandwidth: u32,
    pub spreading_factor: u8,
    pub coding_rate: u8,
    pub name: String,
}

/// What the radio says of its hardware and firmware.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub firmware_code: u8,
    pub max_contacts: Option<u16>,
    pub max_channels: Option<u8>,
    pub build: Option<String>,
    pub model: Option<String>,
    pub version: Option<String>,
    /// Whether it repeats as well (v9+).
    pub repeat: Option<bool>,
    /// How many bytes each hop's hash takes, less one (v10+).
    pub path_hash_mode: Option<u8>,
}

/// The radio's figures, by kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Stats {
    Core {
        battery_mv: u16,
        uptime_secs: u32,
        errors: u16,
        queue: u8,
    },
    Radio {
        /// In dBm.
        noise_floor: i16,
        last_rssi: i8,
        last_snr: f32,
        /// Seconds spent sending and receiving, since it started.
        tx_air_secs: u32,
        rx_air_secs: u32,
    },
    Packets {
        received: u32,
        sent: u32,
        sent_flood: u32,
        sent_direct: u32,
        received_flood: u32,
        received_direct: u32,
        /// Only on firmware that counts them.
        receive_errors: Option<u32>,
    },
}

/// A direct message, as received.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactMessage {
    /// The first six bytes of the sender's public key.
    pub prefix: [u8; 6],
    /// Hops it came; 0xff when it came on a known path.
    pub path_len: u8,
    pub text_type: u8,
    /// By the sender's clock.
    pub timestamp: u32,
    pub text: String,
    /// Only in the newer form of the message.
    pub snr: Option<f32>,
    /// Who wrote it, by the first four bytes of their key, when it is signed:
    /// a room's post, which the room sends as from itself.
    pub author: Option<[u8; 4]>,
}

/// What a room or a repeater let in as, by the lowest bits of its access
/// list's permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Reads, and cannot write: a guest, or one let in only to read.
    ReadOnly,
    ReadWrite,
    Admin,
}

/// A login answered: to whom, by the first six bytes of its key, and as what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedIn {
    pub prefix: [u8; 6],
    pub access: Access,
    /// The node's clock as it answered, from firmware that says it.
    pub clock: Option<u32>,
}

/// A channel message, as received: with no sender but what its text says.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMessage {
    pub channel: u8,
    pub path_len: u8,
    pub text_type: u8,
    pub timestamp: u32,
    pub text: String,
    pub snr: Option<f32>,
}

/// Everything meshctl understands of what the radio sends.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Ok,
    Error(u8),
    ContactStart,
    Contact(Contact),
    /// The latest change among the contacts sent.
    ContactEnd(u32),
    SelfInfo(SelfInfo),
    /// A direct message taken: the tag its acknowledgement will carry, and how
    /// long to wait for one, in milliseconds.
    MessageSent {
        tag: [u8; 4],
        timeout_ms: u32,
    },
    ContactMessage(ContactMessage),
    ChannelMessage(ChannelMessage),
    NoMoreMessages,
    DeviceInfo(DeviceInfo),
    /// A channel's slot, name and secret; both empty for an empty slot.
    ChannelInfo {
        index: u8,
        name: String,
        secret: [u8; 16],
    },
    /// The radio's clock.
    CurrentTime(u32),
    Battery {
        millivolts: u16,
        /// Its storage, in KB, on firmware that says.
        used_kb: Option<u32>,
        total_kb: Option<u32>,
    },
    Stats(Stats),
    /// Its receive delay base and airtime factor, in thousandths.
    Tuning {
        rx_delay: u32,
        airtime_factor: u32,
    },
    /// Which kinds of contact it adds by itself, as bits, and up to how far.
    AutoAdd {
        config: u8,
        max_hops: u8,
    },
    /// The frequency ranges it repeats on, in kHz, both ends included.
    RepeatFrequencies(Vec<(u32, u32)>),
    /// What its sensors let be set, by name, as it says it: `gps` with a
    /// GPS it detected.
    CustomVars(Vec<(String, String)>),
    /// What the radio's own console answered a command.
    CliReply(String),
    /// The region what it floods is scoped to, name and key; none if none.
    DefaultScope(Option<(String, [u8; 16])>),
    /// Pushed: a packet the radio heard, raw, every copy of it: the SNR and
    /// RSSI it was heard at.
    RxLog {
        snr: f32,
        rssi: i8,
        raw: Vec<u8>,
    },
    /// Pushed: what a node answered a request with, by the tag the request
    /// went by.
    BinaryResponse {
        tag: [u8; 4],
        data: Vec<u8>,
    },
    /// Pushed: a contact advertised.
    Advert,
    /// Pushed: the path to a contact changed.
    PathUpdate,
    /// Pushed: a direct message of ours was delivered.
    Ack([u8; 4]),
    /// Pushed: messages are queued, to be asked for.
    MessagesWaiting,
    /// Pushed: a contact the radio had not heard of before.
    NewContact(Contact),
    /// Pushed: a contact gone from the radio, by its key, as when the radio
    /// is full and a new one takes the place of the one changed longest ago.
    ContactDeleted([u8; 32]),
    /// Pushed: a new contact heard, and no room on the radio for it.
    ContactsFull,
    /// Pushed: a trace sent came back, by its tag: the hashes of the path
    /// it went, the SNR each hop heard it at, and the SNR the radio heard
    /// it back at.
    TraceData {
        tag: [u8; 4],
        hashes: Vec<u8>,
        snrs: Vec<f32>,
        last_snr: f32,
    },
    /// Pushed: the way to a contact and back, as a path discovery found
    /// them, by the first six bytes of its key: each a path's length byte,
    /// as contacts have, and its hashes.
    PathDiscovery {
        prefix: [u8; 6],
        there: (u8, Vec<u8>),
        back: (u8, Vec<u8>),
    },
    /// Pushed: a login was taken.
    LoggedIn(LoggedIn),
    /// Pushed: a login was refused, by the first six bytes of the node's key.
    NotLoggedIn([u8; 6]),
    /// Anything else, by its code.
    Other(u8),
}

impl Response {
    /// Whether the radio sent it on its own rather than to answer a command.
    pub fn is_push(&self) -> bool {
        matches!(
            self,
            Self::Advert
                | Self::PathUpdate
                | Self::Ack(_)
                | Self::MessagesWaiting
                | Self::NewContact(_)
                | Self::ContactDeleted(_)
                | Self::ContactsFull
                | Self::BinaryResponse { .. }
                | Self::RxLog { .. }
                | Self::LoggedIn(_)
                | Self::TraceData { .. }
                | Self::PathDiscovery { .. }
                | Self::NotLoggedIn(_)
        ) || matches!(self, Self::Other(code) if *code >= 0x80)
    }

    /// Reads a payload; `None` if it is too short for what its code says.
    pub fn parse(payload: &[u8]) -> Option<Self> {
        let (&code, data) = payload.split_first()?;
        Some(match code {
            0x00 => Self::Ok,
            0x01 => Self::Error(data.first().copied().unwrap_or(0)),
            0x02 => Self::ContactStart,
            0x03 => Self::Contact(contact(data)?),
            0x04 => Self::ContactEnd(u32_at(data, 0).unwrap_or(0)),
            0x05 => Self::SelfInfo(self_info(data)?),
            0x06 => Self::MessageSent {
                tag: bytes_at(data, 1)?,
                timeout_ms: u32_at(data, 5)?,
            },
            0x07 => Self::ContactMessage(contact_message(data, false)?),
            0x08 => Self::ChannelMessage(channel_message(data, false)?),
            0x09 => Self::CurrentTime(u32_at(data, 0)?),
            0x0a => Self::NoMoreMessages,
            0x0c => Self::Battery {
                millivolts: u16::from_le_bytes(bytes_at(data, 0)?),
                used_kb: u32_at(data, 2),
                total_kb: u32_at(data, 6),
            },
            0x0d => Self::DeviceInfo(device_info(data)),
            0x10 => Self::ContactMessage(contact_message(data, true)?),
            0x11 => Self::ChannelMessage(channel_message(data, true)?),
            0x12 => Self::ChannelInfo {
                index: *data.first()?,
                name: text_at(data, 1, 32),
                secret: bytes_at(data, 33).unwrap_or_default(),
            },
            0x17 => Self::Tuning {
                rx_delay: u32_at(data, 0)?,
                airtime_factor: u32_at(data, 4)?,
            },
            0x15 => Self::CustomVars(
                String::from_utf8_lossy(data)
                    .trim_end_matches('\0')
                    .split(',')
                    .filter_map(|pair| {
                        let (name, value) = pair.split_once(':')?;
                        Some((name.to_owned(), value.to_owned()))
                    })
                    .collect(),
            ),
            0x18 => Self::Stats(stats(data)?),
            0x19 => Self::AutoAdd {
                config: *data.first()?,
                max_hops: data.get(1).copied().unwrap_or(0),
            },
            0x1a => Self::RepeatFrequencies(
                data.as_chunks::<8>()
                    .0
                    .iter()
                    .filter_map(|range| Some((u32_at(range, 0)?, u32_at(range, 4)?)))
                    .collect(),
            ),
            0x1d => Self::CliReply(
                String::from_utf8_lossy(data)
                    .trim_end_matches('\0')
                    .to_owned(),
            ),
            0x1c => Self::DefaultScope(
                (data.len() >= 31 + 16)
                    .then(|| Some((text_at(data, 0, 31), bytes_at(data, 31)?)))
                    .flatten()
                    .filter(|(name, _)| !name.is_empty()),
            ),
            0x80 => Self::Advert,
            0x81 => Self::PathUpdate,
            0x82 => Self::Ack(bytes_at(data, 0)?),
            0x83 => Self::MessagesWaiting,
            0x8a => Self::NewContact(contact(data)?),
            0x8f => Self::ContactDeleted(bytes_at(data, 0)?),
            0x90 => Self::ContactsFull,
            0x85 => Self::LoggedIn(logged_in(data)?),
            0x89 => trace_data(data)?,
            0x8d => path_discovery(data)?,
            0x86 => Self::NotLoggedIn(bytes_at(data, 1)?),
            0x88 => Self::RxLog {
                snr: snr(*data.first()?),
                rssi: data.get(1)?.cast_signed(),
                raw: data.get(2..).unwrap_or_default().to_vec(),
            },
            0x8c => Self::BinaryResponse {
                tag: bytes_at(data, 1)?,
                data: data.get(5..).unwrap_or_default().to_vec(),
            },
            other => Self::Other(other),
        })
    }
}

fn bytes_at<const N: usize>(data: &[u8], at: usize) -> Option<[u8; N]> {
    data.get(at..at + N)?.try_into().ok()
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    bytes_at(data, at).map(u32::from_le_bytes)
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    bytes_at(data, at).map(i32::from_le_bytes)
}

/// A string in a fixed field: up to its first zero, without what is not text.
fn text_at(data: &[u8], at: usize, len: usize) -> String {
    let field = data.get(at..(at + len).min(data.len())).unwrap_or_default();
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end])
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// How many bytes a path takes, by its length byte: hops times each hash's
/// size; none when there is no path.
pub fn path_bytes(path_len: u8) -> usize {
    if path_len == 0xff {
        return 0;
    }
    usize::from(path_len & 0x3f) * (usize::from(path_len >> 6) + 1)
}

/// An SNR as the radio packs it, in quarters of a dB.
fn snr(raw: u8) -> f32 {
    f32::from(raw.cast_signed()) / 4.0
}

/// The text of a message, past its signature if it is a signed one.
fn message_text(data: &[u8], at: usize, text_type: u8) -> String {
    let at = if text_type == 2 { at + 4 } else { at };
    String::from_utf8_lossy(data.get(at..).unwrap_or_default()).into_owned()
}

fn contact(data: &[u8]) -> Option<Contact> {
    if data.len() < 143 {
        return None;
    }
    Some(Contact {
        public_key: bytes_at(data, 0)?,
        kind: data[32],
        flags: data[33],
        path_len: data[34].cast_signed(),
        path: data
            .get(35..35 + path_bytes(data[34]))
            .unwrap_or_default()
            .to_vec(),
        name: text_at(data, 99, 32),
        last_advert: u32_at(data, 131)?,
        lat: i32_at(data, 135)?,
        lon: i32_at(data, 139)?,
        last_modified: u32_at(data, 143).unwrap_or(0),
    })
}

fn self_info(data: &[u8]) -> Option<SelfInfo> {
    if data.len() < 55 {
        return None;
    }
    Some(SelfInfo {
        kind: data[0],
        tx_power: data[1].cast_signed(),
        max_tx_power: data[2],
        public_key: bytes_at(data, 3)?,
        lat: i32_at(data, 35)?,
        lon: i32_at(data, 39)?,
        multi_acks: data[43],
        location_policy: data[44],
        telemetry: data[45],
        manual_add: data[46] != 0,
        frequency: u32_at(data, 47)?,
        bandwidth: u32_at(data, 51)?,
        spreading_factor: data.get(55).copied().unwrap_or(0),
        coding_rate: data.get(56).copied().unwrap_or(0),
        name: text_at(data, 57, data.len().saturating_sub(57)),
    })
}

fn device_info(data: &[u8]) -> DeviceInfo {
    let firmware_code = data.first().copied().unwrap_or(0);
    // Older firmware says its version code and nothing more.
    if firmware_code < 3 {
        return DeviceInfo {
            firmware_code,
            ..DeviceInfo::default()
        };
    }
    let text = |at, len| (data.len() >= at + len).then(|| text_at(data, at, len));
    DeviceInfo {
        firmware_code,
        // Halved to fit a byte.
        max_contacts: data.get(1).map(|&n| u16::from(n) * 2),
        max_channels: data.get(2).copied(),
        build: text(7, 12),
        model: text(19, 40),
        version: text(59, 20),
        repeat: (firmware_code >= 9)
            .then(|| data.get(79).map(|&b| b != 0))
            .flatten(),
        path_hash_mode: (firmware_code >= 10)
            .then(|| data.get(80).copied())
            .flatten(),
    }
}

fn stats(data: &[u8]) -> Option<Stats> {
    let (&kind, data) = data.split_first()?;
    let u16_at = |at| bytes_at(data, at).map(u16::from_le_bytes);
    Some(match kind {
        0 => Stats::Core {
            battery_mv: u16_at(0)?,
            uptime_secs: u32_at(data, 2)?,
            errors: u16_at(6)?,
            queue: *data.get(8)?,
        },
        1 => Stats::Radio {
            noise_floor: bytes_at(data, 0).map(i16::from_le_bytes)?,
            last_rssi: data.get(2)?.cast_signed(),
            last_snr: snr(*data.get(3)?),
            tx_air_secs: u32_at(data, 4)?,
            rx_air_secs: u32_at(data, 8)?,
        },
        2 => Stats::Packets {
            received: u32_at(data, 0)?,
            sent: u32_at(data, 4)?,
            sent_flood: u32_at(data, 8)?,
            sent_direct: u32_at(data, 12)?,
            received_flood: u32_at(data, 16)?,
            received_direct: u32_at(data, 20)?,
            receive_errors: u32_at(data, 24),
        },
        _ => return None,
    })
}

fn contact_message(data: &[u8], newer: bool) -> Option<ContactMessage> {
    // The newer form starts with the SNR and two reserved bytes.
    let at = if newer { 3 } else { 0 };
    if data.len() < at + 12 {
        return None;
    }
    let text_type = data[at + 7];
    Some(ContactMessage {
        prefix: bytes_at(data, at)?,
        path_len: data[at + 6],
        text_type,
        timestamp: u32_at(data, at + 8)?,
        text: message_text(data, at + 12, text_type),
        snr: newer.then(|| snr(data[0])),
        author: if text_type == 2 {
            Some(bytes_at(data, at + 12)?)
        } else {
            None
        },
    })
}

/// A trace come back: a reserved byte, the length of its path in bytes, its
/// flags (the low two bits the size of each hash, 0 for one byte), its tag
/// and auth code, the path's hashes, the SNR each hop heard it at, and the
/// SNR the radio heard it at, in quarters of a dB.
fn trace_data(data: &[u8]) -> Option<Response> {
    let path_len = usize::from(*data.get(1)?);
    let size = 1usize << (data.get(2)? & 0x03);
    let tag = bytes_at(data, 3)?;
    let hashes = data.get(11..11 + path_len)?.to_vec();
    let hops = path_len / size;
    let snrs = data
        .get(11 + path_len..11 + path_len + hops)?
        .iter()
        .map(|&b| snr(b))
        .collect();
    let last_snr = snr(*data.get(11 + path_len + hops)?);
    Some(Response::TraceData {
        tag,
        hashes,
        snrs,
        last_snr,
    })
}

/// A path discovery's answer: a reserved byte, the start of the node's key,
/// and the way there and back, each its length byte and its hashes.
fn path_discovery(data: &[u8]) -> Option<Response> {
    let prefix = bytes_at(data, 1)?;
    let path_at = |at: usize| {
        let len = *data.get(at)?;
        let bytes = usize::from(len & 0x3f) * (usize::from(len >> 6) + 1);
        Some((
            (len, data.get(at + 1..at + 1 + bytes)?.to_vec()),
            at + 1 + bytes,
        ))
    };
    let (there, at) = path_at(7)?;
    let (back, _) = path_at(at)?;
    Some(Response::PathDiscovery {
        prefix,
        there,
        back,
    })
}

/// A login taken: what the node let in as, in the access list's bits when
/// its firmware says them, else in the older byte (1 an admin, 2 a guest).
fn logged_in(data: &[u8]) -> Option<LoggedIn> {
    let legacy = *data.first()?;
    let access = match data.get(11).map(|acl| acl & 3) {
        Some(3) => Access::Admin,
        Some(2) => Access::ReadWrite,
        Some(_) => Access::ReadOnly,
        None => match legacy {
            1 => Access::Admin,
            2 => Access::ReadOnly,
            _ => Access::ReadWrite,
        },
    };
    Some(LoggedIn {
        prefix: bytes_at(data, 1)?,
        access,
        clock: u32_at(data, 7),
    })
}

fn channel_message(data: &[u8], newer: bool) -> Option<ChannelMessage> {
    let at = if newer { 3 } else { 0 };
    if data.len() < at + 7 {
        return None;
    }
    let text_type = data[at + 2];
    Some(ChannelMessage {
        channel: data[at],
        path_len: data[at + 1],
        text_type,
        timestamp: u32_at(data, at + 3)?,
        text: message_text(data, at + 7, text_type),
        snr: newer.then(|| snr(data[0])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_radio(payload: &[u8]) -> Vec<u8> {
        let mut framed = vec![FROM_RADIO];
        framed.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_le_bytes());
        framed.extend_from_slice(payload);
        framed
    }

    #[test]
    fn frames_are_split_wherever_they_are_cut() {
        let mut stream = b"log line\r\n".to_vec();
        stream.extend(from_radio(&[0x00]));
        stream.extend(from_radio(&[0x83]));
        let mut parser = FrameParser::new();
        // Cut in the middle of the first frame: nothing yet, then both.
        let (first, rest) = stream.split_at(12);
        assert!(parser.feed(first).is_empty());
        assert_eq!(parser.feed(rest), vec![vec![0x00], vec![0x83]]);
    }

    #[test]
    fn commands_are_framed_from_the_client() {
        let framed = encode(&command::sync_next_message()).unwrap();
        assert_eq!(framed, [b'<', 1, 0, 0x0a]);
        assert!(encode(&[0; MAX_PAYLOAD + 1]).is_none());
    }

    #[test]
    fn app_start_names_the_app() {
        let start = command::app_start("meshctl");
        assert_eq!(&start[..8], &[1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&start[8..], b"meshctl");
    }

    #[test]
    fn a_direct_message_is_laid_out_as_the_firmware_reads_it() {
        let sent = command::send_text(&[1, 2, 3, 4, 5, 6], 0x0102_0304, "hola");
        assert_eq!(
            sent,
            [
                2, 0, 0, 4, 3, 2, 1, 1, 2, 3, 4, 5, 6, b'h', b'o', b'l', b'a'
            ]
        );
        let channel = command::send_channel_text(4, 1, "hi");
        assert_eq!(channel, [3, 0, 4, 1, 0, 0, 0, b'h', b'i']);
        assert_eq!(command::get_contacts(0), [4]);
        assert_eq!(command::get_contacts(7), [4, 7, 0, 0, 0]);
    }

    fn contact_payload(code: u8, name: &str) -> Vec<u8> {
        let mut payload = vec![code];
        payload.extend([0x50; 32]);
        payload.extend([2, 1, 0xff]);
        payload.extend([0; 64]);
        let mut field = [0u8; 32];
        field[..name.len()].copy_from_slice(name.as_bytes());
        payload.extend(field);
        payload.extend(1_795_000_000u32.to_le_bytes());
        payload.extend(43_331_140i32.to_le_bytes());
        payload.extend((-8_463_732i32).to_le_bytes());
        payload.extend(1_795_000_100u32.to_le_bytes());
        payload
    }

    #[test]
    fn a_contact_reads_back() {
        let Some(Response::Contact(contact)) = Response::parse(&contact_payload(0x03, "Faro"))
        else {
            panic!("a contact");
        };
        assert_eq!(contact.public_key, [0x50; 32]);
        assert_eq!(contact.kind, 2);
        assert_eq!(contact.flags, 1);
        assert_eq!(contact.path_len, -1);
        assert_eq!(contact.name, "Faro");
        assert_eq!(contact.lat, 43_331_140);
        assert_eq!(contact.lon, -8_463_732);
        assert_eq!(contact.last_modified, 1_795_000_100);
        // The same layout, pushed when a new one is heard.
        assert!(matches!(
            Response::parse(&contact_payload(0x8a, "Mar")),
            Some(Response::NewContact(c)) if c.name == "Mar"
        ));
    }

    #[test]
    fn a_full_radio_says_so() {
        // One gone to make room, by its whole key; and no room at all.
        let mut deleted = vec![0x8f];
        deleted.extend([0x41; 32]);
        assert_eq!(
            Response::parse(&deleted),
            Some(Response::ContactDeleted([0x41; 32]))
        );
        assert_eq!(Response::parse(&[0x90]), Some(Response::ContactsFull));
        // Both pushed, whatever is being asked meanwhile.
        assert!(Response::parse(&deleted).is_some_and(|r| r.is_push()));
        assert!(Response::ContactsFull.is_push());
        // A key cut short is no key.
        assert_eq!(Response::parse(&deleted[..20]), None);
    }

    #[test]
    fn self_info_reads_back() {
        let mut payload = vec![0x05, 1, 22, 22];
        payload.extend([0x50; 32]);
        payload.extend(43_331_140i32.to_le_bytes());
        payload.extend((-8_463_732i32).to_le_bytes());
        payload.extend([0, 0, 0, 0]);
        payload.extend(869_618u32.to_le_bytes());
        payload.extend(62_500u32.to_le_bytes());
        payload.extend([7, 6]);
        payload.extend(b"Solo probando");
        let Some(Response::SelfInfo(me)) = Response::parse(&payload) else {
            panic!("self info");
        };
        assert_eq!(me.kind, 1);
        assert_eq!(me.frequency, 869_618);
        assert_eq!(me.bandwidth, 62_500);
        assert_eq!((me.spreading_factor, me.coding_rate), (7, 6));
        assert_eq!(me.name, "Solo probando");
    }

    #[test]
    fn messages_in_both_forms_read_back() {
        let mut old = vec![0x07, 1, 2, 3, 4, 5, 6, 3, 0];
        old.extend(42u32.to_le_bytes());
        old.extend(b"hola");
        let Some(Response::ContactMessage(message)) = Response::parse(&old) else {
            panic!("a direct message");
        };
        assert_eq!(message.prefix, [1, 2, 3, 4, 5, 6]);
        assert_eq!((message.path_len, message.timestamp), (3, 42));
        assert_eq!(message.text, "hola");
        assert_eq!(message.snr, None);

        let mut newer = vec![0x11, 0xf6, 0, 0, 2, 10, 0];
        newer.extend(42u32.to_le_bytes());
        newer.extend(b"PVZ CL4: Boa tarde.");
        let Some(Response::ChannelMessage(message)) = Response::parse(&newer) else {
            panic!("a channel message");
        };
        assert_eq!(message.channel, 2);
        assert_eq!(message.path_len, 10);
        assert_eq!(message.snr, Some(-2.5));
        assert_eq!(message.text, "PVZ CL4: Boa tarde.");
    }

    #[test]
    fn a_signed_message_leaves_its_signature_out() {
        let mut signed = vec![0x07, 1, 2, 3, 4, 5, 6, 0, 2];
        signed.extend(42u32.to_le_bytes());
        signed.extend([9, 9, 9, 9]);
        signed.extend(b"firmado");
        let Some(Response::ContactMessage(message)) = Response::parse(&signed) else {
            panic!("a direct message");
        };
        assert_eq!(message.text, "firmado");
        // A room's post: who wrote it, by the start of their key.
        assert_eq!(message.author, Some([9, 9, 9, 9]));
    }

    #[test]
    fn a_login_says_what_it_let_in_as() {
        // As newer firmware says it: the access list's bits, and its clock.
        let mut taken = vec![0x85, 0, 1, 2, 3, 4, 5, 6];
        taken.extend(1000u32.to_le_bytes());
        taken.extend([2, 1]);
        assert_eq!(
            Response::parse(&taken),
            Some(Response::LoggedIn(LoggedIn {
                prefix: [1, 2, 3, 4, 5, 6],
                access: Access::ReadWrite,
                clock: Some(1000),
            }))
        );
        // A guest in the access list, though the older byte says nothing.
        taken[12] = 0;
        assert!(matches!(
            Response::parse(&taken),
            Some(Response::LoggedIn(LoggedIn {
                access: Access::ReadOnly,
                ..
            }))
        ));
        // As older firmware says it: only that byte, 1 for an admin.
        let old = [0x85, 1, 1, 2, 3, 4, 5, 6];
        assert!(matches!(
            Response::parse(&old),
            Some(Response::LoggedIn(LoggedIn {
                access: Access::Admin,
                clock: None,
                ..
            }))
        ));
        assert_eq!(
            Response::parse(&[0x86, 0, 1, 2, 3, 4, 5, 6]),
            Some(Response::NotLoggedIn([1, 2, 3, 4, 5, 6]))
        );
        assert!(Response::parse(&old).is_some_and(|r| r.is_push()));
        // A password past 15 bytes cut there, where a character starts.
        let login = command::send_login(&[7; 32], "contraseñalarguísima");
        assert_eq!(&login[33..], "contraseñalarg".as_bytes());
    }

    #[test]
    fn answers_and_pushes_are_told_apart() {
        let mut sent = vec![0x06, 0];
        sent.extend([1, 2, 3, 4]);
        sent.extend(5_000u32.to_le_bytes());
        assert_eq!(
            Response::parse(&sent),
            Some(Response::MessageSent {
                tag: [1, 2, 3, 4],
                timeout_ms: 5_000
            })
        );
        let ack = Response::parse(&[0x82, 1, 2, 3, 4]).unwrap();
        assert_eq!(ack, Response::Ack([1, 2, 3, 4]));
        assert!(ack.is_push());
        assert!(Response::parse(&[0x83]).unwrap().is_push());
        assert_eq!(Response::parse(&[0x88, 0]), None);
        // A path found: two hops there of two bytes each, one back of one.
        let found = Response::parse(&[
            0x8d, 0, 1, 2, 3, 4, 5, 6, 0x42, 0x52, 0x59, 0x55, 0x5c, 0x01, 0x52,
        ])
        .unwrap();
        assert_eq!(
            found,
            Response::PathDiscovery {
                prefix: [1, 2, 3, 4, 5, 6],
                there: (0x42, vec![0x52, 0x59, 0x55, 0x5c]),
                back: (0x01, vec![0x52]),
            }
        );
        assert!(found.is_push());
        assert_eq!(
            Response::parse(&[0x8d, 0, 1, 2, 3, 4, 5, 6, 0x42, 0x52]),
            None
        );
        assert_eq!(
            Response::parse(&[0x88, 20, 0xb0, 0x15, 0x01, 0x07]),
            Some(Response::RxLog {
                snr: 5.0,
                rssi: -80,
                raw: vec![0x15, 0x01, 0x07]
            })
        );
        assert!(Response::parse(&[0x88, 0, 0]).unwrap().is_push());
        assert!(!Response::parse(&[0x0a]).unwrap().is_push());
    }

    #[test]
    fn a_channel_and_the_device_read_back() {
        let mut channel = vec![0x12, 1];
        let mut name = [0u8; 32];
        name[..7].copy_from_slice(b"galicia");
        channel.extend(name);
        channel.extend([7; 16]);
        assert_eq!(
            Response::parse(&channel),
            Some(Response::ChannelInfo {
                index: 1,
                name: "galicia".into(),
                secret: [7; 16]
            })
        );

        let mut device = vec![0x0d, 9, 175, 40, 0, 0, 0, 0];
        let field = |text: &str, len: usize| {
            let mut field = vec![0u8; len];
            field[..text.len()].copy_from_slice(text.as_bytes());
            field
        };
        device.extend(field("15 Oct 2026", 12));
        device.extend(field("Heltec V3", 40));
        device.extend(field("v1.18.0", 20));
        let Some(Response::DeviceInfo(info)) = Response::parse(&device) else {
            panic!("device info");
        };
        assert_eq!(info.max_contacts, Some(350));
        assert_eq!(info.max_channels, Some(40));
        assert_eq!(info.model.as_deref(), Some("Heltec V3"));
        assert_eq!(info.version.as_deref(), Some("v1.18.0"));
        assert_eq!(info.build.as_deref(), Some("15 Oct 2026"));
    }

    #[test]
    fn the_radios_own_settings_read_back() {
        let mut payload = vec![0x05, 1, 0xf7, 22];
        payload.extend([0x50; 32]);
        payload.extend(43_331_140i32.to_le_bytes());
        payload.extend((-8_463_732i32).to_le_bytes());
        payload.extend([1, 1, 0b10_01_10, 1]);
        payload.extend(869_618u32.to_le_bytes());
        payload.extend(62_500u32.to_le_bytes());
        payload.extend([8, 8]);
        payload.extend(b"Casa");
        let Some(Response::SelfInfo(me)) = Response::parse(&payload) else {
            panic!("self info");
        };
        assert_eq!(me.tx_power, -9);
        assert_eq!((me.multi_acks, me.location_policy), (1, 1));
        assert_eq!(me.telemetry, 0b10_01_10);
        assert!(me.manual_add);

        // Repeat and the path's hash, on firmware new enough to say.
        let mut device = vec![0x0d, 14, 175, 40, 0, 0, 0, 0];
        device.extend([0u8; 72]);
        device.extend([1, 2]);
        let Some(Response::DeviceInfo(info)) = Response::parse(&device) else {
            panic!("device info");
        };
        assert_eq!((info.repeat, info.path_hash_mode), (Some(true), Some(2)));
        device[1] = 8;
        let Some(Response::DeviceInfo(old)) = Response::parse(&device) else {
            panic!("device info");
        };
        assert_eq!((old.repeat, old.path_hash_mode), (None, None));
    }

    #[test]
    fn time_battery_and_stats_read_back() {
        let mut time = vec![0x09];
        time.extend(1_795_000_000u32.to_le_bytes());
        assert_eq!(
            Response::parse(&time),
            Some(Response::CurrentTime(1_795_000_000))
        );
        let mut battery = vec![0x0c];
        battery.extend(4_012u16.to_le_bytes());
        battery.extend(120u32.to_le_bytes());
        battery.extend(1_024u32.to_le_bytes());
        assert_eq!(
            Response::parse(&battery),
            Some(Response::Battery {
                millivolts: 4_012,
                used_kb: Some(120),
                total_kb: Some(1_024)
            })
        );
        let mut radio = vec![0x18, 1];
        radio.extend((-112i16).to_le_bytes());
        radio.extend([(-60i8).cast_unsigned(), 26]);
        radio.extend(30u32.to_le_bytes());
        radio.extend(400u32.to_le_bytes());
        assert_eq!(
            Response::parse(&radio),
            Some(Response::Stats(Stats::Radio {
                noise_floor: -112,
                last_rssi: -60,
                last_snr: 6.5,
                tx_air_secs: 30,
                rx_air_secs: 400
            }))
        );
        let mut packets = vec![0x18, 2];
        for n in 1..=6u32 {
            packets.extend(n.to_le_bytes());
        }
        let Some(Response::Stats(Stats::Packets { receive_errors, .. })) =
            Response::parse(&packets)
        else {
            panic!("packet stats");
        };
        assert_eq!(receive_errors, None);
    }

    #[test]
    fn settings_are_laid_out_as_the_firmware_reads_them() {
        assert_eq!(command::set_tx_power(-9), [0x0c, 0xf7]);
        assert_eq!(
            command::set_radio_params(869_618, 62_500, 8, 8, Some(true)),
            [0x0b, 0xf2, 0x44, 0x0d, 0, 0x24, 0xf4, 0, 0, 8, 8, 1]
        );
        assert_eq!(
            command::set_radio_params(869_618, 62_500, 8, 8, None).len(),
            11
        );
        assert_eq!(
            command::set_other_params(true, 0b10_00_01, 1, 0),
            [0x26, 1, 0b10_00_01, 1, 0]
        );
        assert_eq!(command::set_position(1, -1)[5..], (-1i32).to_le_bytes());
        assert_eq!(command::get_stats(2), [0x38, 2]);
    }

    #[test]
    fn contacts_and_channels_are_laid_out_as_the_firmware_reads_them() {
        let contact = Contact {
            public_key: [0x50; 32],
            kind: 2,
            flags: 0,
            path_len: -1,
            path: Vec::new(),
            name: "Faro".into(),
            last_advert: 7,
            lat: 1,
            lon: -1,
            last_modified: 9,
        };
        let added = command::add_contact(&contact);
        // As the radio lists it, behind its command: it reads back the same.
        assert_eq!(added[0], 0x09);
        let mut listed = vec![0x03];
        listed.extend_from_slice(&added[1..]);
        assert_eq!(
            Response::parse(&listed),
            Some(Response::Contact(contact.clone()))
        );
        // One with a path written back keeps it, every hash of it.
        let routed = Contact {
            flags: 0x03,
            path_len: 2,
            path: vec![0x52, 0x55],
            ..contact.clone()
        };
        let mut listed = vec![0x03];
        listed.extend_from_slice(&command::add_contact(&routed)[1..]);
        assert_eq!(Response::parse(&listed), Some(Response::Contact(routed)));
        assert_eq!(command::get_contact(&[1; 32])[..2], [0x1e, 1]);
        assert_eq!(command::remove_contact(&[1; 32])[..2], [0x0f, 1]);
        assert_eq!(command::reset_path(&[1; 32]).len(), 33);
        assert_eq!(command::send_advert(true), [0x07, 1]);
        let set = command::set_channel(3, "#galicia", &[9; 16]);
        assert_eq!((set[0], set[1], set.len()), (0x20, 3, 2 + 32 + 16));
        assert_eq!(&set[2..10], b"#galicia");
        assert_eq!(set[33], 0);
    }

    #[test]
    fn too_short_is_nothing() {
        assert_eq!(Response::parse(&[]), None);
        assert_eq!(Response::parse(&[0x03, 1, 2, 3]), None);
        assert_eq!(Response::parse(&[0x82, 1]), None);
    }

    #[test]
    fn the_settings_each_answered_on_its_own_read_back() {
        // As a real radio answered them.
        let mut tuning = vec![0x17];
        tuning.extend(0u32.to_le_bytes());
        tuning.extend(1000u32.to_le_bytes());
        assert_eq!(
            Response::parse(&tuning),
            Some(Response::Tuning {
                rx_delay: 0,
                airtime_factor: 1000
            })
        );
        assert_eq!(
            Response::parse(&[0x19, 31, 0]),
            Some(Response::AutoAdd {
                config: 31,
                max_hops: 0
            })
        );
        let mut ranges = vec![0x1a];
        for khz in [433_000u32, 433_000, 869_495, 869_495] {
            ranges.extend(khz.to_le_bytes());
        }
        assert_eq!(
            Response::parse(&ranges),
            Some(Response::RepeatFrequencies(vec![
                (433_000, 433_000),
                (869_495, 869_495)
            ]))
        );
        // As the firmware lists them, and none at all without a GPS.
        assert_eq!(
            Response::parse(b"\x15gps:1,other:x"),
            Some(Response::CustomVars(vec![
                ("gps".into(), "1".into()),
                ("other".into(), "x".into())
            ]))
        );
        assert_eq!(Response::parse(&[0x15]), Some(Response::CustomVars(vec![])));
        assert_eq!(command::set_custom_var("gps", "0"), b"\x29gps:0");
        // No scope is the code alone; one, its name in 31 and its key.
        assert_eq!(Response::parse(&[0x1c]), Some(Response::DefaultScope(None)));
        let mut scope = vec![0x1c];
        scope.extend(command::set_default_scope(Some(("galicia", &[7; 16])))[1..].iter());
        assert_eq!(
            Response::parse(&scope),
            Some(Response::DefaultScope(Some(("galicia".into(), [7; 16]))))
        );
    }

    #[test]
    fn the_rest_of_the_settings_are_laid_out_as_the_firmware_reads_them() {
        assert_eq!(command::set_path_hash_mode(1), [0x3d, 0, 1]);
        assert_eq!(command::set_autoadd(15, 4), [0x3a, 15, 4]);
        let tuning = command::set_tuning(2500, 1000);
        assert_eq!(tuning[0], 0x15);
        assert_eq!(u32_at(&tuning, 1), Some(2500));
        assert_eq!(u32_at(&tuning, 5), Some(1000));
        // A name the firmware takes: under 31 bytes, ended by a zero.
        let scope = command::set_default_scope(Some((&"x".repeat(40), &[1; 16])));
        assert_eq!(scope.len(), 1 + 31 + 16);
        assert_eq!(scope[31], 0);
        assert_eq!(command::set_default_scope(None), [0x3f]);
    }
}
