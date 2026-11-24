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
//! - `react <slot> <emoji>`: someone reacts to the latest said on a channel,
//!   as MeshCore One does; `dmreact <emoji>`, Ana to the latest between her
//!   and the radio.
//! - `echoes off`: no repeater is heard passing on what the radio sends on a
//!   channel, as when none heard it; `echoes on`, two are again.
//! - `post <text>`: someone posts in Sala Coruña, a room: Ana, or someone
//!   the radio does not know, by turns; sent to the radio once logged in.
//! - `room missing`: the room is listed but not found to log in to, as a
//!   splitter in between may list a contact the radio no longer has.
//! - `room strict`: the room says nothing to a wrong password, as one that
//!   lets nobody in to read; `room open`, it lets them in to read again.
//!
//! Sala Coruña takes `admin` as its admin's password and `sala` as its
//! guests'; after a login it sends the posts the radio does not have, and
//! takes the radio's own only if it was let in to write.
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
//! one every radio has; its adverts only printed. Its tuning, auto-add, path
//! hash size and default flood scope are kept as the firmware keeps them.

/// The secret of `Public`, every radio's channel.
const PUBLIC_SECRET: [u8; 16] = [
    0x8b, 0x33, 0x87, 0xe9, 0xc5, 0xcd, 0xea, 0x6a, 0xc9, 0xe5, 0xed, 0xba, 0xa1, 0x15, 0xcd, 0x72,
];

/// The frequencies, in kHz, on which the firmware lets a client repeat.
const REPEAT_FREQUENCIES: [u32; 3] = [433_000, 869_495, 918_000];

use meshctl_core::meshcore::protocol::{FROM_CLIENT, FROM_RADIO};
use meshctl_core::meshcore::raw;
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
    /// Each hop's hash size, less one: 0 to 2.
    path_hash_mode: u8,
    /// Its receive delay base and airtime factor, in thousandths.
    tuning: (u32, u32),
    /// Which kinds it adds by itself, as bits, and up to how far.
    autoadd: (u8, u8),
    /// The region what it floods is scoped to, name and key.
    default_scope: Option<(String, [u8; 16])>,
    /// The region what it floods next goes in, as a client set it: none for
    /// its default.
    send_scope: Option<SendScope>,
    /// The latest said on each channel, by whom, and by its sender's clock.
    said: std::collections::HashMap<u8, (String, String, u32)>,
    /// The latest said between Ana and the radio, and when.
    with_ana: Option<(String, u32)>,
    /// Whether repeaters are heard passing on what is sent on a channel.
    echoes: bool,
    /// Sala Coruña's posts, its own among them: who wrote each, by the first
    /// four bytes of their key, what, and when by its clock.
    room_posts: Vec<([u8; 4], String, u32)>,
    /// What the radio logged in to Sala Coruña as, by its access list's
    /// bits; none until it does.
    room_access: Option<u8>,
    /// The newest of its posts the radio has, as it tells the room on
    /// logging in, which sends it only those after.
    room_synced: u32,
    /// Whether the room lets in to read one with a wrong password, or does
    /// not answer it.
    room_strict: bool,
    /// Whether the radio lists the room but does not find it to log in to,
    /// as when a splitter in between keeps contacts of its own.
    room_missing: bool,
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

/// The region a client set for what the radio floods next.
#[derive(Clone, Copy)]
enum SendScope {
    Key([u8; 16]),
    Unscoped,
}

impl Radio {
    /// The region what it floods now goes in, in words.
    fn scope_words(&self) -> String {
        match (&self.send_scope, &self.default_scope) {
            (Some(SendScope::Key(key)), _) => format!("scoped to key {}", hex_of(&key[..4])),
            (Some(SendScope::Unscoped), _) => "unscoped".to_owned(),
            (None, Some((name, _))) => format!("in its default {name:?}"),
            (None, None) => "unscoped, its default".to_owned(),
        }
    }

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
        // A #name, kept as the apps keep it: without its `#`, its secret
        // the start of the SHA-256 of `#galicia`.
        channels[1] = (
            "galicia".into(),
            [
                0x84, 0x0c, 0x97, 0x22, 0x39, 0xa5, 0xc7, 0x81, 0xde, 0x27, 0x4e, 0x01, 0x1f, 0xf1,
                0x81, 0x59,
            ],
        );
        channels[2] = ("Casa".into(), [0x17; 16]);
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
            // As a real radio has them: two-byte hashes, every kind added
            // and the oldest overwritten, no scope.
            path_hash_mode: 1,
            tuning: (0, 1000),
            autoadd: (31, 0),
            default_scope: None,
            send_scope: None,
            said: std::collections::HashMap::new(),
            with_ana: None,
            echoes: true,
            room_posts: vec![
                (
                    key_prefix(&key(0x53)),
                    "Bienvenidos a Sala Coruña".into(),
                    at - 7200,
                ),
                (
                    key_prefix(&key(0x41)),
                    "¿Alguien para la quedada del sábado?".into(),
                    at - 3600,
                ),
                (
                    key_prefix(&key(0x77)),
                    "Yo me apunto, llevo la antena nueva".into(),
                    at - 1800,
                ),
            ],
            room_access: None,
            room_synced: 0,
            room_strict: false,
            room_missing: false,
            clock_offset: -3600,
            started: at,
            contacts: vec![
                contact(0x41, 1, 0, "Ana", 43_365_000, -8_405_000),
                contact(0x52, 2, 1, "Repetidor Monte", 43_340_000, -8_380_000),
                contact(0x53, 3, 2, "Sala Coruña", 43_370_000, -8_396_000),
                contact(0x55, 2, 0, "Repetidor Torre", 43_386_000, -8_406_000),
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
    frame.extend([u8::from(radio.repeat), radio.path_hash_mode]);
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

/// Bytes in hex, to say them.
fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A whole key, from a command's bytes.
fn key_in(data: &[u8]) -> Option<[u8; 32]> {
    data.get(..32)?.try_into().ok()
}

/// A room's post, as the radio gives it: from the room, signed, with the
/// first four bytes of its author's key before its text, and the room's
/// clock when it was posted.
fn room_post(room: &Contact, author: [u8; 4], at: u32, text: &str) -> Vec<u8> {
    let snr = i8::try_from(20 + random(20)).unwrap_or(0);
    let mut frame = vec![0x10, snr.cast_unsigned(), 0, 0];
    frame.extend(&room.key[..6]);
    frame.extend([room.path_len.cast_unsigned(), 2]);
    frame.extend(at.to_le_bytes());
    frame.extend(author);
    frame.extend(text.as_bytes());
    frame
}

/// The posts of Sala Coruña the radio does not have yet, queued as the room
/// would send them to it, logged in, but not its own; and the newest kept
/// as had.
fn room_sync(radio: &mut Radio) -> usize {
    let Some(room) = radio.contact_named("Sala Coruña").cloned() else {
        return 0;
    };
    let new: Vec<_> = radio
        .room_posts
        .iter()
        .filter(|(author, _, at)| *at > radio.room_synced && author[..] != ME[..4])
        .cloned()
        .collect();
    for (author, text, at) in &new {
        radio.queued.push_back(room_post(&room, *author, *at, text));
        radio.room_synced = radio.room_synced.max(*at);
    }
    new.len()
}

/// The first four bytes of a key, as a room's post names its author by.
fn key_prefix(key: &[u8; 32]) -> [u8; 4] {
    [key[0], key[1], key[2], key[3]]
}

/// A direct message from a contact, in the newer form, with its SNR.
fn direct_message(from: &Contact, text: &str) -> Vec<u8> {
    direct_message_at(from, now(), text)
}

fn direct_message_at(from: &Contact, at: u32, text: &str) -> Vec<u8> {
    let snr = i8::try_from(20 + random(20)).unwrap_or(0);
    let mut frame = vec![0x10, snr.cast_unsigned(), 0, 0];
    frame.extend(&from.key[..6]);
    frame.extend([from.path_len.cast_unsigned(), 0]);
    frame.extend(at.to_le_bytes());
    frame.extend(text.as_bytes());
    frame
}

/// MeshCore One's hash of a message, which its reactions name it by: the
/// first 40 bits of the SHA-256 of its text and its sender's clock, in
/// lowercase Crockford base32.
fn reaction_hash(text: &str, at: u32) -> String {
    use sha2::{Digest, Sha256};
    const BASE32: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let digest = Sha256::new()
        .chain_update(text.trim_matches([' ', '\t']).as_bytes())
        .chain_update(at.to_le_bytes())
        .finalize();
    let bits = digest[..5]
        .iter()
        .fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    (0..8)
        .rev()
        .map(|i| BASE32[usize::try_from((bits >> (i * 5)) & 0x1f).unwrap_or(0)] as char)
        .collect()
}

/// A channel message, in the newer form: its sender only in its text.
fn channel_message(slot: u8, at: u32, sender: &str, text: &str) -> Vec<u8> {
    let snr = i8::try_from(random(40)).unwrap_or(0) - 20;
    let mut frame = vec![0x11, snr.cast_unsigned(), 0, 0, slot, 2, 0];
    frame.extend(at.to_le_bytes());
    frame.extend(format!("{sender}: {text}").as_bytes());
    frame
}

/// A packet heard, raw, as the radio logs it: its SNR in quarters of a dB,
/// its RSSI, and the packet.
fn rx_log(snr: i8, rssi: i8, packet: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x88, (snr * 4).cast_unsigned(), rssi.cast_unsigned()];
    frame.extend(packet);
    frame
}

/// A flooded packet, its path the hashes of those it went through, two
/// bytes each; in a region by its key, if given.
fn flooded(
    payload_type: u8,
    hops: &[[u8; 32]],
    region: Option<&[u8; 16]>,
    payload: &[u8],
) -> Vec<u8> {
    let route = if region.is_some() {
        raw::ROUTE_TRANSPORT_FLOOD
    } else {
        raw::ROUTE_FLOOD
    };
    let mut packet = vec![(payload_type << 2) | route];
    if let Some(key) = region {
        packet.extend(raw::transport_code(key, payload_type, payload).to_le_bytes());
        packet.extend([0, 0]);
    }
    packet.push(0x40 | u8::try_from(hops.len()).unwrap_or(0));
    for hop in hops {
        packet.extend(&hop[..2]);
    }
    packet.extend(payload);
    packet
}

/// The key of a region, by its name: the start of the SHA-256 of `#name`.
fn region_key(name: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("#{name}").as_bytes());
    digest[..16].try_into().unwrap_or_default()
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
            println!("direct text, {}", radio.scope_words());
            // To Ana: kept, for her to react to.
            if let (Some(at), Some(prefix), Some(text)) =
                (u32_in(data, 2), data.get(6..12), data.get(12..))
                && radio
                    .contact_named("Ana")
                    .is_some_and(|ana| &ana.key[..6] == prefix)
            {
                radio.with_ana = Some((String::from_utf8_lossy(text).into_owned(), at));
            }
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
            // To Sala Coruña: a post, taken and acknowledged only from one
            // let in to write; from any other, not even acknowledged.
            if to.name == "Sala Coruña" {
                if radio.room_access.is_some_and(|acl| acl >= 2)
                    && let (Some(at), Some(text)) = (u32_in(data, 2), data.get(12..))
                {
                    let text = String::from_utf8_lossy(text).into_owned();
                    println!("posted to the room: {text}");
                    radio.room_posts.push((key_prefix(&ME), text, at));
                    let events = events.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(900)).await;
                        let mut ack = vec![0x82];
                        ack.extend(tag);
                        let _ = events.send(Event::Push(ack));
                    });
                } else {
                    println!("not taken by the room: not let in to write");
                }
                return vec![sent];
            }
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
        // SEND_LOGIN: a key, then a password. Sala Coruña takes `admin` as
        // its admin's and `sala` as its guests', and lets in to read any
        // other unless strict, when it says nothing; then sends the posts
        // the radio does not have. Any other node answers no.
        0x1a => {
            let Some(key) = key_in(data) else {
                return error(6);
            };
            let Some(to) = radio
                .contacts
                .iter()
                .find(|c| c.key == key && !(radio.room_missing && c.name == "Sala Coruña"))
                .cloned()
            else {
                return error(2);
            };
            let password = String::from_utf8_lossy(data.get(32..).unwrap_or_default()).into_owned();
            println!("login to {} with {password:?}", to.name);
            let tag: [u8; 4] = key_prefix(&key);
            let mut sent = vec![0x06, u8::from(to.path_len < 0)];
            sent.extend(tag);
            sent.extend(4_000u32.to_le_bytes());
            let room = to.name == "Sala Coruña";
            let acl = match password.as_str() {
                "admin" if room => Some(3),
                "sala" if room => Some(2),
                _ if room && !radio.room_strict => Some(0),
                _ if room => None,
                _ => Some(0xff),
            };
            let clock = radio.clock();
            let events = events.clone();
            match acl {
                // Not a room: refused.
                Some(0xff) => {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(900)).await;
                        let mut refused = vec![0x86, 0];
                        refused.extend(&key[..6]);
                        let _ = events.send(Event::Push(refused));
                    });
                }
                Some(acl) => {
                    radio.room_access = Some(acl);
                    let waiting = room_sync(&mut radio);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(900)).await;
                        let mut taken = vec![0x85, u8::from(acl == 3)];
                        taken.extend(&key[..6]);
                        taken.extend(clock.to_le_bytes());
                        taken.extend([acl, 1]);
                        let _ = events.send(Event::Push(taken));
                        if waiting > 0 {
                            tokio::time::sleep(Duration::from_millis(1_500)).await;
                            let _ = events.send(Event::Push(vec![0x83]));
                        }
                    });
                }
                None => {}
            }
            vec![sent]
        }
        // SEND_ANON_REQ: a key, then the request: what it asks, and the way
        // back. Sent direct on a known path, or flooded without one, which
        // a repeater does not answer; one with a path answers its regions.
        0x39 => {
            let Some(key) = key_in(data) else {
                return error(6);
            };
            let Some(to) = radio.contacts.iter().find(|c| c.key == key).cloned() else {
                return error(3);
            };
            let tag: [u8; 4] = u32::try_from(random(u64::from(u32::MAX)))
                .unwrap_or(1)
                .to_le_bytes();
            let flood = to.path_len < 0;
            let mut sent = vec![0x06, u8::from(flood)];
            sent.extend(tag);
            sent.extend(2_500u32.to_le_bytes());
            let regions = data.get(32) == Some(&0x01);
            println!(
                "anonymous request {:?} to {:?}, {}",
                data.get(32),
                to.name,
                if flood { "flooded" } else { "direct" }
            );
            if regions && !flood && matches!(to.kind, 2 | 3) {
                let events = events.clone();
                // Each its own: the room only scoped, the tower on time.
                let (said, behind): (&[u8], u32) = match to.name.as_str() {
                    "Sala Coruña" => (b"coruna,galicia,", 86_400 * 3),
                    "Repetidor Torre" => (b"*,galicia,", 0),
                    _ => (b"*,europe,galicia", 86_400 * 900),
                };
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(600 + random(1_200))).await;
                    let mut answer = vec![0x8c, 0];
                    answer.extend(tag);
                    // A repeater's clock, behind as real ones often are.
                    answer.extend((now() - behind).to_le_bytes());
                    answer.extend(said);
                    answer.extend([0; 4]);
                    let _ = events.send(Event::Push(answer));
                });
            }
            vec![sent]
        }
        // SET_FLOOD_SCOPE_KEY: 0 and a key, 0 alone for its default, 1 for
        // none at all.
        0x36 => match data.first() {
            Some(0) => {
                radio.send_scope = data
                    .get(1..17)
                    .and_then(|k| k.try_into().ok())
                    .map(SendScope::Key);
                ok()
            }
            Some(1) => {
                radio.send_scope = Some(SendScope::Unscoped);
                ok()
            }
            _ => error(6),
        },
        // SEND_CHANNEL_TEXT: taken, and nothing ever said of it again.
        0x03 => {
            let slot = data.get(1).copied().unwrap_or(0);
            println!("channel text on slot {slot}, {}", radio.scope_words());
            // Kept, and heard back as two repeaters pass it on: Torre, then
            // Monte from Torre.
            if let (Some(at), Some(text)) = (u32_in(data, 2), data.get(6..)) {
                let text = String::from_utf8_lossy(text).into_owned();
                let name = radio.name.clone();
                radio.said.insert(slot, (name.clone(), text.clone(), at));
                if let (true, Some((_, secret))) =
                    (radio.echoes, radio.channels.get(usize::from(slot)))
                {
                    let payload = raw::seal_channel_text(secret, at, &format!("{name}: {text}"));
                    let first = flooded(raw::PAYLOAD_GRP_TXT, &[key(0x55)], None, &payload);
                    let second = flooded(
                        raw::PAYLOAD_GRP_TXT,
                        &[key(0x55), key(0x52)],
                        None,
                        &payload,
                    );
                    let events = events.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(700)).await;
                        let _ = events.send(Event::Push(rx_log(7, -88, &first)));
                        tokio::time::sleep(Duration::from_millis(900)).await;
                        let _ = events.send(Event::Push(rx_log(2, -104, &second)));
                    });
                }
            }
            vec![vec![0x00]]
        }
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
        // SET_TUNING_PARAMS, in thousandths, kept within the firmware's.
        0x15 => {
            let (Some(rx), Some(af)) = (u32_in(data, 0), u32_in(data, 4)) else {
                return error(6);
            };
            radio.tuning = (rx.min(20_000), af.min(9_000));
            println!("tuning {:?}", radio.tuning);
            ok()
        }
        // GET_TUNING_PARAMS
        0x2b => {
            let mut frame = vec![0x17];
            frame.extend(radio.tuning.0.to_le_bytes());
            frame.extend(radio.tuning.1.to_le_bytes());
            vec![frame]
        }
        // SET_AUTOADD_CONFIG: hops 64 at most.
        0x3a if !data.is_empty() => {
            radio.autoadd.0 = data[0];
            if let Some(&hops) = data.get(1) {
                radio.autoadd.1 = hops.min(64);
            }
            println!("auto-add {:05b}, hops {}", radio.autoadd.0, radio.autoadd.1);
            ok()
        }
        // GET_AUTOADD_CONFIG
        0x3b => vec![vec![0x19, radio.autoadd.0, radio.autoadd.1]],
        // GET_ALLOWED_REPEAT_FREQ
        0x3c => {
            let mut frame = vec![0x1a];
            for khz in REPEAT_FREQUENCIES {
                frame.extend(khz.to_le_bytes());
                frame.extend(khz.to_le_bytes());
            }
            vec![frame]
        }
        // SET_PATH_HASH_MODE: a reserved zero, then 0 to 2.
        0x3d if data.first() == Some(&0) && data.len() >= 2 => {
            if data[1] >= 3 {
                return error(6);
            }
            radio.path_hash_mode = data[1];
            println!("path hashes of {} bytes", data[1] + 1);
            ok()
        }
        // SET_DEFAULT_FLOOD_SCOPE: a name up to 30 bytes and a key, or none.
        0x3f => {
            if data.len() >= 31 + 16 {
                let end = data[..31].iter().position(|&b| b == 0).unwrap_or(31);
                if end == 0 || end >= 31 {
                    return error(6);
                }
                let name = String::from_utf8_lossy(&data[..end]).into_owned();
                let key: [u8; 16] = data[31..47].try_into().unwrap_or_default();
                println!("default scope {name:?}");
                radio.default_scope = Some((name, key));
            } else {
                println!("no default scope");
                radio.default_scope = None;
            }
            ok()
        }
        // GET_DEFAULT_FLOOD_SCOPE
        0x40 => match &radio.default_scope {
            Some((name, key)) => {
                let mut frame = vec![0x1c];
                frame.extend(field(name, 31));
                frame.extend(key);
                vec![frame]
            }
            None => vec![vec![0x1c]],
        },
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
            let at = now();
            radio
                .said
                .insert(slot, (sender.to_owned(), text.to_owned(), at));
            let message = channel_message(slot, at, sender, text);
            radio.queued.push_back(message);
            // Heard twice: through Monte and Torre, then through the room
            // and Torre; on galicia, in its region.
            let secret = radio.channels[usize::from(slot)].1;
            let payload = raw::seal_channel_text(&secret, at, &format!("{sender}: {text}"));
            let region = (slot == 1).then(|| region_key("galicia"));
            let first = flooded(
                raw::PAYLOAD_GRP_TXT,
                &[key(0x52), key(0x55)],
                region.as_ref(),
                &payload,
            );
            let second = flooded(
                raw::PAYLOAD_GRP_TXT,
                &[key(0x53), key(0x55)],
                region.as_ref(),
                &payload,
            );
            let _ = events.send(Event::Push(rx_log(6, -92, &first)));
            let _ = events.send(Event::Push(vec![0x83]));
            let events = events.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(900)).await;
                let _ = events.send(Event::Push(rx_log(3, -101, &second)));
            });
            format!("{sender} writes on slot {slot}")
        }
        Some("post") => {
            let text = words.collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return "usage: post <text>".into();
            }
            // Ana, or someone the radio does not know, by turns.
            let author = if radio.room_posts.len().is_multiple_of(2) {
                key_prefix(&key(0x41))
            } else {
                key_prefix(&key(0x77))
            };
            radio.room_posts.push((author, text, now()));
            if radio.room_access.is_some() && room_sync(&mut radio) > 0 {
                let _ = events.send(Event::Push(vec![0x83]));
                "posted, and sent to the radio".into()
            } else {
                "posted, kept until the radio logs in".into()
            }
        }
        Some("room") => {
            let word = words.next();
            radio.room_strict = word == Some("strict");
            radio.room_missing = word == Some("missing");
            format!(
                "the room {}",
                if radio.room_missing {
                    "is listed, but the radio does not find it to log in to"
                } else if radio.room_strict {
                    "says nothing to a wrong password"
                } else {
                    "lets in to read with a wrong password"
                }
            )
        }
        Some("echoes") => {
            radio.echoes = words.next() != Some("off");
            format!(
                "repeaters {}",
                if radio.echoes { "heard" } else { "silent" }
            )
        }
        Some("react") => {
            let slot = words.next().and_then(|w| w.parse::<u8>().ok());
            let (Some(slot), Some(emoji)) = (slot, words.next()) else {
                return "usage: react <slot> <emoji>".into();
            };
            let Some((sender, text, at)) = radio.said.get(&slot).cloned() else {
                return format!("nothing said on slot {slot}");
            };
            // Someone else than who said it, in MeshCore One's form.
            let reactor = if sender == "Uxía" { "Pepe" } else { "Uxía" };
            let reaction = format!("@[{sender}]{emoji}\n{}", reaction_hash(&text, at));
            radio
                .queued
                .push_back(channel_message(slot, now(), reactor, &reaction));
            let _ = events.send(Event::Push(vec![0x83]));
            format!("{reactor} reacts {emoji} to {sender}")
        }
        Some("dmreact") => {
            let Some(emoji) = words.next() else {
                return "usage: dmreact <emoji>".into();
            };
            let (Some(ana), Some((text, at))) =
                (radio.contact_named("Ana").cloned(), radio.with_ana.clone())
            else {
                return "nothing said with Ana".into();
            };
            let reaction = format!("{emoji}\n{}", reaction_hash(&text, at));
            radio.queued.push_back(direct_message(&ana, &reaction));
            let _ = events.send(Event::Push(vec![0x83]));
            format!("Ana reacts {emoji}")
        }
        Some("dm") => {
            let text = words.collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return "usage: dm <text>".into();
            }
            let Some(ana) = radio.contact_named("Ana").cloned() else {
                return "Ana is gone".into();
            };
            let at = now();
            radio.with_ana = Some((text.clone(), at));
            radio.queued.push_back(direct_message_at(&ana, at, &text));
            // Straight from her, sealed to the radio's key: whom it is to and
            // from, and what cannot be read here.
            let mut payload = vec![ME[0], ana.key[0]];
            payload.extend((0..18).map(|_| u8::try_from(random(256)).unwrap_or(0)));
            let mut packet = vec![(raw::PAYLOAD_TXT_MSG << 2) | raw::ROUTE_DIRECT, 0x40];
            packet.extend(payload);
            let _ = events.send(Event::Push(rx_log(9, -70, &packet)));
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
