// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the radio answers each command a client sends it, as the firmware
//! does.

use crate::control::busy_secs;
use crate::frames::{
    channel_info, contact, device_info, direct_message, error, field, flooded, i32_in, key_in,
    key_prefix, ok, rx_log, self_info, u32_in,
};
use crate::nodes::{console, node_neighbours, node_series, node_status, node_telemetry, room_sync};
use crate::radio::{
    Contact, Event, GPS_AT, MAX_CONTACTS, ME, REPEAT_FREQUENCIES, Radio, SendScope, key,
};
use meshctl_core::meshcore::raw;
use meshctl_fakes::{now, random};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::broadcast;

/// What the radio answers a command, and what it pushes after a while.
pub fn answer(
    command: &[u8],
    radio: &Mutex<Radio>,
    events: &broadcast::Sender<Event>,
) -> Vec<Vec<u8>> {
    let Some((&code, data)) = command.split_first() else {
        return Vec::new();
    };
    let mut radio = radio.lock().unwrap_or_else(|e| e.into_inner());
    match code {
        // APP_START
        0x01 => vec![self_info(&radio)],
        // SEND_TEXT: to the contact its key prefix names.
        0x02 => {
            println!(
                "direct text, {}: {:?}",
                radio.scope_words(),
                data.get(12..)
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default()
            );
            // A command for a node's console: answered to its admin only, as
            // the firmware does, and acknowledged never.
            if data.first() == Some(&1) {
                return console(&mut radio, data, events);
            }
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
            // Delivered after a moment, and now and then answered; with
            // answers off, never acknowledged.
            if !radio.answers {
                return vec![sent];
            }
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
            println!("login to {}", to.name);
            let tag: [u8; 4] = key_prefix(&key);
            let mut sent = vec![0x06, u8::from(to.path_len < 0)];
            sent.extend(tag);
            sent.extend(4_000u32.to_le_bytes());
            let room = to.name == "Sala Coruña";
            // A repeater or a sensor takes `admin` as its admin's password,
            // and no password as a guest's, as one with no guest password set
            // does; to any other it says nothing.
            let repeater = matches!(to.kind, 2 | 4);
            let acl = match password.as_str() {
                "admin" if room || repeater => Some(3),
                "sala" if room => Some(2),
                _ if room && !radio.room_strict => Some(0),
                "" if repeater => Some(0),
                _ if room || repeater => None,
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
                    let waiting = if room {
                        radio.room_access = Some(acl);
                        room_sync(&mut radio)
                    } else {
                        radio.logins.insert(to.name.clone(), acl);
                        0
                    };
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
            let owner = data.get(32) == Some(&0x02);
            println!(
                "anonymous request {:?} to {:?}, {}",
                data.get(32),
                to.name,
                if flood { "flooded" } else { "direct" }
            );
            if owner && !flood && matches!(to.kind, 2 | 3) {
                let events = events.clone();
                let said = format!("{}\nmeshctl, A Coruña", to.name);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(600 + random(900))).await;
                    let mut answer = vec![0x8c, 0];
                    answer.extend(tag);
                    answer.extend(now().to_le_bytes());
                    answer.extend(said.as_bytes());
                    let _ = events.send(Event::Push(answer));
                });
            }
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
        // RUN_CLI_COMMAND: its own console, answered at once with what it
        // says, as firmware 14 does.
        0x42 => {
            let command = String::from_utf8_lossy(data).trim().to_owned();
            println!("own console: {command:?}");
            let mut words = command.split_whitespace();
            let answer = match (words.next(), words.next()) {
                (Some("ver"), _) => "v1.18.0-fake (Build: 24-Nov-2026)".to_owned(),
                (Some("get"), Some("name")) => format!("> {}", radio.name),
                (Some("get"), Some("radio")) => format!(
                    "> {}.{:03},{},{},{}",
                    radio.frequency / 1000,
                    radio.frequency % 1000,
                    f64::from(radio.bandwidth) / 1000.0,
                    radio.spreading_factor,
                    radio.coding_rate
                ),
                (Some("get"), Some("tx")) => format!("> {}", radio.tx_power),
                (Some("get"), Some(other)) => format!("??: {other}"),
                _ => "Unknown command".to_owned(),
            };
            let mut reply = vec![0x1d];
            reply.extend(answer.as_bytes());
            vec![reply]
        }
        // SEND_BINARY_REQ: a key, then a request to a node logged in to.
        // Its repeaters and its room answer as the firmware does, as far as
        // the login lets: nothing to one not logged in, the access list
        // only to its admin, a guest only its own telemetry.
        0x32 => {
            let Some(key) = key_in(data) else {
                return error(6);
            };
            let Some(to) = radio.contacts.iter().find(|c| c.key == key).cloned() else {
                return error(2);
            };
            let tag: [u8; 4] = u32::try_from(random(u64::from(u32::MAX)))
                .unwrap_or(1)
                .to_le_bytes();
            let mut sent = vec![0x06, u8::from(to.path_len < 0)];
            sent.extend(tag);
            sent.extend(3_000u32.to_le_bytes());
            let acl = if to.name == "Sala Coruña" {
                radio.room_access
            } else {
                radio.logins.get(&to.name).copied()
            };
            let what = data.get(32).copied().unwrap_or(0);
            println!("request {what:#04x} to {:?}, logged in as {acl:?}", to.name);
            let answer = match (acl.filter(|_| radio.answers), what) {
                (None, _) => None,
                (Some(_), 0x01) => Some(node_status(&to, to.kind == 3)),
                (Some(acl), 0x03) => Some(node_telemetry(acl == 3)),
                (Some(_), 0x06) => Some(node_neighbours(&radio, &to)),
                // A sensor's kept readings, over the seconds asked.
                (Some(_), 0x04) if to.kind == 4 => {
                    let seconds = u32_in(data, 33).unwrap_or(86_400);
                    Some(node_series(radio.clock(), seconds))
                }
                (Some(_), 0x07) => {
                    Some(format!("v1.18.0-fake\n{}\nmeshctl, A Coruña", to.name).into_bytes())
                }
                (Some(3), 0x05) => {
                    let mut list = Vec::new();
                    list.extend(&ME[..6]);
                    list.push(3);
                    list.extend(&self::key(0x41)[..6]);
                    list.push(0);
                    Some(list)
                }
                _ => None,
            };
            if let Some(answer) = answer {
                let events = events.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(700 + random(1_000))).await;
                    let mut push = vec![0x8c, 0];
                    push.extend(tag);
                    push.extend(answer);
                    let _ = events.send(Event::Push(push));
                });
            }
            vec![sent]
        }
        // SEND_TRACE_PATH: a tag, an auth code, flags and a path of hashes.
        // A contact heard straight whose hash is the path passes it back
        // with the SNR it heard the radio at; anything else, nothing.
        // SEND_PATH_DISCOVERY_REQ: flooded; the way found comes back after
        // a while, there through Monte and Torre, back through Torre, by two
        // bytes of each; none comes when answers are off.
        0x34 => {
            let Some(key) = data.get(1..).and_then(key_in) else {
                return error(6);
            };
            if !radio.contacts.iter().any(|c| c.key == key) {
                return error(2);
            }
            let tag: [u8; 4] = u32::try_from(random(u64::from(u32::MAX)))
                .unwrap_or(1)
                .to_le_bytes();
            let mut sent = vec![0x06, 1];
            sent.extend(tag);
            sent.extend(4_000u32.to_le_bytes());
            println!("path discovery to {:02x?}", &key[..4]);
            if radio.answers {
                let events = events.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1_500 + random(1_000))).await;
                    let mut push = vec![0x8d, 0];
                    push.extend(&key[..6]);
                    push.push(0x42);
                    push.extend(&self::key(0x52)[..2]);
                    push.extend(&self::key(0x55)[..2]);
                    push.push(0x41);
                    push.extend(&self::key(0x55)[..2]);
                    let _ = events.send(Event::Push(push));
                });
            }
            vec![sent]
        }
        0x24 => {
            let (Some(tag), Some(&flags)) = (data.get(..4), data.get(8)) else {
                return error(6);
            };
            let tag: [u8; 4] = tag.try_into().unwrap_or_default();
            let path = data.get(9..).unwrap_or_default().to_vec();
            let size = 1usize << (flags & 0x03);
            let mut sent = vec![0x06, 0];
            sent.extend(tag);
            sent.extend(1_500u32.to_le_bytes());
            // A ping to one heard straight, or a traceroute there and back
            // through relays: the hops odd, as long the one way as the other.
            let hops = path.len() / size;
            let pinged = path.len() == size
                && radio
                    .contacts
                    .iter()
                    .any(|c| c.path_len == 0 && c.key[..size] == path[..]);
            let back = radio.answers && (pinged || hops >= 3 && hops % 2 == 1);
            println!(
                "trace along {path:02x?}, {}",
                if back { "back" } else { "lost" }
            );
            if back {
                let events = events.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300 + random(600))).await;
                    // Each hop's SNR in quarters of a dB, from -10 to 10 or so,
                    // and the radio's of the last.
                    let snr = || (i8::try_from(random(80)).unwrap_or(0) - 40).cast_unsigned();
                    let mut push = vec![0x89, 0, u8::try_from(path.len()).unwrap_or(0), flags];
                    push.extend(tag);
                    push.extend([0; 4]);
                    push.extend(&path);
                    push.extend((0..hops).map(|_| snr()));
                    push.push(snr());
                    let _ = events.send(Event::Push(push));
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
            println!(
                "channel text on slot {slot}, {}: {:?}",
                radio.scope_words(),
                data.get(6..)
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default()
            );
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
                // With the GPS on, it says where the radio is all the same.
                if radio.told.is_some() {
                    radio.told = Some((lat, lon));
                    println!("told {lat}, {lon}, but the GPS says where");
                } else {
                    (radio.lat, radio.lon) = (lat, lon);
                    println!("at {lat}, {lon}");
                }
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
        // GET_CUSTOM_VARS: its GPS, on or off.
        0x28 => {
            let on = if radio.told.is_some() { "1" } else { "0" };
            let mut frame = vec![0x15];
            frame.extend(format!("gps:{on}").as_bytes());
            vec![frame]
        }
        // SET_CUSTOM_VAR: `gps:0` or `gps:1`, and `gps_interval`, taken.
        0x29 => match String::from_utf8_lossy(data).split_once(':') {
            Some(("gps", "1")) => {
                if radio.told.is_none() {
                    radio.told = Some((radio.lat, radio.lon));
                    (radio.lat, radio.lon) = GPS_AT;
                }
                println!("GPS on: at {}, {}", radio.lat, radio.lon);
                ok()
            }
            Some(("gps", _)) => {
                if let Some(told) = radio.told.take() {
                    (radio.lat, radio.lon) = told;
                }
                println!("GPS off: at {}, {}", radio.lat, radio.lon);
                ok()
            }
            Some(("gps_interval", secs)) if secs.parse::<u32>().is_ok() => {
                println!("GPS every {secs} s");
                ok()
            }
            _ => error(6),
        },
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
                    frame.extend((up / 25 + busy_secs(&mut radio)).to_le_bytes());
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
