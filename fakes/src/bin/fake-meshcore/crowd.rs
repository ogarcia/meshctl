// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A crowd: many contacts the radio keeps, up to as many as it has room
//! for, for the list of nodes and the map to show many, and the radio to be
//! full. Made the same for the same number, so that what is seen once is
//! seen again.

use crate::radio::Contact;

/// Where the crowd is about: A Coruña, where the rest of its mesh is.
const CENTRE: (f64, f64) = (43.37, -8.40);

/// How far from it, at most, in kilometres.
const SPREAD_KM: f64 = 40.0;

const NAMES: [&str; 10] = [
    "Ana", "Brais", "Carme", "Duarte", "Iria", "Lois", "Uxía", "Xoán", "Sabela", "Antía",
];

const PLACES: [&str; 8] = [
    "Monte", "Porto", "Faro", "Ría", "Praia", "Castro", "Vila", "Agra",
];

/// A number made of a contact's index and what it is for, always the same
/// for the same two: chance, but the same each time.
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

/// How long ago a contact last advertised, in seconds: most within the
/// day, some days or weeks ago, a few longer ago than a month.
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

/// A key of its own for each, told apart from every other the radio has
/// by its first byte, and from the rest of the crowd by the next two:
/// meshctl numbers a MeshCore node by the first four bytes of its key.
fn key_of(index: u32) -> [u8; 32] {
    let mut key = [0u8; 32];
    key[0] = 0xcc;
    key[1..3].copy_from_slice(&u16::try_from(index).unwrap_or(u16::MAX).to_be_bytes());
    for (i, byte) in key.iter_mut().enumerate().skip(3) {
        *byte = (mix(index, 20 + u32::try_from(i).unwrap_or(0)) & 0xff) as u8;
    }
    key
}

/// The crowd's contacts, `count` of them, as heard by the radio's clock at
/// `at`: six in ten companions, a quarter repeaters, the rest sensors and
/// rooms; most where they say, one in ten nowhere, at any distance, a path
/// known to most.
pub fn crowd(count: u32, at: u32) -> Vec<Contact> {
    (0..count)
        .map(|i| {
            let kind = match mix(i, 6) % 20 {
                0..12 => 1,
                12..17 => 2,
                17..19 => 4,
                _ => 3,
            };
            let name = NAMES[i as usize % NAMES.len()];
            let place = PLACES[(i as usize / NAMES.len()) % PLACES.len()];
            let name = match kind {
                2 => format!("Repetidor {place} {i}"),
                3 => format!("Sala {place} {i}"),
                4 => format!("Sensor {place} {i}"),
                _ => format!("{name} {place} {i}"),
            };
            let (lat, lon) = if mix(i, 3).is_multiple_of(10) {
                (0, 0)
            } else {
                let angle = fraction(i, 4) * std::f64::consts::TAU;
                let km = fraction(i, 5).sqrt() * SPREAD_KM;
                let lat = CENTRE.0 + km * angle.sin() / 111.32;
                let lon = CENTRE.1 + km * angle.cos() / (111.32 * CENTRE.0.to_radians().cos());
                #[allow(clippy::cast_possible_truncation)]
                ((lat * 1e6).round() as i32, (lon * 1e6).round() as i32)
            };
            let heard = at.saturating_sub(age(i));
            Contact {
                key: key_of(i),
                kind,
                // No path known to one in five; up to three hops to the rest.
                path_len: i8::try_from(mix(i, 7) % 5).unwrap_or(0) - 1,
                name,
                last_advert: heard,
                lat,
                lon,
                // Added now, as the firmware marks it, whenever it was heard.
                last_modified: at,
            }
        })
        .collect()
}
