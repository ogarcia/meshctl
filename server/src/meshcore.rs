// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A MeshCore radio's own settings and figures, between what its driver
//! keeps and what the API shows: checked here as the firmware checks them,
//! so that what it would refuse is said before it is sent.

use meshctl_api::{
    MeshcoreAutoAdd, MeshcoreFrequencies, MeshcorePosition, MeshcoreSettings, MeshcoreStatus,
    MeshcoreTelemetry, MeshcoreTuning, TelemetryShare,
};
use meshctl_core::meshcore::Settings;
use meshctl_core::meshcore::protocol::Stats;

/// The auto-add bits, as the firmware has them.
const OVERWRITE_OLDEST: u8 = 1 << 0;
const COMPANIONS: u8 = 1 << 1;
const REPEATERS: u8 = 1 << 2;
const ROOMS: u8 = 1 << 3;
const SENSORS: u8 = 1 << 4;

fn auto_add((config, max_hops): (u8, u8)) -> MeshcoreAutoAdd {
    MeshcoreAutoAdd {
        companions: config & COMPANIONS != 0,
        repeaters: config & REPEATERS != 0,
        rooms: config & ROOMS != 0,
        sensors: config & SENSORS != 0,
        overwrite_oldest: config & OVERWRITE_OLDEST != 0,
        // The firmware counts one more: 1 is straight, 0 any.
        max_hops: max_hops.checked_sub(1),
    }
}

fn auto_add_bits(add: &MeshcoreAutoAdd) -> (u8, u8) {
    let bit = |on: bool, bit: u8| if on { bit } else { 0 };
    (
        bit(add.companions, COMPANIONS)
            | bit(add.repeaters, REPEATERS)
            | bit(add.rooms, ROOMS)
            | bit(add.sensors, SENSORS)
            | bit(add.overwrite_oldest, OVERWRITE_OLDEST),
        add.max_hops.map_or(0, |hops| hops + 1),
    )
}

/// Thousandths, as the radio takes them.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn thousandths(value: f64) -> u32 {
    (value * 1000.0).round() as u32
}

fn share(bits: u8) -> TelemetryShare {
    match bits & 0x03 {
        0 => TelemetryShare::Nobody,
        1 => TelemetryShare::Allowed,
        _ => TelemetryShare::Anyone,
    }
}

fn bits(share: TelemetryShare) -> u8 {
    match share {
        TelemetryShare::Nobody => 0,
        TelemetryShare::Allowed => 1,
        TelemetryShare::Anyone => 2,
    }
}

/// The settings as the API shows them.
pub fn view(settings: &Settings) -> MeshcoreSettings {
    MeshcoreSettings {
        name: settings.name.clone(),
        position: (settings.lat != 0 || settings.lon != 0).then(|| MeshcorePosition {
            latitude: f64::from(settings.lat) / 1e6,
            longitude: f64::from(settings.lon) / 1e6,
        }),
        frequency: settings.frequency,
        bandwidth: settings.bandwidth,
        spreading_factor: settings.spreading_factor,
        coding_rate: settings.coding_rate,
        tx_power: settings.tx_power,
        max_tx_power: settings.max_tx_power,
        repeat: settings.repeat,
        manual_add_contacts: settings.manual_add,
        share_position: settings.location_policy != 0,
        telemetry: MeshcoreTelemetry {
            base: share(settings.telemetry),
            location: share(settings.telemetry >> 2),
            environment: share(settings.telemetry >> 4),
        },
        multi_acks: settings.multi_acks != 0,
        path_hash_bytes: settings.path_hash_mode.map(|mode| mode + 1),
        default_scope: settings
            .default_scope
            .as_ref()
            .and_then(|scope| scope.as_ref().map(|(name, _)| name.clone())),
        scopes: settings.default_scope.is_some(),
        auto_add: settings.autoadd.map(auto_add),
        tuning: settings
            .tuning
            .map(|(rx_delay, airtime_factor)| MeshcoreTuning {
                rx_delay: f64::from(rx_delay) / 1000.0,
                airtime_factor: f64::from(airtime_factor) / 1000.0,
            }),
        repeat_frequencies: settings
            .repeat_frequencies
            .iter()
            .map(|&(from, to)| MeshcoreFrequencies { from, to })
            .collect(),
    }
}

/// Millionths of a degree, as the radio keeps them.
#[allow(clippy::cast_possible_truncation)]
fn micro(degrees: f64) -> i32 {
    (degrees * 1e6).round() as i32
}

/// The settings as they are to be, from the radio's and what is wanted;
/// which one is not one the radio can have, if any.
pub fn change(current: &Settings, wanted: &MeshcoreSettings) -> Result<Settings, String> {
    let check = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_owned()) };
    let name = wanted.name.trim();
    check(!name.is_empty() && name.len() <= 31, "name")?;
    let (lat, lon) = match wanted.position {
        Some(p) => {
            check(
                (-90.0..=90.0).contains(&p.latitude) && (-180.0..=180.0).contains(&p.longitude),
                "position",
            )?;
            (micro(p.latitude), micro(p.longitude))
        }
        None => (0, 0),
    };
    check(
        (150_000..=2_500_000).contains(&wanted.frequency),
        "frequency",
    )?;
    check((7_000..=500_000).contains(&wanted.bandwidth), "bandwidth")?;
    check(
        (5..=12).contains(&wanted.spreading_factor),
        "spreading_factor",
    )?;
    check((5..=8).contains(&wanted.coding_rate), "coding_rate")?;
    check(
        (-9..=i16::from(current.max_tx_power)).contains(&i16::from(wanted.tx_power)),
        "tx_power",
    )?;
    // Repeat only where the firmware has it, and on its frequencies.
    let repeat = match (current.repeat, wanted.repeat) {
        (None, _) => None,
        (Some(now), None) => Some(now),
        (Some(_), Some(wanted_repeat)) => Some(wanted_repeat),
    };
    check(
        repeat != Some(true)
            || current
                .repeat_frequencies
                .iter()
                .any(|&(from, to)| (from..=to).contains(&wanted.frequency)),
        "repeat",
    )?;
    // Each of the rest only where the firmware has it.
    let path_hash_mode = match (current.path_hash_mode, wanted.path_hash_bytes) {
        (Some(_), Some(bytes)) => {
            check((1..=3).contains(&bytes), "path_hash_bytes")?;
            Some(bytes - 1)
        }
        (now, _) => now,
    };
    let tuning = match (current.tuning, wanted.tuning) {
        (Some(_), Some(t)) => {
            check(
                (0.0..=20.0).contains(&t.rx_delay) && (0.0..=9.0).contains(&t.airtime_factor),
                "tuning",
            )?;
            Some((thousandths(t.rx_delay), thousandths(t.airtime_factor)))
        }
        (now, _) => now,
    };
    let autoadd = match (current.autoadd, wanted.auto_add) {
        (Some(_), Some(add)) => {
            check(add.max_hops.is_none_or(|hops| hops <= 63), "auto_add")?;
            Some(auto_add_bits(&add))
        }
        (now, _) => now,
    };
    let default_scope = match &current.default_scope {
        None => None,
        Some(now) => {
            let name = wanted.default_scope.as_deref().map(str::trim).unwrap_or("");
            let name = name.strip_prefix('#').unwrap_or(name).trim();
            check(name.len() <= 30, "default_scope")?;
            Some(match now {
                // The same region keeps its key, its own if it had one.
                Some((was, key)) if was == name => Some((was.clone(), *key)),
                _ if name.is_empty() => None,
                _ => Some((name.to_owned(), hashtag_secret(name))),
            })
        }
    };
    Ok(Settings {
        name: name.to_owned(),
        lat,
        lon,
        frequency: wanted.frequency,
        bandwidth: wanted.bandwidth,
        spreading_factor: wanted.spreading_factor,
        coding_rate: wanted.coding_rate,
        tx_power: wanted.tx_power,
        max_tx_power: current.max_tx_power,
        repeat,
        manual_add: wanted.manual_add_contacts,
        telemetry: bits(wanted.telemetry.base)
            | (bits(wanted.telemetry.location) << 2)
            | (bits(wanted.telemetry.environment) << 4),
        location_policy: u8::from(wanted.share_position),
        multi_acks: u8::from(wanted.multi_acks),
        path_hash_mode,
        tuning,
        autoadd,
        repeat_frequencies: current.repeat_frequencies.clone(),
        default_scope,
    })
}

/// The figures as the API shows them.
pub fn status(status: meshctl_core::meshcore::Status) -> MeshcoreStatus {
    let mut shown = MeshcoreStatus {
        battery_mv: status.battery_mv,
        storage_used_kb: status.storage_used_kb,
        storage_total_kb: status.storage_total_kb,
        ..MeshcoreStatus::default()
    };
    if let Some(Stats::Core {
        battery_mv,
        uptime_secs,
        queue,
        ..
    }) = status.core
    {
        shown.battery_mv = shown.battery_mv.or(Some(battery_mv));
        shown.uptime = Some(uptime_secs);
        shown.queue = Some(queue);
    }
    if let Some(Stats::Radio {
        noise_floor,
        last_rssi,
        last_snr,
        tx_air_secs,
        rx_air_secs,
    }) = status.radio
    {
        shown.noise_floor = Some(noise_floor);
        shown.last_rssi = Some(last_rssi);
        shown.last_snr = Some(last_snr);
        shown.tx_air_secs = Some(tx_air_secs);
        shown.rx_air_secs = Some(rx_air_secs);
    }
    if let Some(Stats::Packets {
        received,
        sent,
        sent_flood,
        sent_direct,
        received_flood,
        received_direct,
        receive_errors,
    }) = status.packets
    {
        shown.packets_received = Some(received);
        shown.packets_sent = Some(sent);
        shown.sent_flood = Some(sent_flood);
        shown.sent_direct = Some(sent_direct);
        shown.received_flood = Some(received_flood);
        shown.received_direct = Some(received_direct);
        shown.receive_errors = receive_errors;
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radio() -> Settings {
        Settings {
            name: "Casa".into(),
            lat: 43_362_300,
            lon: -8_411_500,
            frequency: 869_618,
            bandwidth: 62_500,
            spreading_factor: 8,
            coding_rate: 8,
            tx_power: 22,
            max_tx_power: 22,
            repeat: Some(false),
            manual_add: false,
            telemetry: 0b10_01_00,
            location_policy: 1,
            multi_acks: 0,
            path_hash_mode: Some(1),
            tuning: Some((0, 1000)),
            autoadd: Some((31, 0)),
            repeat_frequencies: vec![(433_000, 433_000), (869_495, 869_495)],
            default_scope: Some(None),
        }
    }

    #[test]
    fn settings_read_back_as_they_were() {
        let shown = view(&radio());
        assert_eq!(
            shown.telemetry,
            MeshcoreTelemetry {
                base: TelemetryShare::Nobody,
                location: TelemetryShare::Allowed,
                environment: TelemetryShare::Anyone
            }
        );
        assert!(shown.share_position);
        assert_eq!(change(&radio(), &shown), Ok(radio()));
    }

    #[test]
    fn what_the_firmware_refuses_is_refused_first() {
        let shown = view(&radio());
        let with = |edit: fn(&mut MeshcoreSettings)| {
            let mut wanted = shown.clone();
            edit(&mut wanted);
            change(&radio(), &wanted)
        };
        assert_eq!(with(|w| w.name = " ".into()), Err("name".into()));
        assert_eq!(with(|w| w.tx_power = 23), Err("tx_power".into()));
        assert_eq!(
            with(|w| w.spreading_factor = 13),
            Err("spreading_factor".into())
        );
        // Repeat only on its frequencies.
        assert_eq!(with(|w| w.repeat = Some(true)), Err("repeat".into()));
        assert_eq!(
            with(|w| w.path_hash_bytes = Some(4)),
            Err("path_hash_bytes".into())
        );
        assert_eq!(
            with(|w| w.tuning = Some(MeshcoreTuning {
                rx_delay: 21.0,
                airtime_factor: 1.0
            })),
            Err("tuning".into())
        );
        assert_eq!(
            with(|w| w.default_scope = Some("a".repeat(31))),
            Err("default_scope".into())
        );
        assert!(
            with(|w| {
                w.repeat = Some(true);
                w.frequency = 869_495;
            })
            .is_ok()
        );
        // A scope by its name, with or without `#`; its key the name's.
        let scoped = with(|w| w.default_scope = Some("#galicia".into())).unwrap();
        assert_eq!(
            scoped.default_scope,
            Some(Some(("galicia".into(), hashtag_secret("galicia"))))
        );
        assert_eq!(view(&scoped).default_scope.as_deref(), Some("galicia"));
        // Auto-add as the firmware counts: 0 straight is its 1.
        let mut add = view(&radio()).auto_add.unwrap();
        assert_eq!(add.max_hops, None);
        assert!(add.overwrite_oldest && add.repeaters);
        add.max_hops = Some(0);
        add.sensors = false;
        let mut wanted = view(&radio());
        wanted.auto_add = Some(add);
        let added = change(&radio(), &wanted);
        assert_eq!(added.map(|s| s.autoadd), Ok(Some((15, 1))));
        // Nowhere is no position at all.
        assert_eq!(
            with(|w| w.position = None).map(|s| (s.lat, s.lon)),
            Ok((0, 0))
        );
    }
}

/// What a MeshCore link holds: `meshcore://contact/add` or
/// `meshcore://channel/add`, as the apps share them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Contact {
        key: [u8; 32],
        /// 1 a companion, 2 a repeater, 3 a room, 4 a sensor.
        kind: u8,
        name: String,
    },
    Channel {
        name: String,
        secret: [u8; 16],
    },
}

/// Bytes from hex, exactly `N` of them.
fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads a MeshCore link, whatever the case of its scheme. None for anything
/// else, or one missing what it needs.
pub fn parse_link(url: &str) -> Option<Link> {
    let url = url.trim();
    let scheme = "meshcore://";
    let rest = url
        .get(..scheme.len())
        .filter(|s| s.eq_ignore_ascii_case(scheme))
        .map(|_| &url[scheme.len()..])?;
    let (path, query) = rest.split_once('?')?;
    let field = |name: &str| {
        form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    match path.trim_end_matches('/').to_ascii_lowercase().as_str() {
        "contact/add" => {
            let key = unhex::<32>(&field("public_key")?)?;
            let kind = field("type")
                .and_then(|t| t.parse::<u8>().ok())
                .filter(|k| (1..=4).contains(k))
                .unwrap_or(1);
            let name = field("name").unwrap_or_default().trim().to_owned();
            (!name.is_empty()).then_some(Link::Contact { key, kind, name })
        }
        "channel/add" => {
            let secret = unhex::<16>(&field("secret")?)?;
            let name = stored_name(&field("name").unwrap_or_default(), &secret);
            (!name.is_empty() && name.len() <= 31).then_some(Link::Channel { name, secret })
        }
        _ => None,
    }
}

/// The node number a key stands for, as the driver gives it.
fn num_of(key: &[u8; 32]) -> u32 {
    u32::from_be_bytes([key[0], key[1], key[2], key[3]])
}

/// The secret the apps give a `#name` channel: the start of the SHA-256
/// of its name, `#` and all, whether it is written with it or not.
pub fn hashtag_secret(name: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let bare = name.trim();
    let bare = bare.strip_prefix('#').unwrap_or(bare);
    let digest = Sha256::digest(format!("#{bare}").as_bytes());
    let mut secret = [0u8; 16];
    secret.copy_from_slice(&digest[..16]);
    secret
}

/// A channel's name as the radio keeps it: a `#name`'s without its `#`, as
/// the apps keep them, which its secret tells; any other as it is.
pub fn stored_name(name: &str, secret: &[u8; 16]) -> String {
    let name = name.trim();
    match name.strip_prefix('#') {
        Some(bare) if *secret == hashtag_secret(name) => bare.trim().to_owned(),
        _ => name.to_owned(),
    }
}

fn kind_of(name: &str, secret: &[u8; 16]) -> meshctl_api::MeshcoreChannelKind {
    use meshctl_api::MeshcoreChannelKind as Kind;
    if *secret == meshctl_core::meshcore::PUBLIC_SECRET {
        Kind::Public
    } else if *secret == hashtag_secret(name) {
        Kind::Hashtag
    } else {
        Kind::Private
    }
}

/// Whether the radio has a channel already, name and secret: a `#name`'s
/// with its `#` or not.
pub fn has_channel(state: &meshctl_core::mesh::MeshState, name: &str, secret: &[u8; 16]) -> bool {
    twin(state, None, name, secret)
}

/// Whether a channel other than the one in a slot is the very same: name,
/// as kept, and secret.
fn twin(
    state: &meshctl_core::mesh::MeshState,
    besides: Option<u32>,
    name: &str,
    secret: &[u8; 16],
) -> bool {
    let Some(meshcore) = &state.meshcore_channels else {
        return false;
    };
    let name = stored_name(name, secret);
    state.channels.values().any(|c| {
        Some(c.index) != besides
            && meshcore.secrets.get(&c.index) == Some(secret)
            && stored_name(&c.name, secret) == name
    })
}

/// The radio's first slot with no channel.
pub fn free_slot(state: &meshctl_core::mesh::MeshState) -> Option<u8> {
    let slots = state.meshcore_channels.as_ref()?.slots;
    (0..slots).find(|index| !state.channels.contains_key(&u32::from(*index)))
}

fn free_slots(state: &meshctl_core::mesh::MeshState) -> u32 {
    let slots = state.meshcore_channels.as_ref().map_or(0, |c| c.slots);
    (0..slots)
        .filter(|index| !state.channels.contains_key(&u32::from(*index)))
        .count()
        .try_into()
        .unwrap_or_default()
}

/// What a MeshCore link holds, against what the radio has and knows.
pub fn link_contents(
    link: &Link,
    state: &meshctl_core::mesh::MeshState,
) -> meshctl_api::LinkContents {
    match link {
        Link::Contact { key, name, .. } => {
            let node = num_of(key);
            meshctl_api::LinkContents::Contact {
                node,
                id: format!("!{node:08x}"),
                long_name: name.clone(),
                short_name: crate::meshcore::initials(name),
                hw_model: None,
                key: true,
                known: state.nodes.contains_key(&node),
            }
        }
        Link::Channel { name, secret } => meshctl_api::LinkContents::Channels {
            channels: vec![meshctl_api::LinkChannel {
                name: name.clone(),
                key: meshctl_api::ChannelKey::Aes128,
                present: has_channel(state, name, secret),
            }],
            free_slots: free_slots(state),
            preset: None,
            lora_differs: false,
            region: None,
            replaces: false,
            removed: Vec::new(),
        },
    }
}

/// What a radio's repeaters said of their regions, gathered: each region
/// with how many serve it and how many of those are straight, the most
/// served nearby first; the repeaters nearest first.
pub fn gather_regions(
    left: Option<u32>,
    mut repeaters: Vec<meshctl_api::MeshcoreRepeaterRegions>,
) -> meshctl_api::MeshcoreRegions {
    use std::collections::BTreeMap;
    let mut regions: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let (mut unscoped, mut unscoped_direct) = (0, 0);
    for repeater in &repeaters {
        let direct = u32::from(repeater.hops == Some(0));
        for name in &repeater.regions {
            let served = regions.entry(name.clone()).or_default();
            served.0 += 1;
            served.1 += direct;
        }
        if repeater.unscoped {
            unscoped += 1;
            unscoped_direct += direct;
        }
    }
    let mut regions: Vec<meshctl_api::MeshcoreRegion> = regions
        .into_iter()
        .map(|(name, (repeaters, direct))| meshctl_api::MeshcoreRegion {
            name,
            repeaters,
            direct,
        })
        .collect();
    regions.sort_by(|a, b| {
        (b.direct, b.repeaters)
            .cmp(&(a.direct, a.repeaters))
            .then_with(|| a.name.cmp(&b.name))
    });
    repeaters.sort_by_key(|r| (r.hops.unwrap_or(u32::MAX), std::cmp::Reverse(r.answered_at)));
    meshctl_api::MeshcoreRegions {
        searching: left.is_some(),
        left: left.unwrap_or(0),
        regions,
        unscoped,
        unscoped_direct,
        repeaters,
    }
}

/// A name's first letters, as a node with no short name shows.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().and_then(|w| w.chars().next());
    let second = words.next().and_then(|w| w.chars().next());
    first
        .into_iter()
        .chain(second)
        .flat_map(char::to_uppercase)
        .collect()
}

/// A MeshCore radio's channels, each by its kind.
pub fn channels(state: &meshctl_core::mesh::MeshState) -> Option<meshctl_api::MeshcoreChannels> {
    let meshcore = state.meshcore_channels.as_ref()?;
    Some(meshctl_api::MeshcoreChannels {
        slots: u32::from(meshcore.slots),
        channels: state
            .channels
            .values()
            .map(|c| meshctl_api::MeshcoreChannel {
                index: c.index,
                name: c.name.clone(),
                kind: meshcore
                    .secrets
                    .get(&c.index)
                    .map_or(meshctl_api::MeshcoreChannelKind::Private, |s| {
                        kind_of(&c.name, s)
                    }),
            })
            .collect(),
    })
}

/// Why a channel cannot be: the very same as another in another slot.
pub const DUPLICATE: &str = "duplicate";

/// A channel as it is to be in a slot: its name, and its secret as given,
/// as its name gives it, as it was if it keeps its name, or `random`'s.
pub fn channel_change(
    state: &meshctl_core::mesh::MeshState,
    index: u32,
    wanted: &meshctl_api::MeshcoreChannelChange,
    random: impl Fn(&mut [u8; 16]),
) -> Result<(String, [u8; 16]), &'static str> {
    let meshcore = state.meshcore_channels.as_ref().ok_or("radio")?;
    if index >= u32::from(meshcore.slots) {
        return Err("index");
    }
    let name = wanted.name.trim();
    if name.is_empty() || name.len() > 31 {
        return Err("name");
    }
    let kept = state
        .channels
        .get(&index)
        .filter(|c| c.name == name)
        .and_then(|_| meshcore.secrets.get(&index).copied());
    let secret = match &wanted.secret {
        Some(text) => unhex::<16>(text).ok_or("secret")?,
        None if name.starts_with('#') => hashtag_secret(name),
        None => kept.unwrap_or_else(|| {
            let mut secret = [0u8; 16];
            random(&mut secret);
            secret
        }),
    };
    let name = stored_name(name, &secret);
    if name.is_empty() {
        return Err("name");
    }
    if twin(state, Some(index), &name, &secret) {
        return Err(DUPLICATE);
    }
    Ok((name, secret))
}

/// A channel's link, as the apps read it: its name and secret.
pub fn channel_link(state: &meshctl_core::mesh::MeshState, index: u32) -> Option<String> {
    let channel = state.channels.get(&index)?;
    let secret = state.meshcore_channels.as_ref()?.secrets.get(&index)?;
    let name: String = form_urlencoded::byte_serialize(channel.name.as_bytes()).collect();
    Some(format!(
        "meshcore://channel/add?name={name}&secret={}",
        hex(secret)
    ))
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn links_read_as_the_apps_share_them() {
        let key = "50".repeat(32);
        assert_eq!(
            parse_link(&format!(
                "MeshCore://contact/add?name=Faro+Norte&public_key={key}&type=2"
            )),
            Some(Link::Contact {
                key: [0x50; 32],
                kind: 2,
                name: "Faro Norte".into()
            })
        );
        assert_eq!(
            parse_link(
                "meshcore://channel/add?name=%23galicia&secret=00112233445566778899aabbccddeeff"
            ),
            Some(Link::Channel {
                name: "#galicia".into(),
                secret: unhex("00112233445566778899aabbccddeeff").unwrap()
            })
        );
        // A #name's, kept without its `#` as the apps keep it.
        assert_eq!(
            parse_link(
                "meshcore://channel/add?name=%23galicia&secret=840c972239a5c781de274e011ff18159"
            ),
            Some(Link::Channel {
                name: "galicia".into(),
                secret: hashtag_secret("#galicia")
            })
        );
        // A short key, no name, or a Meshtastic link: nothing.
        assert_eq!(
            parse_link("meshcore://contact/add?name=X&public_key=50"),
            None
        );
        assert_eq!(
            parse_link(&format!("meshcore://contact/add?public_key={key}")),
            None
        );
        assert_eq!(parse_link("https://meshtastic.org/e/#CgE"), None);
    }

    #[test]
    fn regions_are_gathered_the_most_served_nearby_first() {
        let repeater =
            |node, hops, regions: &[&str], unscoped| meshctl_api::MeshcoreRepeaterRegions {
                node,
                name: None,
                hops,
                regions: regions.iter().map(|r| (*r).to_owned()).collect(),
                unscoped,
                answered_at: 10,
            };
        let gathered = gather_regions(
            Some(2),
            vec![
                repeater(1, Some(2), &["europe", "coruna"], false),
                repeater(2, Some(0), &["europe", "galicia"], true),
                repeater(3, None, &["coruna"], true),
            ],
        );
        assert!(gathered.searching);
        assert_eq!(gathered.left, 2);
        let names: Vec<_> = gathered
            .regions
            .iter()
            .map(|r| (r.name.as_str(), r.repeaters, r.direct))
            .collect();
        assert_eq!(
            names,
            [("europe", 2, 1), ("galicia", 1, 1), ("coruna", 2, 0)]
        );
        assert_eq!((gathered.unscoped, gathered.unscoped_direct), (2, 1));
        assert_eq!(
            gathered
                .repeaters
                .iter()
                .map(|r| r.node)
                .collect::<Vec<_>>(),
            [2, 1, 3]
        );
    }

    #[test]
    fn a_channel_is_told_by_its_secret() {
        use meshctl_api::MeshcoreChannelKind as Kind;
        assert_eq!(
            kind_of("Public", &meshctl_core::meshcore::PUBLIC_SECRET),
            Kind::Public
        );
        assert_eq!(
            kind_of("#galicia", &hashtag_secret("#galicia")),
            Kind::Hashtag
        );
        // As the apps keep it, and as a real radio has it.
        assert_eq!(
            hashtag_secret("galicia"),
            unhex("840c972239a5c781de274e011ff18159").unwrap()
        );
        assert_eq!(
            kind_of("galicia", &hashtag_secret("#galicia")),
            Kind::Hashtag
        );
        assert_eq!(kind_of("#galicia", &[1; 16]), Kind::Private);
        assert_eq!(
            stored_name("#galicia", &hashtag_secret("galicia")),
            "galicia"
        );
        assert_eq!(stored_name("#galicia", &[1; 16]), "#galicia");
    }
}
