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
    const SET_ADVERT_NAME: u8 = 0x08;
    const SYNC_NEXT_MESSAGE: u8 = 0x0a;
    const SET_RADIO_PARAMS: u8 = 0x0b;
    const SET_RADIO_TX_POWER: u8 = 0x0c;
    const SET_ADVERT_LATLON: u8 = 0x0e;
    const GET_BATT_AND_STORAGE: u8 = 0x14;
    const DEVICE_QUERY: u8 = 0x16;
    const GET_CHANNEL: u8 = 0x1f;
    const SET_OTHER_PARAMS: u8 = 0x26;
    const GET_STATS: u8 = 0x38;
    /// Plain text, as opposed to a CLI command or a signed message.
    const TEXT_PLAIN: u8 = 0;
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

    /// Answered with `Battery`.
    pub fn get_battery() -> Vec<u8> {
        vec![GET_BATT_AND_STORAGE]
    }

    /// Answered with `Stats` of that kind: 0 core, 1 radio, 2 packets.
    pub fn get_stats(kind: u8) -> Vec<u8> {
        vec![GET_STATS, kind]
    }
}

/// A contact the radio keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub public_key: [u8; 32],
    /// 1 chat, 2 repeater, 3 room, 4 sensor.
    pub kind: u8,
    pub flags: u8,
    /// Hops on the path known to it; negative when none is.
    pub path_len: i8,
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
    /// A channel's slot and name; its secret is not kept.
    ChannelInfo {
        index: u8,
        name: String,
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
            },
            0x18 => Self::Stats(stats(data)?),
            0x80 => Self::Advert,
            0x81 => Self::PathUpdate,
            0x82 => Self::Ack(bytes_at(data, 0)?),
            0x83 => Self::MessagesWaiting,
            0x8a => Self::NewContact(contact(data)?),
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
        // 64 bytes of path at 35, which meshctl does not use.
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
        assert!(Response::parse(&[0x88, 0]).unwrap().is_push());
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
                name: "galicia".into()
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
    fn too_short_is_nothing() {
        assert_eq!(Response::parse(&[]), None);
        assert_eq!(Response::parse(&[0x03, 1, 2, 3]), None);
        assert_eq!(Response::parse(&[0x82, 1]), None);
    }
}
