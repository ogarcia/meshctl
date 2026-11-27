// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The other nodes of its mesh: Ana, a contact; Sala Coruña, a room; and
//! the repeaters: their consoles, their status and what they hear.

use crate::radio::{Contact, Event, ME, Radio};
use meshctl_fakes::{now, random};
use std::time::Duration;
use tokio::sync::broadcast;

/// A room's post, as the radio gives it: from the room, signed, with the
/// first four bytes of its author's key before its text, and the room's
/// clock when it was posted.
pub fn room_post(room: &Contact, author: [u8; 4], at: u32, text: &str) -> Vec<u8> {
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
pub fn room_sync(radio: &mut Radio) -> usize {
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

/// A command for the console of one of the radio's repeaters: answered, to
/// its admin only, after a moment, as a message of the console's kind.
pub fn console(radio: &mut Radio, data: &[u8], events: &broadcast::Sender<Event>) -> Vec<Vec<u8>> {
    let (Some(prefix), Some(text)) = (data.get(6..12), data.get(12..)) else {
        return vec![vec![0x01, 6]];
    };
    let Some(to) = radio
        .contacts
        .iter()
        .find(|c| &c.key[..6] == prefix)
        .cloned()
    else {
        return vec![vec![0x01, 2]];
    };
    let command = String::from_utf8_lossy(text).trim().to_owned();
    let admin = radio.logins.get(&to.name) == Some(&3);
    println!(
        "console of {:?}: {command:?}, {}",
        to.name,
        if admin { "its admin" } else { "not its admin" }
    );
    let mut sent = vec![0x06, 0];
    sent.extend([0; 4]);
    sent.extend(4_000u32.to_le_bytes());
    if !admin {
        return vec![sent];
    }
    let mut words = command.split_whitespace();
    let answer = match (words.next(), words.next()) {
        (Some("ver"), _) => "v1.18.0-fake (Build: 24-Nov-2026)".to_owned(),
        (Some("clock"), None) => {
            let clock = radio.clock();
            format!(
                "{:02}:{:02} - fake clock UTC",
                clock / 3600 % 24,
                clock / 60 % 60
            )
        }
        (Some("get"), Some("name")) => to.name.clone(),
        (Some("get"), Some("radio")) => "869.618,62.5,8,8".to_owned(),
        (Some("get"), Some(other)) => format!("??: {other}"),
        (Some("set"), Some(_)) => "OK".to_owned(),
        (Some("neighbors"), _) => radio
            .contacts
            .iter()
            .filter(|c| c.kind == 2 && c.key != to.key)
            .map(|c| {
                format!(
                    "{:02X}{:02X}{:02X}{:02X}:120:28",
                    c.key[0], c.key[1], c.key[2], c.key[3]
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        (Some("advert"), _) => "OK - Advert sent".to_owned(),
        (Some("reboot"), _) => "OK - rebooting".to_owned(),
        (Some("password"), Some(new)) => format!("password now: {new}"),
        _ => "Unknown command".to_owned(),
    };
    let snr = i8::try_from(20 + random(20)).unwrap_or(0);
    let mut frame = vec![0x10, snr.cast_unsigned(), 0, 0];
    frame.extend(&to.key[..6]);
    frame.extend([to.path_len.cast_unsigned(), 1]);
    frame.extend(now().to_le_bytes());
    frame.extend(answer.as_bytes());
    radio.queued.push_back(frame);
    let events = events.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800 + random(800))).await;
        let _ = events.send(Event::Push(vec![0x83]));
    });
    vec![sent]
}

/// How a repeater or a room is doing, as the firmware's `RepeaterStats` or
/// `ServerStats` lay it out: figures that change a little each time.
pub fn node_status(node: &Contact, room: bool) -> Vec<u8> {
    let up = u32::try_from(random(400_000)).unwrap_or(0) + 86_400;
    let received = 20_000 + u32::try_from(random(5_000)).unwrap_or(0);
    let mut data = Vec::new();
    data.extend((3_900 + u16::try_from(random(300)).unwrap_or(0)).to_le_bytes());
    data.extend(u16::try_from(random(3)).unwrap_or(0).to_le_bytes());
    data.extend((-118i16 + i16::try_from(random(6)).unwrap_or(0)).to_le_bytes());
    data.extend((-80i16 - i16::try_from(random(30)).unwrap_or(0)).to_le_bytes());
    data.extend(received.to_le_bytes());
    data.extend((received / 2).to_le_bytes());
    data.extend((up / 90).to_le_bytes());
    data.extend(up.to_le_bytes());
    data.extend((received / 3).to_le_bytes());
    data.extend((received / 6).to_le_bytes());
    data.extend((received / 2).to_le_bytes());
    data.extend((received / 4).to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend((24i16 + i16::try_from(random(16)).unwrap_or(0)).to_le_bytes());
    data.extend(12u16.to_le_bytes());
    data.extend(u16::try_from(received / 10).unwrap_or(0).to_le_bytes());
    if room {
        data.extend(42u16.to_le_bytes());
        data.extend(97u16.to_le_bytes());
    } else {
        data.extend((up / 40).to_le_bytes());
        data.extend(u32::try_from(node.name.len()).unwrap_or(0).to_le_bytes());
    }
    data
}

/// What a node's sensors read, in Cayenne LPP: its voltage and temperature,
/// and to its admin a sensor's humidity and pressure as well.
pub fn node_telemetry(admin: bool) -> Vec<u8> {
    let volts = 395 + u16::try_from(random(20)).unwrap_or(0);
    let tenths = 180 + i16::try_from(random(60)).unwrap_or(0);
    let mut data = vec![1, 116];
    data.extend(volts.to_be_bytes());
    data.extend([1, 103]);
    data.extend(tenths.to_be_bytes());
    if admin {
        data.extend([2, 104, 130]);
        data.extend([2, 115]);
        data.extend(10_132u16.to_be_bytes());
    }
    data
}

/// What a sensor kept over the last `seconds`: its clock, then its battery
/// and a sensor's temperature, each its lowest, highest and mean, wider
/// apart the longer back.
pub fn node_series(clock: u32, seconds: u32) -> Vec<u8> {
    let spread = u16::try_from((seconds / 3600).clamp(1, 168)).unwrap_or(1);
    let mut data = clock.to_le_bytes().to_vec();
    let volts = 400u16;
    data.extend([1, 116]);
    for value in [
        volts - spread.min(30),
        volts + spread.min(12),
        volts - spread.min(30) / 3,
    ] {
        data.extend(value.to_be_bytes());
    }
    let tenths = 160i16;
    let swing = i16::try_from(spread.min(120)).unwrap_or(0);
    data.extend([2, 103]);
    for value in [tenths - swing, tenths + swing, tenths + swing / 4] {
        data.extend(value.to_be_bytes());
    }
    data
}

/// The nodes a repeater or a room hears straight: the radio's other
/// contacts but itself, each by the first four bytes of its key, the
/// strongest first.
pub fn node_neighbours(radio: &Radio, to: &Contact) -> Vec<u8> {
    let heard: Vec<&Contact> = radio
        .contacts
        .iter()
        .filter(|c| c.key != to.key && c.kind == 2)
        .collect();
    let count = u16::try_from(heard.len() + 1).unwrap_or(0);
    let mut data = Vec::new();
    data.extend(count.to_le_bytes());
    data.extend(count.to_le_bytes());
    for (i, contact) in heard.iter().enumerate() {
        data.extend(&contact.key[..4]);
        data.extend((u32::try_from(i).unwrap_or(0) * 900 + 30).to_le_bytes());
        data.push(i8::try_from(28 - 8 * i).unwrap_or(0).cast_unsigned());
    }
    // And the radio itself, as one it hears.
    data.extend(&ME[..4]);
    data.extend(120u32.to_le_bytes());
    data.push(18);
    data
}
