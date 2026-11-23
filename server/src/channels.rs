// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A radio's channels as they move from slot to slot. Each is told apart by
//! a fingerprint of its name and key, never the key itself, and is one
//! conversation wherever it is.

use crate::settings::Invalid;
use meshctl_core::mesh::Settings;
use meshctl_proto::meshtastic::{self as pb, config};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// What tells one channel from another: its name and its key, hashed.
pub type Fingerprint = [u8; 16];

/// The slots a radio has for channels.
pub const SLOTS: u32 = 8;

/// A channel in its slot, as fingerprinted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub index: u32,
    pub fingerprint: Fingerprint,
    /// Its name; an unnamed one goes by its preset's.
    pub name: String,
}

/// A modem preset's name, for an unnamed channel's fingerprint. Only meshctl
/// uses it: nothing is ever named this way on the radio, which names an
/// unnamed channel itself, on the air too.
#[allow(deprecated)] // A radio may still be set to one.
pub fn preset_name(lora: Option<&config::LoRaConfig>) -> &'static str {
    use config::lo_ra_config::ModemPreset as P;
    let Some(lora) = lora else { return "" };
    if !lora.use_preset {
        return "Custom";
    }
    match P::try_from(lora.modem_preset) {
        Ok(P::LongFast) => "LongFast",
        Ok(P::LongSlow) => "LongSlow",
        Ok(P::MediumSlow) => "MediumSlow",
        Ok(P::MediumFast) => "MediumFast",
        Ok(P::ShortSlow) => "ShortSlow",
        Ok(P::ShortFast) => "ShortFast",
        Ok(P::ShortTurbo) => "ShortTurbo",
        Ok(P::LongModerate) => "LongMod",
        Ok(P::LongTurbo) => "LongTurbo",
        Ok(P::LiteFast) => "LiteFast",
        Ok(P::LiteSlow) => "LiteSlow",
        Ok(P::NarrowFast) => "NarrowFast",
        Ok(P::NarrowSlow) => "NarrowSlow",
        Ok(P::TinyFast) => "TinyFast",
        Ok(P::TinySlow) => "TinySlow",
        Ok(P::MediumTurbo) => "MediumTurbo",
        // One the firmware has no name for, as it says.
        Ok(P::VeryLongSlow) | Err(_) => "Invalid",
    }
}

/// A channel's name, or its preset's for an unnamed one.
fn effective_name(settings: &Settings, channel: &pb::ChannelSettings) -> String {
    if channel.name.is_empty() {
        preset_name(settings.config.lora.as_ref()).to_owned()
    } else {
        channel.name.clone()
    }
}

/// The key the radio well knows: what a key of one byte, 1, stands for.
const DEFAULT_KEY: [u8; 16] = [
    0xd4, 0xf1, 0xbb, 0x3a, 0x20, 0x29, 0x07, 0x59, 0xf0, 0xbc, 0xff, 0xab, 0xcf, 0x4e, 0x69, 0x01,
];

/// Whether a channel's key is one anyone can read with, as the firmware
/// tells it: none, any of one byte (the default and the simple ones), or the
/// default spelled out. A secondary without a key uses the primary's.
pub fn public_key(psk: &[u8], secondary: bool, primary_psk: &[u8]) -> bool {
    match psk {
        [] if secondary => public_key(primary_psk, false, &[]),
        [] | [_] => true,
        key => key.len() == DEFAULT_KEY.len() && key[..15] == DEFAULT_KEY[..15],
    }
}

/// A key as the radio uses it: none for none or 0; the default one for 1,
/// and the simple ones, 2 to 255, as it with its last byte moved on.
fn key_in_use(psk: &[u8]) -> Vec<u8> {
    match psk {
        [] | [0] => Vec::new(),
        [n] => {
            let mut key = DEFAULT_KEY.to_vec();
            key[15] = key[15].wrapping_add(n - 1);
            key
        }
        key => key.to_vec(),
    }
}

pub fn fingerprint(name: &str, psk: &[u8]) -> Fingerprint {
    let mut hash = Sha256::new();
    hash.update((name.len() as u32).to_be_bytes());
    hash.update(name.as_bytes());
    hash.update(key_in_use(psk));
    let digest = hash.finalize();
    let mut print = [0; 16];
    print.copy_from_slice(&digest[..16]);
    print
}

fn enabled(channel: &pb::Channel) -> bool {
    channel.role != pb::channel::Role::Disabled as i32
}

/// The channels a radio has, in their slots.
pub fn slots(settings: &Settings) -> Vec<Slot> {
    (0..SLOTS)
        .filter_map(|index| {
            let channel = settings.channels.get(&index).filter(|c| enabled(c))?;
            let channel_settings = channel.settings.clone().unwrap_or_default();
            let name = effective_name(settings, &channel_settings);
            Some(Slot {
                index,
                fingerprint: fingerprint(&name, &channel_settings.psk),
                name,
            })
        })
        .collect()
}

/// The slot of a channel the radio has that one of `changed` would become
/// the very same as, name and key, once they are set: the radio could not
/// tell them apart. Two that already were the same, as another app may have
/// left them, are left be, and either can still be changed.
pub fn duplicate(before: &Settings, after: &Settings, changed: &[u32]) -> Option<u32> {
    let then = slots(before);
    let now = slots(after);
    let print_then = |index: u32| {
        then.iter()
            .find(|s| s.index == index)
            .map(|s| s.fingerprint)
    };
    now.iter()
        .filter(|s| changed.contains(&s.index))
        .find_map(|new| {
            now.iter()
                .find(|other| {
                    other.index != new.index
                        && other.fingerprint == new.fingerprint
                        // Not the same already, both of them as they were.
                        && !(print_then(new.index) == Some(new.fingerprint)
                            && print_then(other.index) == Some(other.fingerprint))
                })
                .map(|other| other.index)
        })
}

/// A fingerprint for a slot whose channel is not known yet, as on a radio
/// before it is first read, or on one that does not say: its conversation
/// takes the channel's own fingerprint once that is known.
pub fn provisional(slot: u32) -> Fingerprint {
    fingerprint(&format!("slot {slot}"), b"provisional")
}

/// Which conversation a slot read is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chat {
    /// The one with its fingerprint.
    Known(i64),
    /// The one the slot had, whose channel was changed where it is: it takes
    /// the new fingerprint.
    Rekeyed(i64),
    /// The same as the one of an earlier slot, by its place in the list: the
    /// very same channel twice.
    SameAs(usize),
    /// A new one.
    New,
}

/// The conversation of each slot read, from the slots there were (each with
/// its conversation and that one's fingerprint) and the conversations known
/// by fingerprint. A slot takes the conversation with its fingerprint; else
/// the one it had, if that one's fingerprint is nowhere now, as when the
/// channel was renamed or given another key where it is; else a new one.
pub fn assign(
    had: &[(u32, i64, Fingerprint)],
    now: &[Slot],
    known: &HashMap<Fingerprint, i64>,
) -> Vec<Chat> {
    let mut chats = Vec::with_capacity(now.len());
    for (at, slot) in now.iter().enumerate() {
        let chat = if let Some(earlier) = now[..at]
            .iter()
            .position(|s| s.fingerprint == slot.fingerprint)
        {
            Chat::SameAs(earlier)
        } else if let Some(&id) = known.get(&slot.fingerprint) {
            Chat::Known(id)
        } else if let Some(&(_, id, _)) = had.iter().find(|(index, id, print)| {
            *index == slot.index
                && !now.iter().any(|s| s.fingerprint == *print)
                // Not one already taken, by a slot it had moved to.
                && !chats.iter().any(|c| matches!(c, Chat::Rekeyed(taken) if taken == id))
        }) {
            Chat::Rekeyed(id)
        } else {
            Chat::New
        };
        chats.push(chat);
    }
    chats
}

/// The channels a radio is to be sent for those it has to be in `order`,
/// by their slots now: the first becomes the primary, the rest follow it
/// with no gap, and any slot left over is freed. Only those that change.
pub fn rearranged(settings: &Settings, order: &[u32]) -> Result<Vec<pb::Channel>, Invalid> {
    let have: Vec<u32> = slots(settings).iter().map(|s| s.index).collect();
    let mut seen = HashSet::new();
    if order.is_empty()
        || order
            .iter()
            .any(|index| !have.contains(index) || !seen.insert(*index))
    {
        return Err(Invalid("order"));
    }
    let mut wanted = Vec::new();
    for (at, from) in order.iter().enumerate() {
        let at = at as u32;
        let mut channel = settings.channels.get(from).cloned().unwrap_or_default();
        // Moved as it is: one unnamed stays so, in whatever slot, for the
        // radio names it the same everywhere, and on the air it is the same
        // channel only under the very same name.
        let channel_settings = channel.settings.take().unwrap_or_default();
        channel.index = at as i32;
        channel.role = if at == 0 {
            pb::channel::Role::Primary
        } else {
            pb::channel::Role::Secondary
        } as i32;
        channel.settings = Some(channel_settings);
        wanted.push(channel);
    }
    for at in order.len() as u32..SLOTS {
        if have.contains(&at) {
            wanted.push(pb::Channel {
                index: at as i32,
                role: pb::channel::Role::Disabled as i32,
                settings: Some(pb::ChannelSettings::default()),
            });
        }
    }
    Ok(wanted
        .into_iter()
        .filter(|channel| settings.channels.get(&(channel.index as u32)) != Some(channel))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(index: u32, name: &str) -> Slot {
        Slot {
            index,
            fingerprint: fingerprint(name, &[index as u8; 16]),
            name: name.into(),
        }
    }

    fn moved(slot: &Slot, index: u32) -> Slot {
        Slot {
            index,
            ..slot.clone()
        }
    }

    fn had(slots: &[Slot], ids: &[i64]) -> Vec<(u32, i64, Fingerprint)> {
        slots
            .iter()
            .zip(ids)
            .map(|(s, id)| (s.index, *id, s.fingerprint))
            .collect()
    }

    fn known(slots: &[Slot], ids: &[i64]) -> HashMap<Fingerprint, i64> {
        slots
            .iter()
            .zip(ids)
            .map(|(s, id)| (s.fingerprint, *id))
            .collect()
    }

    #[test]
    fn channels_that_swap_keep_their_conversations() {
        let old = [slot(0, "A"), slot(1, "B"), slot(2, "C")];
        let new = [old[0].clone(), moved(&old[2], 1), moved(&old[1], 2)];
        let chats = assign(&had(&old, &[10, 11, 12]), &new, &known(&old, &[10, 11, 12]));
        assert_eq!(chats, [Chat::Known(10), Chat::Known(12), Chat::Known(11)]);
    }

    #[test]
    fn a_channel_edited_where_it_is_keeps_its_conversation() {
        let old = [slot(0, "A"), slot(1, "B")];
        let new = [old[0].clone(), slot(1, "B2")];
        let chats = assign(&had(&old, &[10, 11]), &new, &known(&old, &[10, 11]));
        assert_eq!(chats, [Chat::Known(10), Chat::Rekeyed(11)]);
    }

    #[test]
    fn a_channel_back_finds_its_conversation() {
        let old = [slot(0, "A")];
        let gone = slot(1, "B");
        // B, gone before, is still known; C is new.
        let mut all = known(&old, &[10]);
        all.insert(gone.fingerprint, 11);
        let new = [old[0].clone(), moved(&gone, 2), slot(3, "C")];
        let chats = assign(&had(&old, &[10]), &new, &all);
        assert_eq!(chats, [Chat::Known(10), Chat::Known(11), Chat::New]);
    }

    #[test]
    fn the_very_same_channel_twice_is_one_conversation() {
        let a = slot(0, "A");
        let new = [a.clone(), slot(1, "B"), moved(&a, 3)];
        let chats = assign(&[], &new, &HashMap::new());
        assert_eq!(chats, [Chat::New, Chat::New, Chat::SameAs(0)]);
    }

    #[test]
    fn a_slot_first_known_by_its_place_takes_its_channel() {
        let new = [slot(0, "A")];
        let had = [(0, 10, provisional(0))];
        let known = HashMap::from([(provisional(0), 10)]);
        assert_eq!(assign(&had, &new, &known), [Chat::Rekeyed(10)]);
    }

    fn radio() -> Settings {
        let mut settings = Settings::default();
        settings.config.lora = Some(config::LoRaConfig {
            use_preset: true,
            modem_preset: config::lo_ra_config::ModemPreset::LongFast as i32,
            ..Default::default()
        });
        let channel = |index: u32, role: pb::channel::Role, name: &str, psk: Vec<u8>| pb::Channel {
            index: index as i32,
            role: role as i32,
            settings: Some(pb::ChannelSettings {
                name: name.into(),
                psk,
                ..Default::default()
            }),
        };
        settings
            .channels
            .insert(0, channel(0, pb::channel::Role::Primary, "", vec![1]));
        settings.channels.insert(
            1,
            channel(1, pb::channel::Role::Secondary, "Moire", vec![7; 32]),
        );
        settings.channels.insert(
            2,
            channel(2, pb::channel::Role::Secondary, "Lab", vec![9; 16]),
        );
        settings
    }

    #[test]
    fn public_keys_are_told_as_the_radio_tells_them() {
        assert!(public_key(&[], false, &[]));
        assert!(public_key(&[1], false, &[]));
        assert!(public_key(&[5], false, &[]));
        assert!(public_key(&DEFAULT_KEY, false, &[]));
        assert!(!public_key(&[9; 16], false, &[]));
        assert!(!public_key(&[9; 32], false, &[]));
        // A secondary without a key, as its primary.
        assert!(public_key(&[], true, &[1]));
        assert!(!public_key(&[], true, &[9; 32]));
    }

    #[test]
    fn keys_count_as_the_radio_uses_them() {
        assert_eq!(fingerprint("A", &[]), fingerprint("A", &[0]));
        assert_eq!(fingerprint("A", &[1]), fingerprint("A", &DEFAULT_KEY));
        assert_ne!(fingerprint("A", &[1]), fingerprint("A", &[2]));
    }

    #[test]
    fn a_channel_the_same_as_another_is_found() {
        let mut settings = radio();
        // Lab, renamed as the primary and with its key: the same channel.
        let lab = settings.channels.get_mut(&2).unwrap();
        let lab_settings = lab.settings.as_mut().unwrap();
        lab_settings.name = String::new();
        lab_settings.psk = DEFAULT_KEY.to_vec();
        assert_eq!(duplicate(&radio(), &settings, &[2]), Some(0));
        // Not if it is the one left as it was.
        assert_eq!(duplicate(&radio(), &settings, &[1]), None);
        // Two already the same, one of them changed in something else: left
        // be.
        let mut mqtt = settings.clone();
        mqtt.channels
            .get_mut(&2)
            .unwrap()
            .settings
            .as_mut()
            .unwrap()
            .uplink_enabled = true;
        assert_eq!(duplicate(&settings, &mqtt, &[2]), None);
    }

    #[test]
    fn an_unnamed_primary_goes_by_its_preset() {
        let slots = slots(&radio());
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0].name, "LongFast");
        assert_eq!(slots[0].fingerprint, fingerprint("LongFast", &[1]));
    }

    #[test]
    fn a_secondary_made_primary_takes_slot_zero() {
        let settings = radio();
        let sent = rearranged(&settings, &[2, 0, 1]).unwrap();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0].index, 0);
        assert_eq!(sent[0].role, pb::channel::Role::Primary as i32);
        assert_eq!(sent[0].settings.as_ref().unwrap().name, "Lab");
        assert_eq!(sent[0].settings.as_ref().unwrap().psk, vec![9; 16]);
        // The old primary keeps no name, and its key.
        assert_eq!(sent[1].role, pb::channel::Role::Secondary as i32);
        assert_eq!(sent[1].settings.as_ref().unwrap().name, "");
        assert_eq!(sent[1].settings.as_ref().unwrap().psk, vec![1]);
    }

    #[test]
    fn a_channel_left_out_frees_the_last_slot() {
        let sent = rearranged(&radio(), &[0, 2]).unwrap();
        // Lab moves up; slot 2 is freed; the primary does not change.
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].index, 1);
        assert_eq!(sent[0].settings.as_ref().unwrap().name, "Lab");
        assert_eq!(sent[1].index, 2);
        assert_eq!(sent[1].role, pb::channel::Role::Disabled as i32);
    }

    #[test]
    fn an_order_of_channels_there_are_not_is_refused() {
        let settings = radio();
        assert!(rearranged(&settings, &[]).is_err());
        assert!(rearranged(&settings, &[0, 0]).is_err());
        assert!(rearranged(&settings, &[0, 5]).is_err());
    }
}
