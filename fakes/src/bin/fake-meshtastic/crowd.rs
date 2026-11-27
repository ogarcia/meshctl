// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A crowd: many nodes the radio has heard, for the map and the list of
//! nodes to show many. Made the same for the same number, so that what is
//! seen once is seen again.

use meshctl_fakes::now;
use meshctl_proto::meshtastic::{self as pb, config};

/// The most nodes a crowd has.
pub const MOST: u32 = 1_000;

/// The first crowd node's number: the rest follow it.
const FIRST: u32 = 0x0c00_0000;

/// Where the crowd is about: Madrid, where the repeater is.
const CENTRE: (f64, f64) = (40.42, -3.70);

/// How far from it, at most, in kilometres.
const SPREAD_KM: f64 = 40.0;

const NAMES: [&str; 12] = [
    "Ana", "Bea", "Carlos", "Dani", "Elena", "Fran", "Gloria", "Hugo", "Irene", "Javi", "Lucía",
    "Manu",
];

const PLACES: [&str; 8] = [
    "Base", "Coche", "Casa", "Bici", "Monte", "Oficina", "Furgo", "Torre",
];

const ROLES: [config::device_config::Role; 10] = [
    config::device_config::Role::Client,
    config::device_config::Role::Client,
    config::device_config::Role::Client,
    config::device_config::Role::ClientMute,
    config::device_config::Role::ClientBase,
    config::device_config::Role::Router,
    config::device_config::Role::RouterLate,
    config::device_config::Role::ClientHidden,
    config::device_config::Role::Tracker,
    config::device_config::Role::Sensor,
];

const MODELS: [pb::HardwareModel; 8] = [
    pb::HardwareModel::HeltecV3,
    pb::HardwareModel::Tbeam,
    pb::HardwareModel::Rak4631,
    pb::HardwareModel::TEcho,
    pb::HardwareModel::StationG2,
    pb::HardwareModel::HeltecWirelessTracker,
    pb::HardwareModel::TrackerT1000E,
    pb::HardwareModel::NanoG2Ultra,
];

/// A number made of a node's index and what it is for, always the same for
/// the same two: chance, but the same each time.
fn mix(index: u32, what: u32) -> u32 {
    let mut x = index.wrapping_mul(0x9e37_79b9) ^ what.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297a_2d39);
    x ^ (x >> 15)
}

/// A fraction from 0 to 1, as `mix`.
fn fraction(index: u32, what: u32) -> f64 {
    f64::from(mix(index, what)) / f64::from(u32::MAX)
}

/// How long ago a node was last heard, in seconds: most within the day,
/// some days or weeks ago, a few longer ago than a month.
fn age(index: u32) -> u32 {
    let within = |most: u32| mix(index, 2) % most.max(1);
    match mix(index, 1) % 100 {
        0..40 => within(3 * 3_600),
        40..70 => 3 * 3_600 + within(21 * 3_600),
        70..85 => 86_400 + within(6 * 86_400),
        85..95 => 7 * 86_400 + within(23 * 86_400),
        _ => 30 * 86_400 + within(30 * 86_400),
    }
}

/// Where a node is, in ten-millionths of a degree, if it says: one in ten
/// says nothing, and one in twelve is where the one before it is, for the
/// map to have nodes on the same spot.
fn place(index: u32) -> Option<(i32, i32)> {
    if mix(index, 3).is_multiple_of(10) {
        return None;
    }
    let at = if index > 0 && index.is_multiple_of(12) {
        index - 1
    } else {
        index
    };
    let angle = fraction(at, 4) * std::f64::consts::TAU;
    let km = fraction(at, 5).sqrt() * SPREAD_KM;
    let lat = CENTRE.0 + km * angle.sin() / 111.32;
    let lon = CENTRE.1 + km * angle.cos() / (111.32 * CENTRE.0.to_radians().cos());
    #[allow(clippy::cast_possible_truncation)]
    Some(((lat * 1e7).round() as i32, (lon * 1e7).round() as i32))
}

/// The crowd's nodes, `count` of them, up to `MOST`.
pub fn crowd(count: u32) -> Vec<pb::NodeInfo> {
    let heard_now = now();
    (0..count.min(MOST))
        .map(|i| {
            let num = FIRST + i;
            let name = NAMES[i as usize % NAMES.len()];
            let at = PLACES[(i as usize / NAMES.len()) % PLACES.len()];
            let role = ROLES[mix(i, 6) as usize % ROLES.len()];
            // Up to seven hops; one in ten not known, one in twenty by MQTT.
            let hops = match mix(i, 7) % 20 {
                0 | 1 => None,
                h => Some(h % 8),
            };
            let via_mqtt = mix(i, 8).is_multiple_of(20);
            let snr = if hops == Some(0) {
                f32::from(u8::try_from(mix(i, 9) % 40).unwrap_or(0)) / 4.0 - 5.0
            } else {
                0.0
            };
            pb::NodeInfo {
                num,
                user: Some(pb::User {
                    id: format!("!{num:08x}"),
                    long_name: format!("{name} {at} {i}"),
                    short_name: format!("C{:03}", i % 1_000),
                    hw_model: MODELS[mix(i, 10) as usize % MODELS.len()] as i32,
                    role: role as i32,
                    // Made up, as all of it: no private key goes with it.
                    public_key: (0..32u8).map(|b| b ^ (i as u8)).collect(),
                    ..Default::default()
                }),
                position: place(i).map(|(lat, lon)| pb::Position {
                    latitude_i: Some(lat),
                    longitude_i: Some(lon),
                    location_source: pb::position::LocSource::LocInternal as i32,
                    precision_bits: 32,
                    ..Default::default()
                }),
                snr,
                last_heard: heard_now.saturating_sub(age(i)),
                hops_away: hops,
                via_mqtt,
                ..Default::default()
            }
        })
        .collect()
}
