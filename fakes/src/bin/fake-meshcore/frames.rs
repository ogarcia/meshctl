// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The frames the radio sends its clients, made, and what is read from
//! theirs.

use crate::radio::{Contact, MAX_CHANNELS, MAX_CONTACTS, ME, Radio, key};
use meshctl_core::meshcore::raw;
use meshctl_fakes::{now, random};

/// A fixed field: the text, cut or padded with zeros to `len`.
pub fn field(text: &str, len: usize) -> Vec<u8> {
    let mut field = vec![0u8; len];
    let bytes = text.as_bytes();
    let n = bytes.len().min(len);
    field[..n].copy_from_slice(&bytes[..n]);
    field
}

pub fn self_info(radio: &Radio) -> Vec<u8> {
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

pub fn device_info(radio: &Radio) -> Vec<u8> {
    let mut frame = vec![
        0x0d,
        14,
        u8::try_from(MAX_CONTACTS / 2).unwrap_or(0),
        MAX_CHANNELS,
    ];
    // The BLE pin.
    frame.extend([0, 0, 0, 0]);
    frame.extend(field("23 Nov 2026", 12));
    // A model of those it could be, as the panel shows it.
    frame.extend(field("Heltec V4", 40));
    frame.extend(field("v1.18.0-fake", 20));
    // Repeat (v9+) and the path's hash size, less one (v10+).
    frame.extend([u8::from(radio.repeat), radio.path_hash_mode]);
    frame
}

/// An error, by the firmware's code: 1 unsupported, 2 not found, 6 an
/// illegal argument.
pub fn error(code: u8) -> Vec<Vec<u8>> {
    vec![vec![0x01, code]]
}

pub fn ok() -> Vec<Vec<u8>> {
    vec![vec![0x00]]
}

pub fn u32_in(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)?
        .try_into()
        .ok()
        .map(u32::from_le_bytes)
}

pub fn i32_in(data: &[u8], at: usize) -> Option<i32> {
    data.get(at..at + 4)?
        .try_into()
        .ok()
        .map(i32::from_le_bytes)
}

/// A contact, as listed (0x03) or as pushed when first heard (0x8a).
pub fn contact(code: u8, contact: &Contact) -> Vec<u8> {
    let mut frame = vec![code];
    frame.extend(contact.key);
    frame.extend([contact.kind, 0, contact.path_len.cast_unsigned()]);
    // Its path out, one byte a hop: through Torre last, Monte before it.
    let mut path = [0u8; 64];
    let hops = usize::try_from(contact.path_len).unwrap_or(0).min(64);
    for (i, hop) in path.iter_mut().take(hops).enumerate() {
        *hop = if i + 1 == hops {
            key(0x55)[0]
        } else {
            key(0x52)[0]
        };
    }
    frame.extend(path);
    frame.extend(field(&contact.name, 32));
    frame.extend(contact.last_advert.to_le_bytes());
    frame.extend(contact.lat.to_le_bytes());
    frame.extend(contact.lon.to_le_bytes());
    frame.extend(contact.last_modified.to_le_bytes());
    frame
}

pub fn channel_info(index: u8, name: &str, secret: &[u8; 16]) -> Vec<u8> {
    let mut frame = vec![0x12, index];
    frame.extend(field(name, 32));
    frame.extend(secret);
    frame
}

/// Bytes in hex, to say them.
pub fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A whole key, from a command's bytes.
pub fn key_in(data: &[u8]) -> Option<[u8; 32]> {
    data.get(..32)?.try_into().ok()
}

/// The first four bytes of a key, as a room's post names its author by.
pub fn key_prefix(key: &[u8; 32]) -> [u8; 4] {
    [key[0], key[1], key[2], key[3]]
}

/// A direct message from a contact, in the newer form, with its SNR.
pub fn direct_message(from: &Contact, text: &str) -> Vec<u8> {
    direct_message_at(from, now(), text)
}

pub fn direct_message_at(from: &Contact, at: u32, text: &str) -> Vec<u8> {
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
pub fn reaction_hash(text: &str, at: u32) -> String {
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
pub fn channel_message(slot: u8, at: u32, sender: &str, text: &str) -> Vec<u8> {
    let snr = i8::try_from(random(40)).unwrap_or(0) - 20;
    let mut frame = vec![0x11, snr.cast_unsigned(), 0, 0, slot, 2, 0];
    frame.extend(at.to_le_bytes());
    frame.extend(format!("{sender}: {text}").as_bytes());
    frame
}

/// A packet heard, raw, as the radio logs it: its SNR in quarters of a dB,
/// its RSSI, and the packet.
pub fn rx_log(snr: i8, rssi: i8, packet: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x88, (snr * 4).cast_unsigned(), rssi.cast_unsigned()];
    frame.extend(packet);
    frame
}

/// A flooded packet, its path the hashes of those it went through, two
/// bytes each; in a region by its key, if given.
pub fn flooded(
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
pub fn region_key(name: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("#{name}").as_bytes());
    digest[..16].try_into().unwrap_or_default()
}
