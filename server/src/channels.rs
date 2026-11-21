// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A radio's channels as they move from slot to slot. Each is told apart by
//! a fingerprint of its name and key, never the key itself: a channel found
//! in another slot has moved, and its history goes with it; one gone has its
//! history kept aside, to give back if it comes back.

use crate::settings::Invalid;
use meshctl_core::mesh::Settings;
use meshctl_proto::meshtastic::{self as pb, config};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

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
fn preset_name(lora: Option<&config::LoRaConfig>) -> &'static str {
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

pub fn fingerprint(name: &str, psk: &[u8]) -> Fingerprint {
    let mut hash = Sha256::new();
    hash.update((name.len() as u32).to_be_bytes());
    hash.update(name.as_bytes());
    hash.update(psk);
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

/// What became of the channels between two readings of them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Moved: from one slot, to another.
    pub moves: Vec<(u32, u32)>,
    /// Gone, from the slot they had: their history is kept aside.
    pub removed: Vec<Slot>,
    /// Back, after they had gone, in the slot they now have.
    pub restored: Vec<(Fingerprint, u32)>,
}

impl Plan {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty() && self.removed.is_empty() && self.restored.is_empty()
    }
}

/// What became of the channels, from those there were to those there are,
/// knowing those gone before. A channel found elsewhere moved; one that
/// came back is given its history back; one in the same slot with another
/// fingerprint, found nowhere else, was edited where it is, and keeps its
/// history; the rest are gone.
pub fn plan(old: &[Slot], new: &[Slot], former: &HashSet<Fingerprint>) -> Plan {
    let mut plan = Plan::default();
    let mut old_matched = vec![false; old.len()];
    let mut new_matched = vec![false; new.len()];
    for (n, now) in new.iter().enumerate() {
        if let Some(o) = old
            .iter()
            .enumerate()
            .position(|(o, then)| !old_matched[o] && then.fingerprint == now.fingerprint)
        {
            old_matched[o] = true;
            new_matched[n] = true;
            if old[o].index != now.index {
                plan.moves.push((old[o].index, now.index));
            }
        }
    }
    for (n, now) in new.iter().enumerate() {
        if !new_matched[n] && former.contains(&now.fingerprint) {
            new_matched[n] = true;
            plan.restored.push((now.fingerprint, now.index));
        }
    }
    for (o, then) in old.iter().enumerate() {
        if old_matched[o] {
            continue;
        }
        let edited_here = new
            .iter()
            .enumerate()
            .any(|(n, now)| !new_matched[n] && now.index == then.index);
        if edited_here {
            if let Some(n) = new.iter().position(|now| now.index == then.index) {
                new_matched[n] = true;
            }
        } else {
            plan.removed.push(then.clone());
        }
    }
    plan
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

    #[test]
    fn channels_that_swap_take_their_history() {
        let old = [slot(0, "A"), slot(1, "B"), slot(2, "C")];
        let new = [old[0].clone(), moved(&old[2], 1), moved(&old[1], 2)];
        let plan = plan(&old, &new, &HashSet::new());
        assert_eq!(plan.moves, vec![(2, 1), (1, 2)]);
        assert!(plan.removed.is_empty() && plan.restored.is_empty());
    }

    #[test]
    fn a_channel_edited_where_it_is_keeps_its_history() {
        let old = [slot(0, "A"), slot(1, "B")];
        let new = [old[0].clone(), slot(1, "B2")];
        assert!(plan(&old, &new, &HashSet::new()).is_empty());
    }

    #[test]
    fn a_channel_gone_has_its_history_kept_and_given_back() {
        let old = [slot(0, "A"), slot(1, "B"), slot(2, "C")];
        // B goes, and C takes its slot.
        let new = [old[0].clone(), moved(&old[2], 1)];
        let gone = plan(&old, &new, &HashSet::new());
        assert_eq!(gone.moves, vec![(2, 1)]);
        assert_eq!(gone.removed, vec![old[1].clone()]);
        // And B comes back, in another slot.
        let back = [new[0].clone(), new[1].clone(), moved(&old[1], 2)];
        let former = HashSet::from([old[1].fingerprint]);
        let plan = plan(&new, &back, &former);
        assert_eq!(plan.restored, vec![(old[1].fingerprint, 2)]);
        assert!(plan.moves.is_empty() && plan.removed.is_empty());
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
        // And the fingerprints follow them.
        let mut after = settings.clone();
        for channel in sent {
            after.channels.insert(channel.index as u32, channel);
        }
        let plan = plan(&slots(&settings), &slots(&after), &HashSet::new());
        assert_eq!(plan.moves, vec![(2, 0), (0, 1), (1, 2)]);
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
