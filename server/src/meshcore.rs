// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A MeshCore radio's own settings and figures, between what its driver
//! keeps and what the API shows: checked here as the firmware checks them,
//! so that what it would refuse is said before it is sent.

use meshctl_api::{
    MeshcorePosition, MeshcoreSettings, MeshcoreStatus, MeshcoreTelemetry, TelemetryShare,
};
use meshctl_core::meshcore::Settings;
use meshctl_core::meshcore::protocol::Stats;

/// The frequencies, in kHz, on which the firmware lets a client repeat.
const REPEAT_FREQUENCIES: [u32; 3] = [433_000, 869_495, 918_000];

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
        repeat != Some(true) || REPEAT_FREQUENCIES.contains(&wanted.frequency),
        "repeat",
    )?;
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
        assert!(
            with(|w| {
                w.repeat = Some(true);
                w.frequency = 869_495;
            })
            .is_ok()
        );
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
            let name = field("name").unwrap_or_default().trim().to_owned();
            (!name.is_empty() && name.len() <= 31).then_some(Link::Channel { name, secret })
        }
        _ => None,
    }
}

/// The node number a key stands for, as the driver gives it.
fn num_of(key: &[u8; 32]) -> u32 {
    u32::from_be_bytes([key[0], key[1], key[2], key[3]])
}

/// The secret the apps give a `#name` channel: the start of its name's
/// SHA-256, `#` and all.
pub fn hashtag_secret(name: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(name.as_bytes());
    let mut secret = [0u8; 16];
    secret.copy_from_slice(&digest[..16]);
    secret
}

fn kind_of(name: &str, secret: &[u8; 16]) -> meshctl_api::MeshcoreChannelKind {
    use meshctl_api::MeshcoreChannelKind as Kind;
    if *secret == meshctl_core::meshcore::PUBLIC_SECRET {
        Kind::Public
    } else if name.starts_with('#') && *secret == hashtag_secret(name) {
        Kind::Hashtag
    } else {
        Kind::Private
    }
}

/// Whether the radio has a channel already, name and secret.
pub fn has_channel(state: &meshctl_core::mesh::MeshState, name: &str, secret: &[u8; 16]) -> bool {
    let Some(meshcore) = &state.meshcore_channels else {
        return false;
    };
    state
        .channels
        .values()
        .any(|c| c.name == name && meshcore.secrets.get(&c.index) == Some(secret))
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
    let twin = state.channels.values().any(|c| {
        c.index != index && c.name == name && meshcore.secrets.get(&c.index) == Some(&secret)
    });
    if twin {
        return Err(DUPLICATE);
    }
    Ok((name.to_owned(), secret))
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
        assert_eq!(kind_of("#galicia", &[1; 16]), Kind::Private);
    }
}
