// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A pretend MeshCore companion radio on TCP, for working on meshctl without
//! one, or without risking one: it speaks the companion protocol as the
//! firmware does, keeps its contacts and channels, queues the messages it
//! hears to be pulled, and acknowledges direct messages sent to it. It
//! transmits nothing, anywhere. What it does not know yet it refuses, as the
//! firmware does a command it lacks.
//!
//!     cargo run -p meshctl-core --example fake_meshcore -- [port] [control port]
//!
//! With a control port, it listens there, on this machine only, for lines
//! that play what the mesh would do to it:
//!
//! - `hear <slot> <text>`: someone writes that on a channel, as MeshCore
//!   clients do, `NAME: text`.
//! - `dm <text>`: Ana, a contact, writes to the radio directly.
//! - `advert`: a contact not known before advertises, and the radio adds it.
//! - `drop`: the link to every client drops, which reads it all anew.
//!
//! After a direct message to Ana, the radio says it was delivered, and now
//! and then she answers.
//!
//! Its own settings change as the firmware's do, each checked as it checks
//! them; its clock starts an hour behind, as a radio's does after a power
//! cut, and goes only forward. Contacts are added, removed and their paths
//! reset; channels set and emptied, each with its secret, `Public` with the
//! one every radio has; its adverts only printed.

/// The secret of `Public`, every radio's channel.
const PUBLIC_SECRET: [u8; 16] = [
    0x8b, 0x33, 0x87, 0xe9, 0xc5, 0xcd, 0xea, 0x6a, 0xc9, 0xe5, 0xed, 0xba, 0xa1, 0x15, 0xcd, 0x72,
];

/// The frequencies, in kHz, on which the firmware lets a client repeat.
const REPEAT_FREQUENCIES: [u32; 3] = [433_000, 869_495, 918_000];

use meshctl_core::meshcore::protocol::{FROM_CLIENT, FROM_RADIO};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

/// The radio's own key: its first four bytes are its node number in meshctl.
const ME: [u8; 32] = [0xc0; 32];
/// How many contacts and channel slots it says it has room for.
const MAX_CONTACTS: u16 = 350;
const MAX_CHANNELS: u8 = 8;

/// What the control port has happen to every client.
#[derive(Debug, Clone)]
enum Event {
    /// A frame the radio pushes on its own.
    Push(Vec<u8>),
    /// The link drops.
    Drop,
}

/// A contact the radio keeps.
#[derive(Debug, Clone)]
struct Contact {
    key: [u8; 32],
    /// 1 chat, 2 repeater, 3 room, 4 sensor.
    kind: u8,
    /// Hops on the path known to it; -1 when none is.
    path_len: i8,
    name: String,
    last_advert: u32,
    /// In millionths of a degree.
    lat: i32,
    lon: i32,
    last_modified: u32,
}

/// What the radio is set to, and what it holds.
struct Radio {
    name: String,
    lat: i32,
    lon: i32,
    /// In kHz and Hz.
    frequency: u32,
    bandwidth: u32,
    spreading_factor: u8,
    coding_rate: u8,
    tx_power: i8,
    repeat: bool,
    manual_add: bool,
    telemetry: u8,
    location_policy: u8,
    multi_acks: u8,
    /// Its clock, as an offset from the host's: it goes only forward.
    clock_offset: i64,
    /// When it started, for its uptime.
    started: u32,
    contacts: Vec<Contact>,
    /// Each slot's name and secret, both empty when unset.
    channels: Vec<(String, [u8; 16])>,
    /// Messages heard, waiting to be pulled, as the frames that carry them.
    queued: VecDeque<Vec<u8>>,
}

fn now() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
}

/// A number from 0 up to, not including, `below`: not for secrets, only so
/// that answers come at their own time.
fn random(below: u64) -> u64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x2545_f491, |d| d.as_nanos() as u64)
            | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x % below.max(1)
}

/// A key told apart by its first byte, the rest made up from it.
fn key(first: u8) -> [u8; 32] {
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = first.wrapping_add(u8::try_from(i).unwrap_or(0).wrapping_mul(7));
    }
    key
}

impl Radio {
    fn new() -> Self {
        let at = now();
        let contact = |first, kind, path_len, name: &str, lat, lon| Contact {
            key: key(first),
            kind,
            path_len,
            name: name.into(),
            last_advert: at - 600,
            lat,
            lon,
            last_modified: at - 600,
        };
        let mut channels = vec![(String::new(), [0u8; 16]); usize::from(MAX_CHANNELS)];
        channels[0] = ("Public".into(), PUBLIC_SECRET);
        channels[1] = ("galicia".into(), [0x17; 16]);
        Self {
            name: "Fake core".into(),
            lat: 43_362_300,
            lon: -8_411_500,
            frequency: 869_618,
            bandwidth: 62_500,
            spreading_factor: 8,
            coding_rate: 8,
            tx_power: 22,
            repeat: false,
            manual_add: false,
            telemetry: 0,
            location_policy: 1,
            multi_acks: 0,
            clock_offset: -3600,
            started: at,
            contacts: vec![
                contact(0x41, 1, 0, "Ana", 43_365_000, -8_405_000),
                contact(0x52, 2, 1, "Repetidor Monte", 43_340_000, -8_380_000),
                contact(0x53, 3, 2, "Sala Coruña", 43_370_000, -8_396_000),
                contact(0x54, 4, -1, "Sensor Faro", 43_385_800, -8_406_400),
            ],
            channels,
            queued: VecDeque::new(),
        }
    }

    /// Its clock, by the seconds.
    fn clock(&self) -> u32 {
        u32::try_from(i64::from(now()) + self.clock_offset).unwrap_or(0)
    }

    fn contact_named(&self, name: &str) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.name == name)
    }
}

/// A fixed field: the text, cut or padded with zeros to `len`.
fn field(text: &str, len: usize) -> Vec<u8> {
    let mut field = vec![0u8; len];
    let bytes = text.as_bytes();
    let n = bytes.len().min(len);
    field[..n].copy_from_slice(&bytes[..n]);
    field
}

fn self_info(radio: &Radio) -> Vec<u8> {
    let mut frame = vec![0x05, 1, radio.tx_power.cast_unsigned(), 22];
    frame.extend(ME);
    frame.extend(radio.lat.to_le_bytes());
    frame.extend(radio.lon.to_le_bytes());
    frame.extend([
        radio.multi_acks,
        radio.location_policy,
        radio.telemetry,
        u8::from(radio.manual_add),
    ]);
    frame.extend(radio.frequency.to_le_bytes());
    frame.extend(radio.bandwidth.to_le_bytes());
    frame.extend([radio.spreading_factor, radio.coding_rate]);
    frame.extend(radio.name.as_bytes());
    frame
}

fn device_info(radio: &Radio) -> Vec<u8> {
    let mut frame = vec![
        0x0d,
        14,
        u8::try_from(MAX_CONTACTS / 2).unwrap_or(0),
        MAX_CHANNELS,
    ];
    // The BLE pin.
    frame.extend([0, 0, 0, 0]);
    frame.extend(field("23 Nov 2026", 12));
    frame.extend(field("Fake MeshCore", 40));
    frame.extend(field("v1.18.0-fake", 20));
    // Repeat (v9+) and the path's hash size, less one (v10+).
    frame.extend([u8::from(radio.repeat), 0]);
    frame
}

/// An error, by the firmware's code: 1 unsupported, 2 not found, 6 an
/// illegal argument.
fn error(code: u8) -> Vec<Vec<u8>> {
    vec![vec![0x01, code]]
}

fn ok() -> Vec<Vec<u8>> {
    vec![vec![0x00]]
}

fn u32_in(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)?
        .try_into()
        .ok()
        .map(u32::from_le_bytes)
}

fn i32_in(data: &[u8], at: usize) -> Option<i32> {
    data.get(at..at + 4)?
        .try_into()
        .ok()
        .map(i32::from_le_bytes)
}

/// A contact, as listed (0x03) or as pushed when first heard (0x8a).
fn contact(code: u8, contact: &Contact) -> Vec<u8> {
    let mut frame = vec![code];
    frame.extend(contact.key);
    frame.extend([contact.kind, 0, contact.path_len.cast_unsigned()]);
    frame.extend([0u8; 64]);
    frame.extend(field(&contact.name, 32));
    frame.extend(contact.last_advert.to_le_bytes());
    frame.extend(contact.lat.to_le_bytes());
    frame.extend(contact.lon.to_le_bytes());
    frame.extend(contact.last_modified.to_le_bytes());
    frame
}

fn channel_info(index: u8, name: &str, secret: &[u8; 16]) -> Vec<u8> {
    let mut frame = vec![0x12, index];
    frame.extend(field(name, 32));
    frame.extend(secret);
    frame
}

/// A whole key, from a command's bytes.
fn key_in(data: &[u8]) -> Option<[u8; 32]> {
    data.get(..32)?.try_into().ok()
}

/// A direct message from a contact, in the newer form, with its SNR.
fn direct_message(from: &Contact, text: &str) -> Vec<u8> {
    let snr = i8::try_from(20 + random(20)).unwrap_or(0);
    let mut frame = vec![0x10, snr.cast_unsigned(), 0, 0];
    frame.extend(&from.key[..6]);
    frame.extend([from.path_len.cast_unsigned(), 0]);
    frame.extend(now().to_le_bytes());
    frame.extend(text.as_bytes());
    frame
}

/// A channel message, in the newer form: its sender only in its text.
fn channel_message(slot: u8, sender: &str, text: &str) -> Vec<u8> {
    let snr = i8::try_from(random(40)).unwrap_or(0) - 20;
    let hops = u8::try_from(random(4)).unwrap_or(0);
    let mut frame = vec![0x11, snr.cast_unsigned(), 0, 0, slot, hops, 0];
    frame.extend(now().to_le_bytes());
    frame.extend(format!("{sender}: {text}").as_bytes());
    frame
}

/// What the radio answers a command, and what it pushes after a while.
fn answer(command: &[u8], radio: &Mutex<Radio>, events: &broadcast::Sender<Event>) -> Vec<Vec<u8>> {
    let Some((&code, data)) = command.split_first() else {
        return Vec::new();
    };
    let mut radio = radio.lock().unwrap_or_else(|e| e.into_inner());
    match code {
        // APP_START
        0x01 => vec![self_info(&radio)],
        // SEND_TEXT: to the contact its key prefix names.
        0x02 => {
            let Some(prefix) = data.get(6..12) else {
                return vec![vec![0x01, 2]];
            };
            let Some(to) = radio
                .contacts
                .iter()
                .find(|c| &c.key[..6] == prefix)
                .cloned()
            else {
                return vec![vec![0x01, 2]];
            };
            let tag: [u8; 4] = u32::try_from(random(u64::from(u32::MAX)))
                .unwrap_or(1)
                .to_le_bytes();
            let mut sent = vec![0x06, 0];
            sent.extend(tag);
            sent.extend(8_000u32.to_le_bytes());
            // Delivered after a moment, and now and then answered.
            let events = events.clone();
            let answers = to.kind == 1 && random(3) == 0;
            if answers {
                radio
                    .queued
                    .push_back(direct_message(&to, "¡Recibido! Te leo alto y claro."));
            }
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(800 + random(2_000))).await;
                let mut ack = vec![0x82];
                ack.extend(tag);
                let _ = events.send(Event::Push(ack));
                if answers {
                    tokio::time::sleep(Duration::from_secs(2 + random(4))).await;
                    let _ = events.send(Event::Push(vec![0x83]));
                }
            });
            vec![sent]
        }
        // SEND_CHANNEL_TEXT: taken, and nothing ever said of it again.
        0x03 => vec![vec![0x00]],
        // GET_CONTACTS, those changed since a time if one is given.
        0x04 => {
            let since = data
                .get(..4)
                .and_then(|b| b.try_into().ok())
                .map_or(0, u32::from_le_bytes);
            let changed: Vec<&Contact> = radio
                .contacts
                .iter()
                .filter(|c| c.last_modified > since)
                .collect();
            let mut frames = vec![{
                let mut start = vec![0x02];
                start.extend(u32::try_from(changed.len()).unwrap_or(0).to_le_bytes());
                start
            }];
            frames.extend(changed.iter().map(|c| contact(0x03, c)));
            let latest = radio
                .contacts
                .iter()
                .map(|c| c.last_modified)
                .max()
                .unwrap_or(0);
            let mut end = vec![0x04];
            end.extend(latest.to_le_bytes());
            frames.push(end);
            frames
        }
        // SYNC_NEXT_MESSAGE
        0x0a => vec![radio.queued.pop_front().unwrap_or_else(|| vec![0x0a])],
        // GET_DEVICE_TIME
        0x05 => {
            let mut frame = vec![0x09];
            frame.extend(radio.clock().to_le_bytes());
            vec![frame]
        }
        // SET_DEVICE_TIME: only forward.
        0x06 => match u32_in(data, 0) {
            Some(secs) if secs >= radio.clock() => {
                radio.clock_offset = i64::from(secs) - i64::from(now());
                println!("clock set, {} s off now", radio.clock_offset);
                ok()
            }
            _ => error(6),
        },
        // SET_ADVERT_NAME: 31 bytes kept.
        0x08 if !data.is_empty() => {
            let name = &data[..data.len().min(31)];
            radio.name = String::from_utf8_lossy(name).into_owned();
            println!("named {:?}", radio.name);
            ok()
        }
        // SET_RADIO_PARAMS, client repeat with them.
        0x0b => {
            let (Some(frequency), Some(bandwidth), Some(&sf), Some(&cr)) =
                (u32_in(data, 0), u32_in(data, 4), data.get(8), data.get(9))
            else {
                return error(6);
            };
            let repeat = data.get(10).is_some_and(|&r| r != 0);
            if repeat && !REPEAT_FREQUENCIES.contains(&frequency) {
                return error(6);
            }
            if !(150_000..=2_500_000).contains(&frequency)
                || !(7_000..=500_000).contains(&bandwidth)
                || !(5..=12).contains(&sf)
                || !(5..=8).contains(&cr)
            {
                return error(6);
            }
            (radio.frequency, radio.bandwidth) = (frequency, bandwidth);
            (radio.spreading_factor, radio.coding_rate) = (sf, cr);
            radio.repeat = repeat;
            println!("radio {frequency} kHz, {bandwidth} Hz, SF{sf} CR{cr}, repeat {repeat}");
            ok()
        }
        // SET_RADIO_TX_POWER
        0x0c => match data.first().map(|&p| p.cast_signed()) {
            Some(dbm @ -9..=22) => {
                radio.tx_power = dbm;
                println!("tx power {dbm} dBm");
                ok()
            }
            _ => error(6),
        },
        // SET_ADVERT_LATLON
        0x0e => match (i32_in(data, 0), i32_in(data, 4)) {
            (Some(lat), Some(lon))
                if (-90_000_000..=90_000_000).contains(&lat)
                    && (-180_000_000..=180_000_000).contains(&lon) =>
            {
                (radio.lat, radio.lon) = (lat, lon);
                println!("at {lat}, {lon}");
                ok()
            }
            _ => error(6),
        },
        // GET_BATT_AND_STORAGE
        0x14 => {
            let mut frame = vec![0x0c];
            frame.extend(
                u16::try_from(3_900 + random(250))
                    .unwrap_or(4_000)
                    .to_le_bytes(),
            );
            frame.extend(118u32.to_le_bytes());
            frame.extend(1_408u32.to_le_bytes());
            vec![frame]
        }
        // DEVICE_QUERY
        0x16 => vec![device_info(&radio)],
        // SET_OTHER_PARAMS
        0x26 if !data.is_empty() => {
            radio.manual_add = data[0] != 0;
            if let Some(&telemetry) = data.get(1) {
                radio.telemetry = telemetry & 0x3f;
            }
            if let Some(&policy) = data.get(2) {
                radio.location_policy = policy;
            }
            if let Some(&acks) = data.get(3) {
                radio.multi_acks = acks;
            }
            println!(
                "manual add {}, telemetry {:06b}, location {}, acks {}",
                radio.manual_add, radio.telemetry, radio.location_policy, radio.multi_acks
            );
            ok()
        }
        // GET_STATS, by kind.
        0x38 => {
            let up = now() - radio.started;
            match data.first() {
                Some(0) => {
                    let mut frame = vec![0x18, 0];
                    frame.extend(
                        u16::try_from(3_900 + random(250))
                            .unwrap_or(4_000)
                            .to_le_bytes(),
                    );
                    frame.extend(up.to_le_bytes());
                    frame.extend(0u16.to_le_bytes());
                    frame.push(0);
                    vec![frame]
                }
                Some(1) => {
                    let mut frame = vec![0x18, 1];
                    let noise = -115 + i16::try_from(random(8)).unwrap_or(0);
                    frame.extend(noise.to_le_bytes());
                    frame.push((-70 - i8::try_from(random(30)).unwrap_or(0)).cast_unsigned());
                    frame.push(i8::try_from(random(40)).unwrap_or(0).cast_unsigned());
                    frame.extend((up / 400).to_le_bytes());
                    frame.extend((up / 25).to_le_bytes());
                    vec![frame]
                }
                Some(2) => {
                    let mut frame = vec![0x18, 2];
                    let received = up / 9;
                    for n in [
                        received,
                        up / 60,
                        up / 120,
                        up / 120,
                        received / 2,
                        received / 2,
                        3,
                    ] {
                        frame.extend(n.to_le_bytes());
                    }
                    vec![frame]
                }
                _ => error(6),
            }
        }
        // GET_CHANNEL: past the last slot, an error.
        0x1f => {
            let index = data.first().copied().unwrap_or(0);
            match radio.channels.get(usize::from(index)) {
                Some((name, secret)) => vec![channel_info(index, name, secret)],
                None => vec![vec![0x01, 2]],
            }
        }
        // SET_CHANNEL: a name of 32 and a secret of 16; only 128 bits.
        0x20 => {
            let (Some(&index), Some(name), Some(secret)) =
                (data.first(), data.get(1..33), data.get(33..49))
            else {
                return error(6);
            };
            if data.len() >= 1 + 32 + 32 {
                return error(1);
            }
            let Some(slot) = radio.channels.get_mut(usize::from(index)) else {
                return error(2);
            };
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            let name = String::from_utf8_lossy(&name[..end]).into_owned();
            let secret: [u8; 16] = secret.try_into().unwrap_or_default();
            println!("channel {index}: {name:?}");
            *slot = (name, secret);
            ok()
        }
        // SEND_SELF_ADVERT: said, to nobody here.
        0x07 => {
            let flood = data.first() == Some(&1);
            println!("advert, {}", if flood { "flooded" } else { "zero hop" });
            ok()
        }
        // ADD_UPDATE_CONTACT: added, or the one with its key updated.
        0x09 if data.len() >= 32 + 3 + 64 + 32 + 4 => {
            let Some(key) = key_in(data) else {
                return error(6);
            };
            let name_field = &data[99..131];
            let end = name_field.iter().position(|&b| b == 0).unwrap_or(32);
            let at = radio.clock();
            let contact = Contact {
                key,
                kind: data[32],
                path_len: data[34].cast_signed(),
                name: String::from_utf8_lossy(&name_field[..end]).into_owned(),
                last_advert: u32_in(data, 131).unwrap_or(at),
                lat: i32_in(data, 135).unwrap_or(0),
                lon: i32_in(data, 139).unwrap_or(0),
                last_modified: at,
            };
            println!("contact {:?} kept", contact.name);
            if let Some(known) = radio.contacts.iter_mut().find(|c| c.key == key) {
                *known = contact;
            } else if radio.contacts.len() < usize::from(MAX_CONTACTS) {
                radio.contacts.push(contact);
            } else {
                return error(3);
            }
            ok()
        }
        // RESET_PATH
        0x0d => match key_in(data).and_then(|k| radio.contacts.iter_mut().find(|c| c.key == k)) {
            Some(contact) => {
                contact.path_len = -1;
                println!("path to {:?} forgotten", contact.name);
                ok()
            }
            None => error(2),
        },
        // REMOVE_CONTACT
        0x0f => {
            let Some(key) = key_in(data) else {
                return error(2);
            };
            let before = radio.contacts.len();
            radio.contacts.retain(|c| c.key != key);
            if radio.contacts.len() == before {
                return error(2);
            }
            println!("contact removed");
            ok()
        }
        // Anything else, not known yet: as the firmware does a command it
        // lacks.
        other => {
            println!("command 0x{other:02x} refused, not known yet");
            vec![vec![0x01, 1]]
        }
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut framed = vec![FROM_RADIO];
    framed.extend(u16::try_from(payload.len()).unwrap_or(0).to_le_bytes());
    framed.extend(payload);
    framed
}

/// One client, from connecting until it goes.
async fn serve(
    stream: TcpStream,
    radio: Arc<Mutex<Radio>>,
    events: broadcast::Sender<Event>,
) -> std::io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let mut pushed = events.subscribe();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        tokio::select! {
            read = reader.read(&mut chunk) => {
                let n = read?;
                if n == 0 {
                    return Ok(());
                }
                buffer.extend_from_slice(&chunk[..n]);
                // Every whole command in what came, each answered in turn.
                loop {
                    let Some(start) = buffer.iter().position(|&b| b == FROM_CLIENT) else {
                        buffer.clear();
                        break;
                    };
                    buffer.drain(..start);
                    if buffer.len() < 3 {
                        break;
                    }
                    let len = usize::from(u16::from_le_bytes([buffer[1], buffer[2]]));
                    if buffer.len() < 3 + len {
                        break;
                    }
                    let command: Vec<u8> = buffer.drain(..3 + len).skip(3).collect();
                    for answer in answer(&command, &radio, &events) {
                        writer.write_all(&frame(&answer)).await?;
                    }
                }
            }
            event = pushed.recv() => match event {
                Ok(Event::Push(payload)) => writer.write_all(&frame(&payload)).await?,
                Ok(Event::Drop) => return Ok(()),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
        }
    }
}

/// Does what a line of the control port says; what to answer it.
fn control(line: &str, radio: &Mutex<Radio>, events: &broadcast::Sender<Event>) -> String {
    let mut words = line.trim().splitn(3, ' ');
    let mut radio = radio.lock().unwrap_or_else(|e| e.into_inner());
    match words.next() {
        Some("hear") => {
            let slot = words.next().and_then(|w| w.parse::<u8>().ok());
            let (Some(slot), Some(text)) = (slot, words.next()) else {
                return "usage: hear <slot> <text>".into();
            };
            if radio
                .channels
                .get(usize::from(slot))
                .is_none_or(|(name, _)| name.is_empty())
            {
                return format!("no channel in slot {slot}");
            }
            const SENDERS: [&str; 4] = ["Pepe", "Uxía", "Brais 🚲", "EA1XYZ"];
            let sender = SENDERS[usize::try_from(random(4)).unwrap_or(0)];
            let message = channel_message(slot, sender, text);
            radio.queued.push_back(message);
            let _ = events.send(Event::Push(vec![0x83]));
            format!("{sender} writes on slot {slot}")
        }
        Some("dm") => {
            let text = words.collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return "usage: dm <text>".into();
            }
            let Some(ana) = radio.contact_named("Ana").cloned() else {
                return "Ana is gone".into();
            };
            radio.queued.push_back(direct_message(&ana, &text));
            let _ = events.send(Event::Push(vec![0x83]));
            "Ana writes".into()
        }
        Some("advert") => {
            let at = now();
            let n = u8::try_from(radio.contacts.len()).unwrap_or(0);
            let new = Contact {
                key: key(0x60 + n),
                kind: 1,
                path_len: i8::try_from(random(3)).unwrap_or(0),
                name: format!("Nodo nuevo {n}"),
                last_advert: at,
                lat: 43_350_000 + i32::try_from(random(40_000)).unwrap_or(0),
                lon: -8_420_000 + i32::try_from(random(40_000)).unwrap_or(0),
                last_modified: at,
            };
            let name = new.name.clone();
            let _ = events.send(Event::Push(contact(0x8a, &new)));
            radio.contacts.push(new);
            format!("{name} advertises")
        }
        Some("drop") => {
            let _ = events.send(Event::Drop);
            "dropped".into()
        }
        _ => "commands: hear <slot> <text>, dm <text>, advert, drop".into(),
    }
}

async fn controlled(
    stream: TcpStream,
    radio: Arc<Mutex<Radio>>,
    events: broadcast::Sender<Event>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let said = control(&line, &radio, &events);
        writer.write_all(format!("{said}\n").as_bytes()).await?;
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().map_or(Ok(5000), |p| p.parse())?;
    let control_port: Option<u16> = args.next().map(|p| p.parse()).transpose()?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let radio = Arc::new(Mutex::new(Radio::new()));
    let (events, _) = broadcast::channel(64);
    println!("fake MeshCore radio on port {port}");
    if let Some(control_port) = control_port {
        let control = TcpListener::bind(("127.0.0.1", control_port)).await?;
        println!("control on 127.0.0.1:{control_port}");
        let radio = Arc::clone(&radio);
        let events = events.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = control.accept().await {
                let (radio, events) = (Arc::clone(&radio), events.clone());
                tokio::spawn(async move {
                    let _ = controlled(stream, radio, events).await;
                });
            }
        });
    }
    loop {
        let (stream, address) = listener.accept().await?;
        println!("client {address}");
        let radio = Arc::clone(&radio);
        let events = events.clone();
        tokio::spawn(async move {
            if let Err(error) = serve(stream, radio, events).await {
                println!("client {address} gone: {error}");
            }
        });
    }
}
