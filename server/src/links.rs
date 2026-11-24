// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Links to add nodes and channels from, as the Meshtastic apps share them:
//! a contact after `meshtastic.org/v/#`, channels after `meshtastic.org/e/#`,
//! each a protobuf in base64url. What one holds, and what adding it takes.

use crate::channels::{Fingerprint, fingerprint, preset_name};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use meshctl_api::{ChannelChange, ChannelKey, KeyChange, LinkChannel, LinkContents};
use meshctl_core::mesh::{Node, Settings};
use meshctl_proto::meshtastic::{self as pb, ChannelSet, SharedContact, channel};
use prost::Message;
use std::collections::BTreeMap;

/// What a link holds, decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Contact(SharedContact),
    /// The channels, and whether the link is meant to replace the radio's
    /// rather than to add to them.
    Channels(ChannelSet, bool),
}

/// Reads a link: a contact or channels, whatever the case of its host, with
/// or without `https://` or `www.`, its base64 padded or not, URL-safe or
/// not. None for anything else.
pub fn parse(url: &str) -> Option<Parsed> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let (host, rest) = rest.split_once('/')?;
    if !host.eq_ignore_ascii_case("meshtastic.org") {
        return None;
    }
    let (path, data) = rest.split_once('#')?;
    // `e/?add=true#…` adds; `e/#…` replaces, as the apps share a whole
    // configuration.
    let (kind, query) = path.split_once('?').unwrap_or((path, ""));
    let kind = kind.trim_end_matches('/');
    let adds = query
        .split('&')
        .any(|pair| pair.trim_end_matches('/').eq_ignore_ascii_case("add=true"));
    let data = data
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_");
    let bytes = URL_SAFE_NO_PAD.decode(data).ok()?;
    match kind {
        "v" => {
            let contact = SharedContact::decode(bytes.as_slice()).ok()?;
            contact.user.is_some().then_some(Parsed::Contact(contact))
        }
        "e" => {
            let set = ChannelSet::decode(bytes.as_slice()).ok()?;
            (!set.settings.is_empty()).then_some(Parsed::Channels(set, !adds))
        }
        _ => None,
    }
}

/// The kind of a channel's key, as the settings say it.
fn key_kind(psk: &[u8]) -> ChannelKey {
    match psk {
        [] | [0] => ChannelKey::None,
        [1] => ChannelKey::Default,
        [_] => ChannelKey::Simple,
        key if key.len() <= 16 => ChannelKey::Aes128,
        _ => ChannelKey::Aes256,
    }
}

/// A link's channel's name: its own, or its preset's when it has none.
fn name_of(settings: &pb::ChannelSettings, lora: Option<&pb::config::LoRaConfig>) -> String {
    if settings.name.is_empty() {
        preset_name(lora).to_owned()
    } else {
        settings.name.clone()
    }
}

/// Whether the radio has a channel already, by its name and key: a slot of
/// its own unnamed goes by the radio's preset.
fn present(radio: &Settings, name: &str, psk: &[u8]) -> bool {
    let wanted = fingerprint(name, psk);
    radio.channels.values().any(|c| {
        c.role != channel::Role::Disabled as i32
            && c.settings.as_ref().is_some_and(|s| {
                fingerprint(&name_of(s, radio.config.lora.as_ref()), &s.psk) == wanted
            })
    })
}

/// The radio's slots that hold no channel, in order: never the primary's.
fn free_slots(radio: &Settings) -> Vec<u32> {
    (1..8)
        .filter(|index| {
            radio
                .channels
                .get(index)
                .is_none_or(|c| c.role == channel::Role::Disabled as i32)
        })
        .collect()
}

/// What a link holds, against what the radio has and knows.
pub fn contents(parsed: &Parsed, radio: &Settings, nodes: &BTreeMap<u32, Node>) -> LinkContents {
    match parsed {
        Parsed::Contact(contact) => {
            let user = contact.user.clone().unwrap_or_default();
            LinkContents::Contact {
                node: contact.node_num,
                id: format!("!{:08x}", contact.node_num),
                long_name: user.long_name,
                short_name: user.short_name,
                hw_model: pb::HardwareModel::try_from(user.hw_model)
                    .ok()
                    .filter(|m| *m != pb::HardwareModel::Unset)
                    .map(|m| m.as_str_name().to_owned()),
                key: !user.public_key.is_empty(),
                known: nodes.contains_key(&contact.node_num),
            }
        }
        Parsed::Channels(set, replaces) => {
            let lora = set.lora_config.as_ref();
            let kept: Vec<Fingerprint> = set
                .settings
                .iter()
                .map(|s| fingerprint(&name_of(s, lora), &s.psk))
                .collect();
            let removed = radio
                .channels
                .values()
                .filter(|c| c.role != channel::Role::Disabled as i32)
                .filter_map(|c| c.settings.as_ref())
                .map(|s| (name_of(s, radio.config.lora.as_ref()), s.psk.clone()))
                .filter(|(name, psk)| !kept.contains(&fingerprint(name, psk)))
                .map(|(name, _)| name)
                .collect();
            let channels = set
                .settings
                .iter()
                .map(|settings| {
                    let name = name_of(settings, lora);
                    LinkChannel {
                        present: present(radio, &name, &settings.psk),
                        key: key_kind(&settings.psk),
                        name,
                    }
                })
                .collect();
            let ours = radio.config.lora.as_ref();
            let lora_differs = lora.is_some_and(|theirs| {
                ours.is_none_or(|ours| {
                    theirs.use_preset != ours.use_preset
                        || (theirs.use_preset && theirs.modem_preset != ours.modem_preset)
                        || (theirs.region != 0 && theirs.region != ours.region)
                })
            });
            LinkContents::Channels {
                channels,
                free_slots: u32::try_from(free_slots(radio).len()).unwrap_or_default(),
                preset: lora.map(|l| preset_name(Some(l)).to_owned()),
                lora_differs,
                region: lora
                    .filter(|l| l.region != 0)
                    .and_then(|l| pb::config::lo_ra_config::RegionCode::try_from(l.region).ok())
                    .map(|r| r.as_str_name().to_owned()),
                replaces: *replaces,
                removed,
                lora: lora_settings(set, radio),
            }
        }
    }
}

/// The channels a link adds to a radio, each to a free slot, as secondaries:
/// those it has already left out. None if they do not fit.
pub fn channel_changes(set: &ChannelSet, radio: &Settings) -> Option<Vec<ChannelChange>> {
    let lora = set.lora_config.as_ref();
    let wanted: Vec<&pb::ChannelSettings> = set
        .settings
        .iter()
        .filter(|s| !present(radio, &name_of(s, lora), &s.psk))
        .collect();
    let free = free_slots(radio);
    if wanted.len() > free.len() {
        return None;
    }
    Some(
        wanted
            .into_iter()
            .zip(free)
            .map(|(settings, index)| {
                let (key, custom_key) = match settings.psk.as_slice() {
                    [] | [0] => (KeyChange::None, None),
                    [1] => (KeyChange::Default, None),
                    psk => (KeyChange::Custom, Some(STANDARD.encode(psk))),
                };
                ChannelChange {
                    index,
                    role: "SECONDARY".into(),
                    name: name_of(settings, lora),
                    key,
                    custom_key,
                    uplink: settings.uplink_enabled,
                    downlink: settings.downlink_enabled,
                    position_precision: settings
                        .module_settings
                        .as_ref()
                        .map(|m| m.position_precision),
                    muted: None,
                }
            })
            .collect(),
    )
}

/// The channels a link puts in place of every one of a radio's: the first
/// the primary, unnamed if it is, the rest secondaries, named by their
/// preset when they are not; the slots left over emptied. Eight at most.
pub fn replacing_changes(set: &ChannelSet) -> Vec<ChannelChange> {
    let lora = set.lora_config.as_ref();
    (0..8_u32)
        .map(|index| match set.settings.get(index as usize) {
            Some(settings) => {
                let (key, custom_key) = match settings.psk.as_slice() {
                    [] | [0] => (KeyChange::None, None),
                    [1] => (KeyChange::Default, None),
                    psk => (KeyChange::Custom, Some(STANDARD.encode(psk))),
                };
                ChannelChange {
                    index,
                    role: if index == 0 { "PRIMARY" } else { "SECONDARY" }.into(),
                    name: if index == 0 {
                        settings.name.clone()
                    } else {
                        name_of(settings, lora)
                    },
                    key,
                    custom_key,
                    uplink: settings.uplink_enabled,
                    downlink: settings.downlink_enabled,
                    position_precision: settings
                        .module_settings
                        .as_ref()
                        .map(|m| m.position_precision),
                    muted: None,
                }
            }
            None => ChannelChange {
                index,
                role: "DISABLED".into(),
                name: String::new(),
                key: KeyChange::None,
                custom_key: None,
                uplink: false,
                downlink: false,
                position_precision: None,
                muted: None,
            },
        })
        .collect()
}

/// A link's LoRa settings as the radio's are changed: its region, when it
/// says one, and its preset or the modem by hand; the rest as the radio
/// has it now.
pub fn lora_settings(set: &ChannelSet, radio: &Settings) -> Option<meshctl_api::LoraSettings> {
    use pb::config::lo_ra_config::{ModemPreset, RegionCode};
    let theirs = set.lora_config.as_ref()?;
    let ours = radio.config.lora.clone().unwrap_or_default();
    let region = if theirs.region == 0 {
        ours.region
    } else {
        theirs.region
    };
    Some(meshctl_api::LoraSettings {
        region: RegionCode::try_from(region).ok()?.as_str_name().to_owned(),
        use_preset: theirs.use_preset,
        modem_preset: ModemPreset::try_from(theirs.modem_preset)
            .ok()?
            .as_str_name()
            .to_owned(),
        bandwidth: theirs.bandwidth,
        spread_factor: theirs.spread_factor,
        coding_rate: theirs.coding_rate,
        hop_limit: if theirs.hop_limit == 0 {
            ours.hop_limit
        } else {
            theirs.hop_limit
        },
        tx_enabled: ours.tx_enabled,
        advanced: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(kind: &str, message: &impl Message) -> String {
        format!(
            "https://meshtastic.org/{kind}/#{}",
            URL_SAFE_NO_PAD.encode(message.encode_to_vec())
        )
    }

    fn radio(channels: &[(u32, &str, &[u8])]) -> Settings {
        let mut settings = Settings::default();
        settings.config.lora = Some(pb::config::LoRaConfig {
            use_preset: true,
            modem_preset: pb::config::lo_ra_config::ModemPreset::LongFast as i32,
            region: pb::config::lo_ra_config::RegionCode::Eu868 as i32,
            ..Default::default()
        });
        for (index, name, psk) in channels {
            settings.channels.insert(
                *index,
                pb::Channel {
                    index: *index as i32,
                    role: if *index == 0 {
                        channel::Role::Primary
                    } else {
                        channel::Role::Secondary
                    } as i32,
                    settings: Some(pb::ChannelSettings {
                        name: (*name).into(),
                        psk: psk.to_vec(),
                        ..Default::default()
                    }),
                },
            );
        }
        settings
    }

    #[test]
    fn a_contact_link_reads_as_the_apps_share_it() {
        let contact = SharedContact {
            node_num: 0x1234_abcd,
            user: Some(pb::User {
                long_name: "Casa".into(),
                short_name: "CASA".into(),
                public_key: vec![7; 32],
                ..Default::default()
            }),
            ..Default::default()
        };
        let url = link("v", &contact);
        assert_eq!(parse(&url), Some(Parsed::Contact(contact.clone())));
        // Padded, in standard base64, in capitals, without the scheme.
        let data = STANDARD.encode(contact.encode_to_vec());
        assert!(parse(&format!("MESHTASTIC.ORG/v/#{data}")).is_some());
        assert!(parse("https://example.org/v/#CAE").is_none());
        assert!(parse("https://meshtastic.org/v/#!!!").is_none());
        match contents(&parse(&url).unwrap(), &radio(&[]), &BTreeMap::new()) {
            LinkContents::Contact { id, key, known, .. } => {
                assert_eq!((id.as_str(), key, known), ("!1234abcd", true, false));
            }
            other => panic!("a contact, not {other:?}"),
        }
    }

    #[test]
    fn channels_go_to_free_slots_but_those_the_radio_has() {
        let set = ChannelSet {
            settings: vec![
                // The radio's primary: unnamed, the default key.
                pb::ChannelSettings {
                    psk: vec![1],
                    ..Default::default()
                },
                pb::ChannelSettings {
                    name: "Galicia".into(),
                    psk: vec![5; 32],
                    ..Default::default()
                },
                pb::ChannelSettings {
                    name: "Abierto".into(),
                    psk: vec![],
                    ..Default::default()
                },
            ],
            lora_config: Some(pb::config::LoRaConfig {
                use_preset: true,
                modem_preset: pb::config::lo_ra_config::ModemPreset::MediumFast as i32,
                ..Default::default()
            }),
        };
        assert!(matches!(
            parse(&link("e", &set)),
            Some(Parsed::Channels(_, true))
        ));
        let parsed = parse(&link("e/?add=true", &set)).unwrap();
        assert!(matches!(parsed, Parsed::Channels(_, false)));
        let radio = radio(&[(0, "", &[1]), (1, "Lab", &[9; 32])]);
        let LinkContents::Channels {
            channels,
            free_slots,
            preset,
            lora_differs,
            removed,
            lora: read_lora,
            ..
        } = contents(&parsed, &radio, &BTreeMap::new())
        else {
            panic!("channels");
        };
        // Replacing would take Lab, and the primary, which the link names
        // by its own preset.
        assert_eq!(removed, ["LongFast", "Lab"]);
        let replacing = replacing_changes(&set);
        assert_eq!(replacing.len(), 8);
        assert_eq!(
            (replacing[0].role.as_str(), replacing[0].name.as_str()),
            ("PRIMARY", "")
        );
        assert_eq!(replacing[1].name, "Galicia");
        assert!(replacing[3..].iter().all(|c| c.role == "DISABLED"));
        let lora = lora_settings(&set, &radio).unwrap();
        // Its preset; the radio's region, as the link says none.
        assert_eq!(
            (lora.modem_preset.as_str(), lora.region.as_str()),
            ("MEDIUM_FAST", "EU_868")
        );
        // As read, the same: what the LoRa tab takes from the link.
        assert_eq!(read_lora, Some(lora));
        // Its unnamed one goes by the link's preset, which is not the radio's.
        assert_eq!(channels[0].name, "MediumFast");
        assert!(!channels[0].present);
        assert_eq!(channels[1].key, ChannelKey::Aes256);
        assert_eq!(
            (free_slots, preset.as_deref(), lora_differs),
            (6, Some("MediumFast"), true)
        );
        let changes = channel_changes(&set, &radio).unwrap();
        assert_eq!(
            changes.iter().map(|c| c.index).collect::<Vec<_>>(),
            [2, 3, 4]
        );
        assert_eq!(changes[1].key, KeyChange::Custom);
        assert_eq!(changes[2].key, KeyChange::None);
        // Added, again: nothing.
        let mut after = radio.clone();
        for change in &changes {
            let psk = set.settings[(change.index - 2) as usize].psk.clone();
            after.channels.insert(
                change.index,
                pb::Channel {
                    index: change.index as i32,
                    role: channel::Role::Secondary as i32,
                    settings: Some(pb::ChannelSettings {
                        name: change.name.clone(),
                        psk,
                        ..Default::default()
                    }),
                },
            );
        }
        assert_eq!(channel_changes(&set, &after).unwrap(), []);
        // More than there is room for: none.
        let full = radio_full();
        assert!(channel_changes(&set, &full).is_none());
    }

    fn radio_full() -> Settings {
        radio(&[
            (0, "", &[1]),
            (1, "a", &[2; 16]),
            (2, "b", &[3; 16]),
            (3, "c", &[4; 16]),
            (4, "d", &[5; 16]),
            (5, "e", &[6; 16]),
            (6, "f", &[7; 16]),
        ])
    }
}
