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
