// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! MeshCore packets as heard, raw, as a companion radio logs each it hears,
//! every copy: their route, path and region code, and a channel message's
//! text, opened with its channel's secret. As the firmware's `Packet` lays
//! them out and `Utils` seals them.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Flooded, with region codes.
pub const ROUTE_TRANSPORT_FLOOD: u8 = 0;
/// Flooded, its path built up as it goes.
pub const ROUTE_FLOOD: u8 = 1;
/// Sent along a path given.
pub const ROUTE_DIRECT: u8 = 2;
/// Sent along a path given, with region codes.
pub const ROUTE_TRANSPORT_DIRECT: u8 = 3;

/// A text to a node: the hashes of whom it is to and from, then sealed.
pub const PAYLOAD_TXT_MSG: u8 = 0x02;
/// A text to a channel: its hash, then sealed.
pub const PAYLOAD_GRP_TXT: u8 = 0x05;

/// Bytes of the MAC that seals a payload.
const MAC_SIZE: usize = 2;

/// A packet as heard: how it was sent, its region codes, its path and what
/// it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPacket {
    pub route: u8,
    pub payload_type: u8,
    /// The region codes, on those sent with them: the first is the region's.
    pub transport: Option<[u16; 2]>,
    /// Its path length byte: hops, and each hash's size in its top bits.
    pub path_len: u8,
    /// The hashes of the repeaters it went through, as heard: for a flood,
    /// from the one nearest its sender on.
    pub path: Vec<u8>,
    pub payload: Vec<u8>,
}

impl RawPacket {
    /// Reads a packet as the radio logs it; none if it is not one.
    pub fn parse(raw: &[u8]) -> Option<Self> {
        let (&header, mut rest) = raw.split_first()?;
        let route = header & 0x03;
        let payload_type = (header >> 2) & 0x0f;
        let transport = if matches!(route, ROUTE_TRANSPORT_FLOOD | ROUTE_TRANSPORT_DIRECT) {
            let codes = rest.get(..4)?;
            rest = &rest[4..];
            Some([
                u16::from_le_bytes([codes[0], codes[1]]),
                u16::from_le_bytes([codes[2], codes[3]]),
            ])
        } else {
            None
        };
        let (&path_len, rest) = rest.split_first()?;
        let size = usize::from(path_len >> 6) + 1;
        if size == 4 {
            return None;
        }
        let bytes = usize::from(path_len & 0x3f) * size;
        let path = rest.get(..bytes)?.to_vec();
        let payload = rest.get(bytes..)?.to_vec();
        if payload.is_empty() {
            return None;
        }
        Some(Self {
            route,
            payload_type,
            transport,
            path_len,
            path,
            payload,
        })
    }

    /// Whether it was flooded, its path built up by those it went through.
    pub fn flooded(&self) -> bool {
        matches!(self.route, ROUTE_FLOOD | ROUTE_TRANSPORT_FLOOD)
    }

    /// Each repeater's hash on its path.
    pub fn hops(&self) -> Vec<Vec<u8>> {
        let size = usize::from(self.path_len >> 6) + 1;
        self.path.chunks(size).map(<[u8]>::to_vec).collect()
    }

    /// What names the packet whatever path a copy took: its type and what it
    /// carries, hashed, as the firmware tells copies apart.
    pub fn key(&self) -> u64 {
        let digest = Sha256::new()
            .chain_update([self.payload_type])
            .chain_update(&self.payload)
            .finalize();
        u64::from_le_bytes(digest[..8].try_into().unwrap_or_default())
    }

    /// Whether its region code is the one a region's key gives it.
    pub fn in_region(&self, key: &[u8; 16]) -> bool {
        self.transport
            .is_some_and(|codes| codes[0] == transport_code(key, self.payload_type, &self.payload))
    }
}

/// The region code a packet goes with, by its region's key: the start of
/// the HMAC of its type and payload; 0000 and FFFF kept apart.
pub fn transport_code(key: &[u8; 16], payload_type: u8, payload: &[u8]) -> u16 {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    mac.update(&[payload_type]);
    mac.update(payload);
    let out = mac.finalize().into_bytes();
    match u16::from_le_bytes([out[0], out[1]]) {
        0 => 1,
        0xffff => 0xfffe,
        code => code,
    }
}

/// The byte a channel goes by in what is sent to it: the start of its
/// secret's SHA-256.
pub fn channel_hash(secret: &[u8; 16]) -> u8 {
    Sha256::digest(secret)[0]
}

/// A channel's text, opened with its secret: its sender's clock and the
/// text, as `name: text`. None if the secret is not the one it was sealed
/// with.
pub fn open_channel_text(secret: &[u8; 16], payload: &[u8]) -> Option<(u32, String)> {
    let (&hash, sealed) = payload.split_first()?;
    if hash != channel_hash(secret) || sealed.len() <= MAC_SIZE {
        return None;
    }
    let (mac, encrypted) = sealed.split_at(MAC_SIZE);
    if !encrypted.len().is_multiple_of(16) {
        return None;
    }
    // The MAC's key is the secret as 32 bytes, its second half zeros.
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(secret);
    let mut check = <Hmac<Sha256> as Mac>::new_from_slice(&key).expect("any key length");
    check.update(encrypted);
    if check.finalize().into_bytes()[..MAC_SIZE] != *mac {
        return None;
    }
    let cipher = Aes128::new(GenericArray::from_slice(secret));
    let mut plain = encrypted.to_vec();
    for block in plain.chunks_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    let timestamp = u32::from_le_bytes(plain.get(..4)?.try_into().ok()?);
    // Plain text only: the top bits of its type byte say other kinds.
    if plain.get(4)? >> 2 != 0 {
        return None;
    }
    let text = plain.get(5..)?;
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    Some((
        timestamp,
        String::from_utf8_lossy(&text[..end]).into_owned(),
    ))
}

/// A channel's text sealed with its secret, as the firmware seals it: what
/// a channel message carries.
pub fn seal_channel_text(secret: &[u8; 16], timestamp: u32, text: &str) -> Vec<u8> {
    use aes::cipher::BlockEncrypt;
    let mut plain = timestamp.to_le_bytes().to_vec();
    plain.push(0);
    plain.extend(text.as_bytes());
    while !plain.len().is_multiple_of(16) {
        plain.push(0);
    }
    let cipher = Aes128::new(GenericArray::from_slice(secret));
    for block in plain.chunks_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(secret);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).expect("any key length");
    mac.update(&plain);
    let mut payload = vec![channel_hash(secret)];
    payload.extend(&mac.finalize().into_bytes()[..MAC_SIZE]);
    payload.extend(plain);
    payload
}

/// What each kind of payload is called, as the firmware's `Packet` names it.
pub fn payload_name(payload_type: u8) -> &'static str {
    match payload_type {
        0x00 => "REQ",
        0x01 => "RESPONSE",
        0x02 => "TXT_MSG",
        0x03 => "ACK",
        0x04 => "ADVERT",
        0x05 => "GRP_TXT",
        0x06 => "GRP_DATA",
        0x07 => "ANON_REQ",
        0x08 => "PATH",
        0x09 => "TRACE",
        0x0a => "MULTIPART",
        0x0b => "CONTROL",
        0x0f => "RAW_CUSTOM",
        _ => "UNKNOWN",
    }
}

/// An advert to its key, its node said what it is, its name and where.
pub const PAYLOAD_ADVERT: u8 = 0x04;
/// An acknowledgement: its four bytes of checksum.
pub const PAYLOAD_ACK: u8 = 0x03;
/// A request with no login: whom it is to, by its hash, then the key of
/// whom it is from, then sealed.
pub const PAYLOAD_ANON_REQ: u8 = 0x07;
/// A trace along a path: its tag, auth code and flags, then the hashes of
/// the path; the SNR each hop heard it at goes in its path as it goes.
pub const PAYLOAD_TRACE: u8 = 0x09;

/// The bytes of a signature, as an advert carries one.
const SIGNATURE_SIZE: usize = 64;

/// A node's advert, as heard: its key, its clock as it sent it, what kind
/// of node it is (1 a companion, 2 a repeater, 3 a room, 4 a sensor), and
/// its name and place when it says them.
#[derive(Debug, Clone, PartialEq)]
pub struct Advert {
    pub key: [u8; 32],
    pub timestamp: u32,
    pub kind: u8,
    pub name: Option<String>,
    /// Latitude and longitude, in degrees.
    pub position: Option<(f64, f64)>,
}

/// An advert's payload, as `AdvertDataParser` reads what follows its key,
/// clock and signature: its flags, its place if they say, two words of
/// features if they say, and its name if they say.
pub fn advert(payload: &[u8]) -> Option<Advert> {
    let key: [u8; 32] = payload.get(..32)?.try_into().ok()?;
    let timestamp = u32::from_le_bytes(payload.get(32..36)?.try_into().ok()?);
    let app = payload.get(36 + SIGNATURE_SIZE..)?;
    let (&flags, mut rest) = app.split_first()?;
    let mut position = None;
    if flags & 0x10 != 0 {
        let lat = i32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
        let lon = i32::from_le_bytes(rest.get(4..8)?.try_into().ok()?);
        position = Some((f64::from(lat) / 1e6, f64::from(lon) / 1e6));
        rest = &rest[8..];
    }
    for feature in [0x20, 0x40] {
        if flags & feature != 0 {
            rest = rest.get(2..)?;
        }
    }
    let name = (flags & 0x80 != 0)
        .then(|| {
            String::from_utf8_lossy(rest)
                .trim_end_matches('\0')
                .to_owned()
        })
        .filter(|name| !name.is_empty());
    Some(Advert {
        key,
        timestamp,
        kind: flags & 0x0f,
        name,
        position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed(secret: &[u8; 16], timestamp: u32, text: &str) -> Vec<u8> {
        seal_channel_text(secret, timestamp, text)
    }

    #[test]
    fn a_flooded_packet_reads_with_its_path() {
        // Flooded group text, two hops of two-byte hashes.
        let mut raw = vec![
            (PAYLOAD_GRP_TXT << 2) | ROUTE_FLOOD,
            0x42,
            0xaa,
            0xbb,
            0xcc,
            0xdd,
        ];
        raw.extend([1, 2, 3]);
        let packet = RawPacket::parse(&raw).unwrap();
        assert!(packet.flooded());
        assert_eq!(packet.transport, None);
        assert_eq!(packet.hops(), [vec![0xaa, 0xbb], vec![0xcc, 0xdd]]);
        assert_eq!(packet.payload, [1, 2, 3]);
        // Another copy, another path, the same packet.
        let mut other = vec![(PAYLOAD_GRP_TXT << 2) | ROUTE_FLOOD, 0x01, 0x77];
        other.extend([1, 2, 3]);
        assert_eq!(RawPacket::parse(&other).unwrap().key(), packet.key());
        // Too short for its path: nothing.
        assert_eq!(RawPacket::parse(&[0x05, 0x03, 1]), None);
    }

    #[test]
    fn a_region_is_told_by_its_code() {
        let key = [7u8; 16];
        let payload = vec![9, 9, 9];
        let code = transport_code(&key, PAYLOAD_GRP_TXT, &payload);
        let mut raw = vec![(PAYLOAD_GRP_TXT << 2) | ROUTE_TRANSPORT_FLOOD];
        raw.extend(code.to_le_bytes());
        raw.extend([0, 0, 0x00]);
        raw.extend(&payload);
        let packet = RawPacket::parse(&raw).unwrap();
        assert!(packet.flooded());
        assert!(packet.in_region(&key));
        assert!(!packet.in_region(&[8; 16]));
    }

    #[test]
    fn a_channel_text_opens_with_its_secret_alone() {
        let secret = [0x11u8; 16];
        let payload = sealed(&secret, 1_795_000_000, "Ana: hola a todos");
        assert_eq!(
            open_channel_text(&secret, &payload),
            Some((1_795_000_000, "Ana: hola a todos".to_owned()))
        );
        assert_eq!(open_channel_text(&[0x12; 16], &payload), None);
        // Tampered with: its MAC says so.
        let mut bad = payload.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert_eq!(open_channel_text(&secret, &bad), None);
    }

    #[test]
    fn an_advert_reads_with_its_name_and_place() {
        let mut payload = vec![0x52; 32];
        payload.extend(1_795_519_657u32.to_le_bytes());
        payload.extend([0; SIGNATURE_SIZE]);
        // A repeater, with a place, a feature of two bytes and a name.
        payload.push(0x80 | 0x20 | 0x10 | 2);
        payload.extend(43_370_000i32.to_le_bytes());
        payload.extend((-8_400_000i32).to_le_bytes());
        payload.extend([9, 9]);
        payload.extend(b"Torre\0");
        let advert = advert(&payload).unwrap();
        assert_eq!(advert.key, [0x52; 32]);
        assert_eq!(advert.timestamp, 1_795_519_657);
        assert_eq!(advert.kind, 2);
        assert_eq!(advert.name.as_deref(), Some("Torre"));
        assert_eq!(advert.position, Some((43.37, -8.4)));
        // Cut short before its flags, it is no advert.
        assert!(super::advert(&payload[..36 + SIGNATURE_SIZE]).is_none());
    }
}
