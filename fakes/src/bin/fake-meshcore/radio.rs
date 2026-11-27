// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio: what it has and is set to, as the firmware keeps it.

use crate::frames::{hex_of, key_prefix, region_key};
use meshctl_fakes::now;
use std::collections::VecDeque;

/// Where its GPS says it is, in millionths of a degree: the Tower of
/// Hercules.
pub const GPS_AT: (i32, i32) = (43_385_833, -8_406_389);

/// The frequencies, in kHz, on which the firmware lets a client repeat.
pub const REPEAT_FREQUENCIES: [u32; 3] = [433_000, 869_495, 918_000];

/// The radio's own key: its first four bytes are its node number in meshctl.
pub const ME: [u8; 32] = [0xc0; 32];
/// How many contacts and channel slots it says it has room for.
pub const MAX_CONTACTS: u16 = 350;
pub const MAX_CHANNELS: u8 = 8;

/// What the control port has happen to every client.
#[derive(Debug, Clone)]
pub enum Event {
    /// A frame the radio pushes on its own.
    Push(Vec<u8>),
    /// The link drops.
    Drop,
}

/// A contact the radio keeps.
#[derive(Debug, Clone)]
pub struct Contact {
    pub key: [u8; 32],
    /// 1 chat, 2 repeater, 3 room, 4 sensor.
    pub kind: u8,
    /// Hops on the path known to it; -1 when none is.
    pub path_len: i8,
    pub name: String,
    pub last_advert: u32,
    /// In millionths of a degree.
    pub lat: i32,
    pub lon: i32,
    pub last_modified: u32,
}

/// What the radio is set to, and what it holds.
pub struct Radio {
    pub name: String,
    pub lat: i32,
    pub lon: i32,
    /// In kHz and Hz.
    pub frequency: u32,
    pub bandwidth: u32,
    pub spreading_factor: u8,
    pub coding_rate: u8,
    pub tx_power: i8,
    pub repeat: bool,
    pub manual_add: bool,
    pub telemetry: u8,
    pub location_policy: u8,
    pub multi_acks: u8,
    /// Each hop's hash size, less one: 0 to 2.
    pub path_hash_mode: u8,
    /// Its receive delay base and airtime factor, in thousandths.
    pub tuning: (u32, u32),
    /// Which kinds it adds by itself, as bits, and up to how far.
    pub autoadd: (u8, u8),
    /// The region what it floods is scoped to, name and key.
    pub default_scope: Option<(String, [u8; 16])>,
    /// Where it was told it is, kept while its GPS is on and says instead.
    pub told: Option<(i32, i32)>,
    /// The region what it floods next goes in, as a client set it: none for
    /// its default.
    pub send_scope: Option<SendScope>,
    /// The latest said on each channel, by whom, and by its sender's clock.
    pub said: std::collections::HashMap<u8, (String, String, u32)>,
    /// The latest said between Ana and the radio, and when.
    pub with_ana: Option<(String, u32)>,
    /// Whether repeaters are heard passing on what is sent on a channel.
    pub echoes: bool,
    /// Whether pings come back and requests are answered.
    pub answers: bool,
    /// Sala Coruña's posts, its own among them: who wrote each, by the first
    /// four bytes of their key, what, and when by its clock.
    pub room_posts: Vec<([u8; 4], String, u32)>,
    /// What the radio logged in to Sala Coruña as, by its access list's
    /// bits; none until it does.
    pub room_access: Option<u8>,
    /// The newest of its posts the radio has, as it tells the room on
    /// logging in, which sends it only those after.
    pub room_synced: u32,
    /// Whether the room lets in to read one with a wrong password, or does
    /// not answer it.
    pub room_strict: bool,
    /// Whether the radio lists the room but does not find it to log in to,
    /// as when a splitter in between keeps contacts of its own.
    pub room_missing: bool,
    /// What the radio logged in to each repeater as, by its name.
    pub logins: std::collections::HashMap<String, u8>,
    /// Its clock, as an offset from the host's: it goes only forward.
    pub clock_offset: i64,
    /// When it started, for its uptime.
    pub started: u32,
    /// How busy it hears the channel besides, in percent, from when, and
    /// the seconds received that way so far.
    pub busy: (u32, u32, u32),
    pub contacts: Vec<Contact>,
    /// Each slot's name and secret, both empty when unset.
    pub channels: Vec<(String, [u8; 16])>,
    /// Messages heard, waiting to be pulled, as the frames that carry them.
    pub queued: VecDeque<Vec<u8>>,
    /// How many nodes not known before have advertised, each with a key of
    /// its own.
    pub adverts: u16,
}

/// A key told apart by its first byte, the rest made up from it.
pub fn key(first: u8) -> [u8; 32] {
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = first.wrapping_add(u8::try_from(i).unwrap_or(0).wrapping_mul(7));
    }
    key
}

/// The region a client set for what the radio floods next.
#[derive(Clone, Copy)]
pub enum SendScope {
    Key([u8; 16]),
    Unscoped,
}

impl Radio {
    /// The region what it floods now goes in, in words.
    pub fn scope_words(&self) -> String {
        match (&self.send_scope, &self.default_scope) {
            (Some(SendScope::Key(key)), _) => format!("scoped to key {}", hex_of(&key[..4])),
            (Some(SendScope::Unscoped), _) => "unscoped".to_owned(),
            (None, Some((name, _))) => format!("in its default {name:?}"),
            (None, None) => "unscoped, its default".to_owned(),
        }
    }

    pub fn new() -> Self {
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
        // Galicia's, as a radio there has them: the first after the preset
        // the mesh there uses, and then the provinces'. Each a #name, kept
        // as the apps keep it, without its `#`, its secret the start of the
        // SHA-256 of `#name`.
        for (slot, name) in [
            "NarrowFast",
            "Iberia",
            "Galicia",
            "ACoruña",
            "Lugo",
            "Ourense",
            "Pontevedra",
        ]
        .into_iter()
        .enumerate()
        {
            channels[slot] = (name.into(), region_key(name));
        }
        Self {
            // A ship of the Culture's, as Iain M. Banks named them: Just Testing.
            name: "Solo probando".into(),
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
            told: None,
            send_scope: None,
            said: std::collections::HashMap::new(),
            with_ana: None,
            echoes: true,
            answers: true,
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
            logins: std::collections::HashMap::new(),
            clock_offset: -3600,
            started: at,
            busy: (0, at, 0),
            contacts: vec![
                contact(0x41, 1, 0, "Ana", 43_365_000, -8_405_000),
                contact(0x52, 2, 1, "Repetidor Monte", 43_340_000, -8_380_000),
                contact(0x53, 3, 2, "Sala Coruña", 43_370_000, -8_396_000),
                contact(0x55, 2, 0, "Repetidor Torre", 43_386_000, -8_406_000),
                contact(0x54, 4, -1, "Sensor Faro", 43_385_800, -8_406_400),
            ],
            channels,
            queued: VecDeque::new(),
            adverts: 0,
        }
    }

    /// Its clock, by the seconds.
    pub fn clock(&self) -> u32 {
        u32::try_from(i64::from(now()) + self.clock_offset).unwrap_or(0)
    }

    pub fn contact_named(&self, name: &str) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.name == name)
    }
}
