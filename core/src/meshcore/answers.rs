// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What MeshCore repeaters, rooms and sensors answer the requests of one
//! logged in to them, past the four bytes of the request's tag they start
//! with: as the firmware's `handleRequest` writes them.

/// What a repeater or a room says of how it is doing: `RepeaterStats` and
/// `ServerStats` in the firmware, alike up to the duplicates.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeStatus {
    pub battery_mv: u16,
    /// Packets waiting to go out.
    pub queue: u16,
    /// The noise it hears, in dBm.
    pub noise_floor: i16,
    pub last_rssi: i16,
    pub last_snr: f32,
    pub received: u32,
    pub sent: u32,
    pub sent_flood: u32,
    pub sent_direct: u32,
    pub received_flood: u32,
    pub received_direct: u32,
    pub flood_duplicates: u16,
    pub direct_duplicates: u16,
    /// Seconds it spent sending.
    pub airtime: u32,
    /// Seconds since it started.
    pub uptime: u32,
    /// Its error flags.
    pub errors: u16,
    /// A repeater's: seconds it spent receiving, and packets it could not
    /// read.
    pub receive_airtime: Option<u32>,
    pub receive_errors: Option<u32>,
    /// A room's: posts it took, and pushed to those logged in.
    pub posted: Option<u16>,
    pub pushed: Option<u16>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// A status answer: a repeater's, of 56 bytes, or a room's, of 52.
pub fn status(data: &[u8], room: bool) -> Option<NodeStatus> {
    Some(NodeStatus {
        battery_mv: u16_at(data, 0)?,
        queue: u16_at(data, 2)?,
        noise_floor: i16_at(data, 4)?,
        last_rssi: i16_at(data, 6)?,
        received: u32_at(data, 8)?,
        sent: u32_at(data, 12)?,
        airtime: u32_at(data, 16)?,
        uptime: u32_at(data, 20)?,
        sent_flood: u32_at(data, 24)?,
        sent_direct: u32_at(data, 28)?,
        received_flood: u32_at(data, 32)?,
        received_direct: u32_at(data, 36)?,
        errors: u16_at(data, 40)?,
        last_snr: f32::from(i16_at(data, 42)?) / 4.0,
        direct_duplicates: u16_at(data, 44)?,
        flood_duplicates: u16_at(data, 46)?,
        receive_airtime: (!room).then(|| u32_at(data, 48)).flatten(),
        receive_errors: (!room).then(|| u32_at(data, 52)).flatten(),
        posted: room.then(|| u16_at(data, 48)).flatten(),
        pushed: room.then(|| u16_at(data, 50)).flatten(),
    })
}

/// A reading of a sensor of a node, as Cayenne LPP gives it: on which
/// channel (1 its own, the rest its sensors), of which LPP type, and its
/// values: one, or three for an accelerometer, a gyrometer, a colour or a
/// place (latitude, longitude, altitude).
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub channel: u8,
    pub kind: u8,
    pub values: Vec<f64>,
}

/// The LPP types MeshCore's library writes, with how many bytes each value
/// takes, how many values, its divisor, and whether it is signed.
fn lpp_type(kind: u8) -> Option<(usize, usize, f64, bool)> {
    Some(match kind {
        0 | 1 | 102 | 142 => (1, 1, 1.0, false),
        2 | 3 => (2, 1, 100.0, true),
        100 | 118 | 133 => (4, 1, 1.0, false),
        101 | 125 | 128 | 132 => (2, 1, 1.0, false),
        103 => (2, 1, 10.0, true),
        104 => (1, 1, 2.0, false),
        113 => (2, 3, 1000.0, true),
        115 => (2, 1, 10.0, false),
        116 => (2, 1, 100.0, false),
        117 => (2, 1, 1000.0, false),
        120 => (1, 1, 1.0, false),
        121 => (2, 1, 1.0, true),
        130 | 131 => (4, 1, 1000.0, false),
        134 => (2, 3, 100.0, true),
        135 => (1, 3, 1.0, false),
        _ => return None,
    })
}

/// A big-endian integer of so many bytes, signed or not.
fn be(bytes: &[u8], signed: bool) -> f64 {
    let raw = bytes.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
    let bits = bytes.len() * 8;
    if signed && bits < 64 && raw >> (bits - 1) == 1 {
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_wrap)]
        return (raw as i64 - (1i64 << bits)) as f64;
    }
    #[allow(clippy::cast_precision_loss)]
    let value = raw as f64;
    value
}

/// A telemetry answer, in Cayenne LPP: each reading, up to the first of a
/// type not known, where what follows cannot be told apart.
pub fn telemetry(data: &[u8]) -> Vec<Reading> {
    let mut readings = Vec::new();
    let mut at = 0;
    while let (Some(&channel), Some(&kind)) = (data.get(at), data.get(at + 1)) {
        at += 2;
        let values = if kind == 136 {
            // A place: latitude and longitude in ten-thousandths of a
            // degree, altitude in hundredths of a metre, three bytes each.
            let Some(bytes) = data.get(at..at + 9) else {
                break;
            };
            at += 9;
            vec![
                be(&bytes[0..3], true) / 10_000.0,
                be(&bytes[3..6], true) / 10_000.0,
                be(&bytes[6..9], true) / 100.0,
            ]
        } else {
            let Some((size, count, divisor, signed)) = lpp_type(kind) else {
                break;
            };
            let Some(bytes) = data.get(at..at + size * count) else {
                break;
            };
            at += size * count;
            bytes
                .chunks(size)
                .map(|value| be(value, signed) / divisor)
                .collect()
        };
        readings.push(Reading {
            channel,
            kind,
            values,
        });
    }
    readings
}

/// A node another hears straight: by the first bytes of its key, how long
/// ago, and at what SNR.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbour {
    pub prefix: Vec<u8>,
    pub seconds_ago: u32,
    pub snr: f32,
}

/// A neighbours answer: how many the node hears in all, and those it told,
/// each by `prefix` bytes of its key.
pub fn neighbours(data: &[u8], prefix: usize) -> Option<(u16, Vec<Neighbour>)> {
    let total = u16_at(data, 0)?;
    let told = usize::from(u16_at(data, 2)?);
    let mut at = 4;
    let mut heard = Vec::with_capacity(told);
    for _ in 0..told {
        let key = data.get(at..at + prefix)?.to_vec();
        let seconds_ago = u32_at(data, at + prefix)?;
        let snr = f32::from(data.get(at + prefix + 4)?.cast_signed()) / 4.0;
        at += prefix + 5;
        heard.push(Neighbour {
            prefix: key,
            seconds_ago,
            snr,
        });
    }
    Some((total, heard))
}

/// Who runs a node, as it says: its firmware, its name and what its owner
/// wrote of themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub firmware: String,
    pub name: String,
    pub owner: String,
}

/// An owner answer: three lines.
pub fn owner(data: &[u8]) -> Option<Owner> {
    let text = String::from_utf8_lossy(data);
    let text = text.trim_end_matches('\0');
    let mut lines = text.splitn(3, '\n');
    Some(Owner {
        firmware: lines.next()?.to_owned(),
        name: lines.next().unwrap_or_default().to_owned(),
        owner: lines.next().unwrap_or_default().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeaters_status_and_a_rooms() {
        let mut data = Vec::new();
        data.extend(4123u16.to_le_bytes());
        data.extend(2u16.to_le_bytes());
        data.extend((-118i16).to_le_bytes());
        data.extend((-92i16).to_le_bytes());
        for n in [1000u32, 800, 120, 86_400, 300, 500, 400, 600] {
            data.extend(n.to_le_bytes());
        }
        data.extend(0u16.to_le_bytes());
        data.extend(26i16.to_le_bytes());
        data.extend(3u16.to_le_bytes());
        data.extend(40u16.to_le_bytes());
        let mut repeater = data.clone();
        repeater.extend(900u32.to_le_bytes());
        repeater.extend(7u32.to_le_bytes());
        let status = status(&repeater, false).unwrap();
        assert_eq!(status.battery_mv, 4123);
        assert_eq!(status.noise_floor, -118);
        assert!((status.last_snr - 6.5).abs() < f32::EPSILON);
        assert_eq!((status.uptime, status.flood_duplicates), (86_400, 40));
        assert_eq!(
            (status.receive_airtime, status.receive_errors),
            (Some(900), Some(7))
        );
        assert_eq!(status.posted, None);

        let mut room = data;
        room.extend(12u16.to_le_bytes());
        room.extend(30u16.to_le_bytes());
        let status = super::status(&room, true).unwrap();
        assert_eq!((status.posted, status.pushed), (Some(12), Some(30)));
        assert_eq!(status.receive_airtime, None);
        assert_eq!(super::status(&room[..40], true), None);
    }

    #[test]
    fn telemetry_reads_as_cayenne_lpp() {
        // 4.12 V and 21.5 °C of its own; -3.5 °C and a place of a sensor's;
        // then a type not known, where reading stops.
        let data = [
            1, 116, 0x01, 0x9c, 1, 103, 0x00, 0xd7, 2, 103, 0xff, 0xdd, 2, 136, 0x06, 0x9d, 0xdc,
            0xfe, 0xb7, 0x30, 0x00, 0x03, 0xe8, 3, 250, 1, 2,
        ];
        let readings = telemetry(&data);
        assert_eq!(readings.len(), 4);
        assert_eq!(
            readings[0],
            Reading {
                channel: 1,
                kind: 116,
                values: vec![4.12]
            }
        );
        assert_eq!(readings[1].values, vec![21.5]);
        assert_eq!(readings[2].values, vec![-3.5]);
        let place = &readings[3].values;
        assert!((place[0] - 43.3628).abs() < 1e-9);
        assert!((place[1] - -8.4176).abs() < 1e-9);
        assert!((place[2] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn neighbours_by_the_start_of_their_keys() {
        let mut data = Vec::new();
        data.extend(5u16.to_le_bytes());
        data.extend(2u16.to_le_bytes());
        data.extend([0x1e, 0x35, 0xaa, 0x01]);
        data.extend(60u32.to_le_bytes());
        data.push(30);
        data.extend([0x52, 0x59, 0x60, 0x67]);
        data.extend(3600u32.to_le_bytes());
        data.push((-10i8).cast_unsigned());
        let (total, heard) = neighbours(&data, 4).unwrap();
        assert_eq!(total, 5);
        assert_eq!(heard[0].prefix, vec![0x1e, 0x35, 0xaa, 0x01]);
        assert_eq!((heard[1].seconds_ago, heard[1].snr), (3600, -2.5));
        assert_eq!(neighbours(&data[..10], 4), None);
    }

    #[test]
    fn an_owner_in_three_lines() {
        assert_eq!(
            owner(b"v1.18.0\nStaLocaia\nAlguien, Vigo"),
            Some(Owner {
                firmware: "v1.18.0".into(),
                name: "StaLocaia".into(),
                owner: "Alguien, Vigo".into(),
            })
        );
        assert_eq!(owner(b"v1.18.0\nTorre\n").unwrap().owner, "");
    }
}
