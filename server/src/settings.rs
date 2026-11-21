// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's own settings, between what the radio sent and what the API
//! shows and takes. What is shown leaves the secrets out; what is changed is
//! built on what the radio has, so that whatever the API does not cover goes
//! back exactly as it was.

use crate::lora;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use meshctl_api::{
    ChannelChange, ChannelKey, ChannelSecret, ChannelSettings, DeviceSettings, KeyChange,
    LoraAdvanced, LoraParams, LoraSettings, NodeSettings, NodeSettingsChange, OwnerSettings,
    PUBLIC_POSITION_PRECISION, RegionLimits, SettingsChoices,
};
use meshctl_core::mesh::Settings;
use meshctl_core::radio::SettingsChange;
use meshctl_proto::meshtastic::{self as pb, config};

/// The most bytes each name takes, as the firmware keeps them.
const LONG_NAME_MAX: usize = 39;
const SHORT_NAME_MAX: usize = 4;
const CHANNEL_NAME_MAX: usize = 11;

/// The slots a radio has for channels.
const CHANNEL_SLOTS: u32 = 8;

const HOP_LIMIT_MAX: u32 = 7;

/// Every name an enum of the protobufs has, in the order of its values.
fn names(name: impl Fn(i32) -> Option<&'static str>) -> Vec<String> {
    (0..=255).filter_map(name).map(str::to_owned).collect()
}

/// Roles the protobufs mark as deprecated: not offered, though a radio that
/// has one still says so.
const DEPRECATED_ROLES: &[&str] = &["ROUTER_CLIENT", "REPEATER"];

fn role_name(value: i32) -> Option<&'static str> {
    config::device_config::Role::try_from(value)
        .ok()
        .map(|r| r.as_str_name())
}

fn region_name(value: i32) -> Option<&'static str> {
    config::lo_ra_config::RegionCode::try_from(value)
        .ok()
        .map(|r| r.as_str_name())
}

fn preset_name(value: i32) -> Option<&'static str> {
    config::lo_ra_config::ModemPreset::try_from(value)
        .ok()
        .map(|p| p.as_str_name())
}

fn channel_role_name(value: i32) -> Option<&'static str> {
    pb::channel::Role::try_from(value)
        .ok()
        .map(|r| r.as_str_name())
}

pub fn choices() -> SettingsChoices {
    // Only those the firmware has.
    let regions: Vec<String> = names(region_name)
        .into_iter()
        .filter(|region| lora::known(region))
        .collect();
    let presets = names(preset_name);
    SettingsChoices {
        roles: names(role_name)
            .into_iter()
            .filter(|role| !DEPRECATED_ROLES.contains(&role.as_str()))
            .collect(),
        presets_by_region: lora::presets_by_region(&regions),
        preset_params: presets
            .iter()
            .map(|preset| {
                let (bandwidth, spread_factor, coding_rate) = lora::params(preset, false);
                let params = LoraParams {
                    bandwidth,
                    spread_factor,
                    coding_rate,
                };
                (preset.clone(), params)
            })
            .collect(),
        bandwidths: lora::BANDWIDTHS.to_vec(),
        wide_bandwidths: lora::WIDE_BANDWIDTHS.to_vec(),
        region_limits: regions
            .iter()
            .map(|code| {
                let region = lora::region(code);
                let limits = RegionLimits {
                    frequency_start: region.start,
                    frequency_end: region.end,
                    duty_cycle: region.duty_cycle,
                    power_limit: region.power_limit,
                    spacing: region.spacing,
                    padding: region.padding,
                    default_slot: region.default_slot,
                };
                (code.clone(), limits)
            })
            .collect(),
        regions,
        presets,
    }
}

/// What kind of key a channel has, from the key itself.
fn key_kind(psk: &[u8]) -> ChannelKey {
    match psk {
        [] | [0] => ChannelKey::None,
        [1] => ChannelKey::Default,
        [_] => ChannelKey::Simple,
        key if key.len() <= 16 => ChannelKey::Aes128,
        _ => ChannelKey::Aes256,
    }
}

/// A channel's key, for the one who asks for it; `None` for a slot there is
/// not.
pub fn secret(settings: &Settings, index: u32) -> Option<ChannelSecret> {
    if index >= CHANNEL_SLOTS {
        return None;
    }
    let psk = settings
        .channels
        .get(&index)
        .and_then(|c| c.settings.as_ref())
        .map_or(&[][..], |s| s.psk.as_slice());
    Some(ChannelSecret {
        index,
        kind: key_kind(psk),
        key: if psk.is_empty() {
            String::new()
        } else {
            BASE64.encode(psk)
        },
    })
}

/// A key of one's own, from its base64: of a simple one's byte, or AES-128
/// or AES-256.
fn own_key(text: Option<&str>) -> Result<Vec<u8>, Invalid> {
    let text: String = text
        .ok_or(Invalid("custom_key"))?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let key = BASE64.decode(text).map_err(|_| Invalid("custom_key"))?;
    check(matches!(key.len(), 1 | 16 | 32), "custom_key")?;
    Ok(key)
}

/// The settings as the API shows them; `None` until the radio has sent them.
pub fn view(settings: &Settings) -> Option<NodeSettings> {
    let owner = settings.owner.as_ref()?;
    let device = settings.config.device.as_ref()?;
    let lora = settings.config.lora.as_ref()?;
    let primary_psk = settings
        .channels
        .get(&0)
        .and_then(|c| c.settings.as_ref())
        .map_or(&[][..], |s| s.psk.as_slice());
    let channels = (0..CHANNEL_SLOTS)
        .map(|index| {
            let channel = settings.channels.get(&index);
            let channel_settings = channel.and_then(|c| c.settings.as_ref());
            let secondary = channel.is_some_and(|c| c.role == pb::channel::Role::Secondary as i32);
            let modules = channel_settings.and_then(|s| s.module_settings.as_ref());
            ChannelSettings {
                index,
                role: channel
                    .and_then(|c| channel_role_name(c.role))
                    .unwrap_or("DISABLED")
                    .to_owned(),
                name: channel_settings.map(|s| s.name.clone()).unwrap_or_default(),
                key: key_kind(channel_settings.map_or(&[][..], |s| s.psk.as_slice())),
                uplink: channel_settings.is_some_and(|s| s.uplink_enabled),
                downlink: channel_settings.is_some_and(|s| s.downlink_enabled),
                position_precision: modules.map_or(0, |m| m.position_precision),
                public: crate::channels::public_key(
                    channel_settings.map_or(&[][..], |s| s.psk.as_slice()),
                    secondary,
                    primary_psk,
                ),
                muted: modules.is_some_and(|m| m.is_muted),
            }
        })
        .collect();
    Some(NodeSettings {
        owner: OwnerSettings {
            long_name: owner.long_name.clone(),
            short_name: owner.short_name.clone(),
        },
        device: DeviceSettings {
            role: role_name(device.role).unwrap_or_default().to_owned(),
        },
        lora: LoraSettings {
            region: region_name(lora.region).unwrap_or_default().to_owned(),
            use_preset: lora.use_preset,
            modem_preset: preset_name(lora.modem_preset)
                .unwrap_or_default()
                .to_owned(),
            hop_limit: lora.hop_limit,
            tx_enabled: lora.tx_enabled,
            bandwidth: lora.bandwidth,
            spread_factor: lora.spread_factor,
            coding_rate: lora.coding_rate,
            advanced: Some(LoraAdvanced {
                tx_power: lora.tx_power,
                frequency_slot: lora.channel_num,
                override_frequency: lora.override_frequency,
                frequency_offset: lora.frequency_offset,
                override_duty_cycle: lora.override_duty_cycle,
                rx_boosted_gain: lora.sx126x_rx_boosted_gain,
                ok_to_mqtt: lora.config_ok_to_mqtt,
                ignore_mqtt: lora.ignore_mqtt,
                pa_fan_disabled: lora.pa_fan_disabled,
            }),
        },
        channels,
        choices: choices(),
    })
}

/// Why a change cannot be made: the API says `invalid_settings` for all of
/// them, and the reference what each setting takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub &'static str);

/// What [`Invalid`] says of a channel the very same as another.
pub const DUPLICATE: &str = "duplicate channel";

/// What [`Invalid`] says of a position more precise than a public channel
/// takes.
pub const PUBLIC_PRECISION: &str = "public channel precision";

fn check(ok: bool, what: &'static str) -> Result<(), Invalid> {
    if ok { Ok(()) } else { Err(Invalid(what)) }
}

/// What the radio is to be sent for a change, built on what it has; and
/// whether it reboots to apply it.
pub fn change(
    settings: &Settings,
    wanted: &NodeSettingsChange,
    random: impl Fn(&mut [u8]),
) -> Result<(SettingsChange, bool), Invalid> {
    let mut change = SettingsChange::default();
    let mut reboots = false;

    if let Some(owner) = &wanted.owner {
        let long_name = owner.long_name.trim();
        let short_name = owner.short_name.trim();
        check(
            !long_name.is_empty() && long_name.len() <= LONG_NAME_MAX,
            "long_name",
        )?;
        check(
            !short_name.is_empty() && short_name.len() <= SHORT_NAME_MAX,
            "short_name",
        )?;
        let mut user = settings.owner.clone().ok_or(Invalid("owner"))?;
        user.long_name = long_name.to_owned();
        user.short_name = short_name.to_owned();
        change.owner = Some(user);
    }

    if let Some(device) = &wanted.device {
        let role =
            config::device_config::Role::from_str_name(&device.role).ok_or(Invalid("role"))?;
        let mut current = settings.config.device.clone().ok_or(Invalid("device"))?;
        if current.role != role as i32 {
            current.role = role as i32;
            change.configs.push(pb::Config {
                payload_variant: Some(config::PayloadVariant::Device(current)),
            });
            reboots = true;
        }
    }

    if let Some(lora) = &wanted.lora {
        let region = config::lo_ra_config::RegionCode::from_str_name(&lora.region)
            .ok_or(Invalid("region"))?;
        let preset = config::lo_ra_config::ModemPreset::from_str_name(&lora.modem_preset)
            .ok_or(Invalid("modem_preset"))?;
        check((1..=HOP_LIMIT_MAX).contains(&lora.hop_limit), "hop_limit")?;
        check(lora::known(&lora.region), "region")?;
        // A preset the region does not take would not be used.
        check(
            !lora.use_preset || lora::presets(&lora.region).contains(&lora.modem_preset.as_str()),
            "modem_preset",
        )?;
        let before = settings.config.lora.clone().ok_or(Invalid("lora"))?;
        let mut current = before.clone();
        current.region = region as i32;
        current.use_preset = lora.use_preset;
        current.modem_preset = preset as i32;
        current.hop_limit = lora.hop_limit;
        current.tx_enabled = lora.tx_enabled;
        // By hand, as the firmware takes them; with a preset, left be.
        if !lora.use_preset {
            check(
                lora::bandwidths(&lora.region).contains(&lora.bandwidth),
                "bandwidth",
            )?;
            check(
                lora::SPREAD_FACTORS.contains(&lora.spread_factor),
                "spread_factor",
            )?;
            check(
                lora::CODING_RATES.contains(&lora.coding_rate),
                "coding_rate",
            )?;
            current.bandwidth = lora.bandwidth;
            current.spread_factor = lora.spread_factor;
            current.coding_rate = lora.coding_rate;
        } else if !before.use_preset {
            // Back to a preset, the coding rate set by hand would raise the
            // preset's (from firmware 2.7.18): 0 is the preset's own.
            current.coding_rate = 0;
        }
        if let Some(advanced) = &lora.advanced {
            // Held to what the radio would keep: past its region's power or
            // band only for a licensed owner, as the firmware lets one.
            let licensed = settings.owner.as_ref().is_some_and(|o| o.is_licensed);
            let region = lora::region(&lora.region);
            check(
                advanced.tx_power >= 0
                    && (licensed || advanced.tx_power <= region.power_limit as i32),
                "tx_power",
            )?;
            let bandwidth = if lora.use_preset {
                lora::params(&lora.modem_preset, lora::wide(&lora.region)).0
            } else {
                lora.bandwidth
            };
            check(
                advanced.override_frequency != 0.0
                    || advanced.frequency_slot <= lora::slots(&lora.region, bandwidth),
                "frequency_slot",
            )?;
            let frequency = f64::from(advanced.override_frequency);
            check(
                frequency == 0.0 || licensed || (region.start..=region.end).contains(&frequency),
                "override_frequency",
            )?;
            check(
                advanced.frequency_offset.is_finite() && advanced.frequency_offset.abs() <= 1.0,
                "frequency_offset",
            )?;
            current.tx_power = advanced.tx_power;
            current.channel_num = advanced.frequency_slot;
            current.override_frequency = advanced.override_frequency;
            current.frequency_offset = advanced.frequency_offset;
            current.override_duty_cycle = advanced.override_duty_cycle;
            current.sx126x_rx_boosted_gain = advanced.rx_boosted_gain;
            current.config_ok_to_mqtt = advanced.ok_to_mqtt;
            current.ignore_mqtt = advanced.ignore_mqtt;
            current.pa_fan_disabled = advanced.pa_fan_disabled;
        }
        if current != before {
            change.configs.push(pb::Config {
                payload_variant: Some(config::PayloadVariant::Lora(current)),
            });
            reboots = true;
        }
    }

    for wanted in &wanted.channels {
        change.channels.push(channel(settings, wanted, &random)?);
    }
    // No channel the very same as another: the radio would take all that is
    // heard on them as the first's.
    if !change.channels.is_empty() {
        let mut after = settings.clone();
        for channel in &change.channels {
            after.channels.insert(channel.index as u32, channel.clone());
        }
        let changed: Vec<u32> = change.channels.iter().map(|c| c.index as u32).collect();
        if crate::channels::duplicate(settings, &after, &changed).is_some() {
            return Err(Invalid(DUPLICATE));
        }
        // Nor a position more precise than a public channel takes, which the
        // radio would cut down without a word to this: on those changed, or
        // on a secondary that takes the primary's key, if that changed.
        let primary_psk = after
            .channels
            .get(&0)
            .and_then(|c| c.settings.as_ref())
            .map_or(Vec::new(), |s| s.psk.clone());
        let too_precise = after.channels.values().any(|channel| {
            let Some(channel_settings) = channel.settings.as_ref() else {
                return false;
            };
            let touched = changed.contains(&(channel.index as u32))
                || (changed.contains(&0) && channel_settings.psk.is_empty());
            let precision = channel_settings
                .module_settings
                .as_ref()
                .map_or(0, |m| m.position_precision);
            touched
                && channel.role != pb::channel::Role::Disabled as i32
                && precision > PUBLIC_POSITION_PRECISION
                && crate::channels::public_key(
                    &channel_settings.psk,
                    channel.role == pb::channel::Role::Secondary as i32,
                    &primary_psk,
                )
        });
        if too_precise {
            return Err(Invalid(PUBLIC_PRECISION));
        }
    }

    Ok((change, reboots))
}

fn channel(
    settings: &Settings,
    wanted: &ChannelChange,
    random: impl Fn(&mut [u8]),
) -> Result<pb::Channel, Invalid> {
    check(wanted.index < CHANNEL_SLOTS, "index")?;
    let role = pb::channel::Role::from_str_name(&wanted.role).ok_or(Invalid("channel role"))?;
    // The primary is slot 0, and only slot 0.
    check(
        (wanted.index == 0) == (role == pb::channel::Role::Primary),
        "channel role",
    )?;
    // Unnamed, the radio names it after its modem preset, as it does the
    // primary: a channel moved off slot 0 keeps that name only so.
    let name = wanted.name.trim();
    check(name.len() <= CHANNEL_NAME_MAX, "channel name")?;

    let mut channel = settings
        .channels
        .get(&wanted.index)
        .cloned()
        .unwrap_or_default();
    channel.index = i32::try_from(wanted.index).unwrap_or_default();
    channel.role = role as i32;
    let channel_settings = channel.settings.get_or_insert_with(Default::default);
    channel_settings.name = name.to_owned();
    channel_settings.uplink_enabled = wanted.uplink;
    channel_settings.downlink_enabled = wanted.downlink;
    if let Some(precision) = wanted.position_precision {
        check(precision <= 32, "position_precision")?;
        channel_settings
            .module_settings
            .get_or_insert_with(Default::default)
            .position_precision = precision;
    }
    if let Some(muted) = wanted.muted {
        channel_settings
            .module_settings
            .get_or_insert_with(Default::default)
            .is_muted = muted;
    }
    channel_settings.psk = match wanted.key {
        KeyChange::Keep => std::mem::take(&mut channel_settings.psk),
        KeyChange::None => vec![0],
        KeyChange::Default => vec![1],
        KeyChange::NewAes128 => fresh(16, &random),
        KeyChange::NewAes256 => fresh(32, &random),
        KeyChange::Custom => own_key(wanted.custom_key.as_deref())?,
    };
    Ok(channel)
}

fn fresh(len: usize, random: impl Fn(&mut [u8])) -> Vec<u8> {
    let mut key = vec![0; len];
    random(&mut key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        let mut settings = Settings {
            owner: Some(pb::User {
                id: "!3134c955".into(),
                long_name: "Casa".into(),
                short_name: "CASA".into(),
                public_key: vec![9; 32],
                ..Default::default()
            }),
            ..Default::default()
        };
        settings.config.device = Some(config::DeviceConfig {
            role: config::device_config::Role::ClientBase as i32,
            node_info_broadcast_secs: 43_200,
            ..Default::default()
        });
        settings.config.lora = Some(config::LoRaConfig {
            region: config::lo_ra_config::RegionCode::EuN868 as i32,
            use_preset: true,
            modem_preset: config::lo_ra_config::ModemPreset::NarrowFast as i32,
            hop_limit: 5,
            tx_enabled: true,
            sx126x_rx_boosted_gain: true,
            ..Default::default()
        });
        settings.channels.insert(
            4,
            pb::Channel {
                index: 4,
                role: pb::channel::Role::Secondary as i32,
                settings: Some(pb::ChannelSettings {
                    name: "Moire".into(),
                    psk: vec![7; 32],
                    ..Default::default()
                }),
            },
        );
        settings
    }

    fn no_random(_: &mut [u8]) {}

    #[test]
    fn what_is_shown_has_no_secrets() {
        let view = view(&settings()).unwrap();
        assert_eq!(view.owner.long_name, "Casa");
        assert_eq!(view.device.role, "CLIENT_BASE");
        assert_eq!(view.lora.modem_preset, "NARROW_FAST");
        assert_eq!(view.channels.len(), 8);
        assert_eq!(view.channels[4].key, ChannelKey::Aes256);
        assert_eq!(view.channels[5].role, "DISABLED");
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("psk") && !json.contains("public_key"));
        assert!(view.choices.roles.contains(&"ROUTER".to_owned()));
    }

    #[test]
    fn nothing_shown_before_the_radio_says() {
        assert!(view(&Settings::default()).is_none());
    }

    #[test]
    fn what_is_not_changed_goes_back_as_it_was() {
        let current = settings();
        let wanted = NodeSettingsChange {
            lora: Some(LoraSettings {
                region: "EU_N_868".into(),
                use_preset: true,
                modem_preset: "NARROW_SLOW".into(),
                hop_limit: 3,
                tx_enabled: true,
                bandwidth: 0,
                spread_factor: 0,
                coding_rate: 0,
                advanced: None,
            }),
            ..Default::default()
        };
        let (change, reboots) = super::change(&current, &wanted, no_random).unwrap();
        assert!(reboots);
        let Some(config::PayloadVariant::Lora(lora)) = &change.configs[0].payload_variant else {
            panic!("a LoRa config");
        };
        assert_eq!(lora.hop_limit, 3);
        assert!(lora.sx126x_rx_boosted_gain, "untouched by the API, kept");
    }

    #[test]
    fn a_radio_is_set_by_hand_or_by_preset() {
        let manual = NodeSettingsChange {
            lora: Some(LoraSettings {
                region: "EU_868".into(),
                use_preset: false,
                modem_preset: "LONG_FAST".into(),
                hop_limit: 3,
                tx_enabled: true,
                bandwidth: 62,
                spread_factor: 9,
                coding_rate: 6,
                advanced: None,
            }),
            ..Default::default()
        };
        let (change, _) = super::change(&settings(), &manual, no_random).unwrap();
        let Some(config::PayloadVariant::Lora(lora)) = &change.configs[0].payload_variant else {
            panic!("a LoRa config");
        };
        assert!(!lora.use_preset);
        assert_eq!(
            (lora.bandwidth, lora.spread_factor, lora.coding_rate),
            (62, 9, 6)
        );
        // With a preset, what was set by hand is left as it was.
        let mut with_preset = manual.clone();
        if let Some(lora) = with_preset.lora.as_mut() {
            lora.use_preset = true;
            lora.bandwidth = 0;
        }
        let (change, _) = super::change(&settings(), &with_preset, no_random).unwrap();
        let Some(config::PayloadVariant::Lora(lora)) = &change.configs[0].payload_variant else {
            panic!("a LoRa config");
        };
        assert_eq!(lora.bandwidth, 0, "the test radio's, untouched");

        // Back to a preset from by hand: the coding rate is the preset's.
        let mut by_hand = settings();
        if let Some(lora) = by_hand.config.lora.as_mut() {
            lora.use_preset = false;
            lora.coding_rate = 8;
        }
        let (change, _) = super::change(&by_hand, &with_preset, no_random).unwrap();
        let Some(config::PayloadVariant::Lora(lora)) = &change.configs[0].payload_variant else {
            panic!("a LoRa config");
        };
        assert_eq!(lora.coding_rate, 0);
    }

    #[test]
    fn what_few_touch_is_held_to_what_the_radio_keeps() {
        let with = |tx_power, frequency_slot, override_frequency| NodeSettingsChange {
            lora: Some(LoraSettings {
                region: "EU_868".into(),
                use_preset: true,
                modem_preset: "LONG_FAST".into(),
                hop_limit: 3,
                tx_enabled: true,
                bandwidth: 0,
                spread_factor: 0,
                coding_rate: 0,
                advanced: Some(LoraAdvanced {
                    tx_power,
                    frequency_slot,
                    override_frequency,
                    frequency_offset: 0.0,
                    override_duty_cycle: false,
                    rx_boosted_gain: true,
                    ok_to_mqtt: true,
                    ignore_mqtt: false,
                    pa_fan_disabled: false,
                }),
            }),
            ..Default::default()
        };
        let apply = |wanted| super::change(&settings(), &wanted, no_random).map(|_| ());
        // EU_868: 27 dBm at most, and one slot at LongFast's 250 kHz.
        assert_eq!(apply(with(27, 1, 0.0)), Ok(()));
        assert_eq!(apply(with(0, 0, 0.0)), Ok(()));
        assert_eq!(apply(with(28, 0, 0.0)), Err(Invalid("tx_power")));
        assert_eq!(apply(with(10, 2, 0.0)), Err(Invalid("frequency_slot")));
        // A frequency of its own: within the band, and the slot then matters not.
        assert_eq!(apply(with(10, 2, 869.5)), Ok(()));
        assert_eq!(
            apply(with(10, 0, 915.0)),
            Err(Invalid("override_frequency"))
        );
        let (change, _) = super::change(&settings(), &with(14, 1, 0.0), no_random).unwrap();
        let Some(config::PayloadVariant::Lora(lora)) = &change.configs[0].payload_variant else {
            panic!("a LoRa config");
        };
        assert_eq!((lora.tx_power, lora.channel_num), (14, 1));
        assert!(lora.config_ok_to_mqtt);
    }

    #[test]
    fn the_same_settings_change_nothing() {
        let current = settings();
        let view = view(&current).unwrap();
        let wanted = NodeSettingsChange {
            device: Some(view.device),
            lora: Some(view.lora),
            ..Default::default()
        };
        let (change, reboots) = super::change(&current, &wanted, no_random).unwrap();
        assert!(change.configs.is_empty());
        assert!(!reboots);
    }

    #[test]
    fn the_owner_keeps_its_key() {
        let wanted = NodeSettingsChange {
            owner: Some(OwnerSettings {
                long_name: " Faro ".into(),
                short_name: "🗼".into(),
            }),
            ..Default::default()
        };
        let (change, reboots) = super::change(&settings(), &wanted, no_random).unwrap();
        let owner = change.owner.unwrap();
        assert_eq!(owner.long_name, "Faro");
        assert_eq!(owner.public_key, vec![9; 32]);
        assert!(!reboots);
    }

    #[test]
    fn a_channel_keeps_or_gets_its_key() {
        let keep = ChannelChange {
            index: 4,
            role: "SECONDARY".into(),
            name: "Moire".into(),
            key: KeyChange::Keep,
            custom_key: None,
            uplink: false,
            downlink: false,
            position_precision: None,
            muted: None,
        };
        let (change, _) = super::change(
            &settings(),
            &NodeSettingsChange {
                channels: vec![keep.clone()],
                ..Default::default()
            },
            no_random,
        )
        .unwrap();
        assert_eq!(
            change.channels[0].settings.as_ref().unwrap().psk,
            vec![7; 32]
        );

        let new_key = ChannelChange {
            index: 5,
            name: "Nuevo".into(),
            key: KeyChange::NewAes128,
            ..keep
        };
        let (change, _) = super::change(
            &settings(),
            &NodeSettingsChange {
                channels: vec![new_key],
                ..Default::default()
            },
            |key| key.fill(3),
        )
        .unwrap();
        assert_eq!(
            change.channels[0].settings.as_ref().unwrap().psk,
            vec![3; 16]
        );
    }

    #[test]
    fn a_channel_shares_its_position_as_told() {
        let moire = |precision| ChannelChange {
            index: 4,
            role: "SECONDARY".into(),
            name: "Moire".into(),
            key: KeyChange::Keep,
            custom_key: None,
            uplink: false,
            downlink: false,
            position_precision: precision,
            muted: None,
        };
        let precision_of = |channel: ChannelChange| {
            let wanted = NodeSettingsChange {
                channels: vec![channel],
                ..Default::default()
            };
            super::change(&settings(), &wanted, no_random).map(|(change, _)| {
                change.channels[0]
                    .settings
                    .as_ref()
                    .unwrap()
                    .module_settings
                    .map(|m| m.position_precision)
            })
        };
        assert_eq!(precision_of(moire(Some(13))), Ok(Some(13)));
        // Not said, as it was: the test radio's Moire has none.
        assert_eq!(precision_of(moire(None)), Ok(None));
        assert!(precision_of(moire(Some(33))).is_err());
    }

    #[test]
    fn a_public_channel_takes_no_precise_position() {
        let moire = |key, precision| ChannelChange {
            index: 4,
            role: "SECONDARY".into(),
            name: "Moire".into(),
            key,
            custom_key: None,
            uplink: false,
            downlink: false,
            position_precision: Some(precision),
            muted: None,
        };
        let apply = |channel| {
            super::change(
                &settings(),
                &NodeSettingsChange {
                    channels: vec![channel],
                    ..Default::default()
                },
                no_random,
            )
            .map(|_| ())
        };
        // Its own key: exactly, if so.
        assert_eq!(apply(moire(KeyChange::Keep, 32)), Ok(()));
        // The default key: some 730 m off at most.
        assert_eq!(
            apply(moire(KeyChange::Default, 32)),
            Err(Invalid(PUBLIC_PRECISION))
        );
        assert_eq!(
            apply(moire(KeyChange::Default, 16)),
            Err(Invalid(PUBLIC_PRECISION))
        );
        assert_eq!(apply(moire(KeyChange::Default, 15)), Ok(()));
        assert_eq!(apply(moire(KeyChange::None, 0)), Ok(()));
    }

    #[test]
    fn a_channel_takes_a_key_of_its_own() {
        let own = |key: &str| ChannelChange {
            index: 4,
            role: "SECONDARY".into(),
            name: "Moire".into(),
            key: KeyChange::Custom,
            custom_key: Some(key.into()),
            uplink: false,
            downlink: false,
            position_precision: None,
            muted: None,
        };
        let apply = |channel| {
            super::change(
                &settings(),
                &NodeSettingsChange {
                    channels: vec![channel],
                    ..Default::default()
                },
                no_random,
            )
        };
        // Sixteen bytes, as the apps show them, spaces and all.
        let (change, _) = apply(own(" 1PG7OiApB1nwvP+rz05pAQ== ")).unwrap();
        let psk = &change.channels[0].settings.as_ref().unwrap().psk;
        assert_eq!(psk.len(), 16);
        assert_eq!(psk[0], 0xd4);
        assert_eq!(
            apply(own("AQ==")).unwrap().0.channels[0]
                .settings
                .as_ref()
                .unwrap()
                .psk,
            vec![1]
        );
        // Not base64, a length no cipher takes, or none at all.
        assert!(apply(own("not a key!")).is_err());
        assert!(apply(own("AAEC")).is_err());
        assert!(
            apply(ChannelChange {
                custom_key: None,
                ..own("")
            })
            .is_err()
        );
    }

    #[test]
    fn a_key_is_handed_out_when_asked_for() {
        let settings = settings();
        let moire = secret(&settings, 4).unwrap();
        assert_eq!(moire.kind, ChannelKey::Aes256);
        assert_eq!(BASE64.decode(&moire.key).unwrap(), vec![7; 32]);
        let empty = secret(&settings, 6).unwrap();
        assert_eq!(empty.kind, ChannelKey::None);
        assert_eq!(empty.key, "");
        assert_eq!(secret(&settings, 8), None);
    }

    #[test]
    fn a_channel_the_same_as_another_is_refused() {
        // Moire again, in another slot, with its very key.
        let same = ChannelChange {
            index: 6,
            role: "SECONDARY".into(),
            name: "Moire".into(),
            key: KeyChange::Custom,
            custom_key: Some("BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".into()),
            uplink: false,
            downlink: false,
            position_precision: None,
            muted: None,
        };
        let wanted = NodeSettingsChange {
            channels: vec![same.clone()],
            ..Default::default()
        };
        assert_eq!(
            super::change(&settings(), &wanted, no_random).err(),
            Some(Invalid(DUPLICATE))
        );
        // Named, it is another channel.
        let named = NodeSettingsChange {
            channels: vec![ChannelChange {
                name: "Otro".into(),
                ..same
            }],
            ..Default::default()
        };
        assert!(super::change(&settings(), &named, no_random).is_ok());
    }

    #[test]
    fn what_the_radio_cannot_have_is_refused() {
        let channel = |index: u32, role: &str, name: &str| ChannelChange {
            index,
            role: role.into(),
            name: name.into(),
            key: KeyChange::Keep,
            custom_key: None,
            uplink: false,
            downlink: false,
            position_precision: None,
            muted: None,
        };
        let lora = |hop_limit, region: &str| LoraSettings {
            region: region.into(),
            use_preset: true,
            modem_preset: "LONG_FAST".into(),
            hop_limit,
            tx_enabled: true,
            bandwidth: 0,
            spread_factor: 0,
            coding_rate: 0,
            advanced: None,
        };
        let by_hand = |bandwidth, spread_factor, coding_rate| LoraSettings {
            use_preset: false,
            bandwidth,
            spread_factor,
            coding_rate,
            ..lora(3, "EU_868")
        };
        let cases = [
            NodeSettingsChange {
                owner: Some(OwnerSettings {
                    long_name: "x".repeat(40),
                    short_name: "X".into(),
                }),
                ..Default::default()
            },
            NodeSettingsChange {
                owner: Some(OwnerSettings {
                    long_name: "Faro".into(),
                    short_name: "FAROS".into(),
                }),
                ..Default::default()
            },
            NodeSettingsChange {
                device: Some(DeviceSettings {
                    role: "PIRATE".into(),
                }),
                ..Default::default()
            },
            NodeSettingsChange {
                lora: Some(lora(8, "EU_868")),
                ..Default::default()
            },
            NodeSettingsChange {
                lora: Some(lora(3, "ATLANTIS")),
                ..Default::default()
            },
            // A preset its region does not take.
            NodeSettingsChange {
                lora: Some(lora(3, "EU_N_868")),
                ..Default::default()
            },
            // By hand: a bandwidth there is not, or 2.4 GHz's below it, a
            // spreading factor or a coding rate out of range.
            NodeSettingsChange {
                lora: Some(by_hand(100, 11, 5)),
                ..Default::default()
            },
            NodeSettingsChange {
                lora: Some(by_hand(800, 11, 5)),
                ..Default::default()
            },
            NodeSettingsChange {
                lora: Some(by_hand(250, 13, 5)),
                ..Default::default()
            },
            NodeSettingsChange {
                lora: Some(by_hand(250, 11, 4)),
                ..Default::default()
            },
            NodeSettingsChange {
                channels: vec![channel(1, "PRIMARY", "Dos")],
                ..Default::default()
            },
            NodeSettingsChange {
                channels: vec![channel(0, "SECONDARY", "Uno")],
                ..Default::default()
            },
            NodeSettingsChange {
                channels: vec![channel(3, "SECONDARY", "Más de once")],
                ..Default::default()
            },
            NodeSettingsChange {
                channels: vec![channel(8, "DISABLED", "")],
                ..Default::default()
            },
            NodeSettingsChange {
                channels: vec![channel(3, "SECONDARY", "Demasiado largo")],
                ..Default::default()
            },
        ];
        for wanted in cases {
            assert!(
                super::change(&settings(), &wanted, no_random).is_err(),
                "{wanted:?}"
            );
        }
    }
}
