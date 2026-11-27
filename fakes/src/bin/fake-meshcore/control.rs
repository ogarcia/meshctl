// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The control port: lines that play what the mesh would do to the radio.

use crate::frames::{
    channel_message, contact, direct_message, direct_message_at, flooded, key_prefix,
    reaction_hash, region_key, rx_log,
};
use crate::nodes::room_sync;
use crate::radio::{Contact, Event, MAX_CONTACTS, ME, Radio, key};
use meshctl_core::meshcore::raw;
use meshctl_fakes::{now, random};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::broadcast;

/// The seconds received on a channel made busy with `busy`, so far.
pub fn busy_secs(radio: &mut Radio) -> u32 {
    let at = now();
    let (percent, since, banked) = radio.busy;
    let banked = banked + (at - since) * percent / 100;
    radio.busy = (percent, at, banked);
    banked
}

/// A node not known before heard advertising, as the firmware takes one:
/// left out if its kind is not added by itself, or it is further than the
/// hops allowed, and only told of; added if there is room, or in place of
/// the contact changed longest ago if the radio may overwrite one; and if
/// not, told of, and the radio full. What happened, in words.
fn advertised(
    radio: &mut Radio,
    new: Contact,
    hops: u8,
    events: &broadcast::Sender<Event>,
) -> String {
    let name = new.name.clone();
    // Its kind's bit, as the auto-add settings have them.
    let bit = 1u8 << new.kind.min(4);
    let (config, most_hops) = radio.autoadd;
    let taken = !radio.manual_add || config & bit != 0;
    if !taken {
        let _ = events.send(Event::Push(contact(0x8a, &new)));
        return format!("{name} advertises, not added: its kind is not added by itself");
    }
    if most_hops > 0 && hops >= most_hops {
        let _ = events.send(Event::Push(contact(0x8a, &new)));
        return format!("{name} advertises, not added: {hops} hops away");
    }
    let mut told = format!("{name} advertises, added");
    if radio.contacts.len() >= usize::from(MAX_CONTACTS) {
        // None is a favourite here: the one changed longest ago goes.
        let oldest = radio
            .contacts
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| c.last_modified)
            .map(|(i, _)| i);
        match oldest {
            Some(i) if config & 0x01 != 0 => {
                let gone = radio.contacts.remove(i);
                let mut deleted = vec![0x8f];
                deleted.extend(gone.key);
                let _ = events.send(Event::Push(deleted));
                told = format!("{name} advertises, added in place of {}", gone.name);
            }
            _ => {
                let _ = events.send(Event::Push(contact(0x8a, &new)));
                let _ = events.send(Event::Push(vec![0x90]));
                return format!("{name} advertises, not added: the radio is full");
            }
        }
    }
    let mut advert = vec![0x80];
    advert.extend(new.key);
    let _ = events.send(Event::Push(advert));
    radio.contacts.push(new);
    told
}

/// Does what a line of the control port says; what to answer it.
pub fn control(line: &str, radio: &Mutex<Radio>, events: &broadcast::Sender<Event>) -> String {
    let mut words = line.trim().splitn(3, ' ');
    let mut radio = radio.lock().unwrap_or_else(|e| e.into_inner());
    match words.next() {
        Some("hear") => {
            let slot = words.next().and_then(|w| w.parse::<u8>().ok());
            let (Some(slot), Some(said)) = (slot, words.next()) else {
                return "usage: hear <slot> [<name>|]<text>".into();
            };
            if radio
                .channels
                .get(usize::from(slot))
                .is_none_or(|(name, _)| name.is_empty())
            {
                return format!("no channel in slot {slot}");
            }
            const SENDERS: [&str; 4] = ["Pepe", "Uxía", "Brais 🚲", "EA1XYZ"];
            let (sender, text) = match said.split_once('|') {
                Some((name, text)) => (name.trim(), text.trim()),
                None => (SENDERS[usize::try_from(random(4)).unwrap_or(0)], said),
            };
            let at = now();
            radio
                .said
                .insert(slot, (sender.to_owned(), text.to_owned(), at));
            let message = channel_message(slot, at, sender, text);
            radio.queued.push_back(message);
            // Heard twice: through Monte and Torre, then through the room
            // and Torre; on Galicia, in its region.
            let secret = radio.channels[usize::from(slot)].1;
            let payload = raw::seal_channel_text(&secret, at, &format!("{sender}: {text}"));
            let region =
                (radio.channels[usize::from(slot)].0 == "Galicia").then(|| region_key("galicia"));
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
        Some("answers") => {
            radio.answers = words.next() != Some("off");
            format!("answers {}", if radio.answers { "given" } else { "none" })
        }
        Some("busy") => {
            let Some(percent) = words.next().and_then(|w| w.parse::<u32>().ok()) else {
                return "usage: busy <percent>".into();
            };
            busy_secs(&mut radio);
            radio.busy.0 = percent.min(95);
            format!("channel {}% busier", radio.busy.0)
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
        // A node not known before advertises, of the kind asked for, a
        // companion if none, its advert flooded through Monte.
        Some("advert") => {
            let (kind, called) = match words.next() {
                None | Some("chat") => (1, "Nodo nuevo"),
                Some("repeater") => (2, "Repetidor nuevo"),
                Some("room") => (3, "Sala nueva"),
                Some("sensor") => (4, "Sensor nuevo"),
                Some(_) => return "usage: advert [chat|repeater|room|sensor]".into(),
            };
            let at = now();
            radio.adverts = radio.adverts.wrapping_add(1);
            let n = radio.adverts;
            let mut new_key = key(0x60);
            new_key[0] = 0xad;
            new_key[1..3].copy_from_slice(&n.to_be_bytes());
            let new = Contact {
                key: new_key,
                kind,
                path_len: i8::try_from(random(3)).unwrap_or(0),
                name: format!("{called} {n}"),
                last_advert: at,
                lat: 43_350_000 + i32::try_from(random(40_000)).unwrap_or(0),
                lon: -8_420_000 + i32::try_from(random(40_000)).unwrap_or(0),
                last_modified: at,
            };
            // Heard raw, its advert flooded through Monte.
            let mut payload = new.key.to_vec();
            payload.extend(at.to_le_bytes());
            payload.extend([0; 64]);
            payload.push(0x80 | 0x10 | new.kind);
            payload.extend(new.lat.to_le_bytes());
            payload.extend(new.lon.to_le_bytes());
            payload.extend(new.name.as_bytes());
            let packet = flooded(raw::PAYLOAD_ADVERT, &[key(0x52)], None, &payload);
            let _ = events.send(Event::Push(rx_log(5, -95, &packet)));
            advertised(&mut radio, new, 1, events)
        }
        // Many contacts, about A Coruña, of every kind and heard at every
        // age, as many as asked for and the radio has room for; the link
        // dropped to have them read. 0, none.
        Some("crowd") => {
            let Some(count) = words.next().and_then(|w| w.parse::<u32>().ok()) else {
                return format!("usage: crowd <contacts, up to {MAX_CONTACTS} in all>");
            };
            radio.contacts.retain(|c| c.key[0] != 0xcc);
            let room = u32::from(MAX_CONTACTS).saturating_sub(u32::try_from(radio.contacts.len()).unwrap_or(u32::MAX));
            let clock = radio.clock();
            let crowd = crate::crowd::crowd(count.min(room), clock);
            let told = crowd.len();
            radio.contacts.extend(crowd);
            let all = radio.contacts.len();
            drop(radio);
            let _ = events.send(Event::Drop);
            format!("a crowd of {told}, {all} contacts of {MAX_CONTACTS}; dropped to have them read")
        }
        // Someone else's trace heard, raw: along Monte, Torre and Monte
        // again, by two bytes of each, two of its hops gone; then the last.
        Some("trace") => {
            let tag = u32::try_from(random(u64::from(u32::MAX))).unwrap_or(7);
            let mut payload = tag.to_le_bytes().to_vec();
            payload.extend([0; 4]);
            payload.push(0x01);
            for hop in [key(0x52), key(0x55), key(0x52)] {
                payload.extend(&hop[..2]);
            }
            let leg = |snrs: &[i8]| {
                let mut packet = vec![(raw::PAYLOAD_TRACE << 2) | raw::ROUTE_DIRECT];
                packet.push(u8::try_from(snrs.len()).unwrap_or(0));
                packet.extend(snrs.iter().map(|s| (s * 4).cast_unsigned()));
                packet.extend(&payload);
                packet
            };
            let _ = events.send(Event::Push(rx_log(7, -88, &leg(&[5, -3]))));
            let last = leg(&[5, -3, 8]);
            let events = events.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(700)).await;
                let _ = events.send(Event::Push(rx_log(4, -97, &last)));
            });
            "a trace goes by".into()
        }
        Some("drop") => {
            let _ = events.send(Event::Drop);
            "dropped".into()
        }
        _ => "commands: hear <slot> [<name>|]<text>, dm <text>, react <slot> <emoji>, \
             dmreact <emoji>, post <text>, room missing|strict|open, advert [chat|repeater|room|sensor], crowd <contacts>, trace, \
             answers on|off, busy <percent>, echoes on|off, drop"
            .into(),
    }
}
