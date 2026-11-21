// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How the radio transmits, as the firmware has it: which modem presets each
//! region takes, what each preset stands for, and what can be set by hand
//! instead. Kept as the firmware keeps it, so that nothing is sent that the
//! radio would refuse or change.

use std::collections::BTreeMap;

const STANDARD: &[&str] = &[
    "LONG_FAST",
    "LONG_SLOW",
    "MEDIUM_SLOW",
    "MEDIUM_FAST",
    "SHORT_SLOW",
    "SHORT_FAST",
    "LONG_MODERATE",
    "SHORT_TURBO",
    "LONG_TURBO",
    "MEDIUM_TURBO",
];
const EU_868: &[&str] = &[
    "LONG_FAST",
    "LONG_SLOW",
    "MEDIUM_SLOW",
    "MEDIUM_FAST",
    "SHORT_SLOW",
    "SHORT_FAST",
    "LONG_MODERATE",
];
const LITE: &[&str] = &["LITE_FAST", "LITE_SLOW"];
const NARROW: &[&str] = &["NARROW_FAST", "NARROW_SLOW"];
const TINY: &[&str] = &["TINY_FAST", "TINY_SLOW"];
const UNSET: &[&str] = &["LONG_FAST"];

/// The presets a region takes, its default first.
pub fn presets(region: &str) -> &'static [&'static str] {
    match region {
        "UNSET" => UNSET,
        "EU_868" => EU_868,
        "EU_866" => LITE,
        "EU_N_868" | "ITU2_125CM" | "ITU1_70CM" | "ITU2_70CM" | "ITU3_70CM" => NARROW,
        "ITU1_2M" | "ITU2_2M" | "ITU3_2M" => TINY,
        _ => STANDARD,
    }
}

/// Whether a region is on 2.4 GHz, where bandwidths are wider.
pub fn wide(region: &str) -> bool {
    region == "LORA_24"
}

/// The bandwidths that can be set by hand, by the code the radio keeps:
/// kHz, but for a few that stand for a fraction (31 for 31.25…).
pub const BANDWIDTHS: &[u32] = &[8, 10, 16, 21, 31, 42, 62, 125, 250, 500];
/// On 2.4 GHz.
pub const WIDE_BANDWIDTHS: &[u32] = &[200, 400, 800, 1600];

pub fn bandwidths(region: &str) -> &'static [u32] {
    if wide(region) {
        WIDE_BANDWIDTHS
    } else {
        BANDWIDTHS
    }
}

/// What a preset stands for: bandwidth (as its code), spreading factor and
/// coding rate (the n of 4/n), on 2.4 GHz if `wide`.
pub fn params(preset: &str, wide: bool) -> (u32, u32, u32) {
    let (bandwidth, spread_factor, coding_rate) = match preset {
        "SHORT_TURBO" => (500, 7, 5),
        "SHORT_FAST" => (250, 7, 5),
        "SHORT_SLOW" => (250, 8, 5),
        "MEDIUM_FAST" => (250, 9, 5),
        "MEDIUM_SLOW" => (250, 10, 5),
        "MEDIUM_TURBO" => (500, 9, 5),
        "LONG_TURBO" => (500, 11, 8),
        "LONG_MODERATE" => (125, 11, 8),
        "LONG_SLOW" => (125, 12, 8),
        "LITE_FAST" => (125, 9, 5),
        "LITE_SLOW" => (125, 10, 5),
        "NARROW_FAST" => (62, 7, 6),
        "NARROW_SLOW" => (62, 8, 6),
        "TINY_FAST" => (16, 7, 5),
        "TINY_SLOW" => (16, 8, 6),
        // LONG_FAST, and any other, as the firmware takes them.
        _ => (250, 11, 5),
    };
    // Only the standard ones have a wider bandwidth on 2.4 GHz.
    let bandwidth = match (wide && STANDARD.contains(&preset), bandwidth) {
        (true, 500) => 1600,
        (true, 250) => 800,
        (true, 125) => 400,
        (_, bandwidth) => bandwidth,
    };
    (bandwidth, spread_factor, coding_rate)
}

/// The regions the firmware has: the protobufs name a few more (UA_868,
/// EU_874, EU_917), which it refuses as not known.
const KNOWN: &[&str] = &[
    "UNSET",
    "US",
    "EU_433",
    "EU_868",
    "EU_866",
    "EU_N_868",
    "CN",
    "JP",
    "ANZ",
    "ANZ_433",
    "RU",
    "KR",
    "TW",
    "IN",
    "NZ_865",
    "TH",
    "UA_433",
    "MY_433",
    "MY_919",
    "SG_923",
    "PH_433",
    "PH_868",
    "PH_915",
    "KZ_433",
    "KZ_863",
    "NP_865",
    "BR_902",
    "ITU1_2M",
    "ITU2_2M",
    "ITU3_2M",
    "ITU2_125CM",
    "ITU1_70CM",
    "ITU2_70CM",
    "ITU3_70CM",
    "LORA_24",
];

pub fn known(region: &str) -> bool {
    KNOWN.contains(&region)
}

/// What a region is to the firmware: its band (MHz), the share of the time
/// it may transmit (%), the most power it may (dBm), and how its frequency
/// slots are laid out (MHz between them, and at each side of one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub start: f64,
    pub end: f64,
    pub duty_cycle: f64,
    pub power_limit: u32,
    pub spacing: f64,
    pub padding: f64,
    /// The slot it uses when none is set, from 1; 0 for the one the primary
    /// channel's name picks.
    pub default_slot: u32,
}

/// As the firmware's table has them. One not known, as the unset region.
pub fn region(code: &str) -> Region {
    // Each profile's spacing and padding, in MHz.
    const STD: (f64, f64) = (0.0, 0.0);
    const LITE: (f64, f64) = (0.4, 0.0375);
    const NARROW: (f64, f64) = (0.0, 0.0104);
    const HAM_20KHZ: (f64, f64) = (0.0, 0.0022);
    const HAM_100KHZ: (f64, f64) = (0.0, 0.01875);
    let (start, end, duty_cycle, power_limit, (spacing, padding)) = match code {
        "US" => (902.0, 928.0, 100.0, 30, STD),
        "EU_433" => (433.0, 434.0, 10.0, 10, STD),
        "EU_868" => (869.4, 869.65, 10.0, 27, STD),
        "EU_866" => (865.6, 867.6, 2.5, 27, LITE),
        "EU_N_868" => (869.4, 869.65, 10.0, 27, NARROW),
        "CN" => (470.0, 510.0, 100.0, 19, STD),
        "JP" => (920.5, 923.5, 100.0, 13, STD),
        "ANZ" => (915.0, 928.0, 100.0, 30, STD),
        "ANZ_433" => (433.05, 434.79, 100.0, 14, STD),
        "RU" => (868.7, 869.2, 100.0, 20, STD),
        "KR" => (920.0, 923.0, 100.0, 23, STD),
        "TW" => (920.0, 925.0, 100.0, 27, STD),
        "IN" => (865.0, 867.0, 100.0, 30, STD),
        "NZ_865" => (864.0, 868.0, 100.0, 36, STD),
        "TH" => (920.0, 925.0, 10.0, 27, STD),
        "UA_433" => (433.0, 434.7, 10.0, 10, STD),
        "MY_433" => (433.0, 435.0, 100.0, 20, STD),
        "MY_919" => (919.0, 924.0, 100.0, 27, STD),
        "SG_923" => (917.0, 925.0, 100.0, 20, STD),
        "PH_433" => (433.0, 434.7, 100.0, 10, STD),
        "PH_868" => (868.0, 869.4, 100.0, 14, STD),
        "PH_915" => (915.0, 918.0, 100.0, 24, STD),
        "KZ_433" => (433.075, 434.775, 100.0, 10, STD),
        "KZ_863" => (863.0, 868.0, 100.0, 30, STD),
        "NP_865" => (865.0, 868.0, 100.0, 30, STD),
        "BR_902" => (902.0, 907.5, 100.0, 30, STD),
        "ITU1_2M" => (144.0, 146.0, 100.0, 30, HAM_20KHZ),
        "ITU2_2M" | "ITU3_2M" => (144.0, 148.0, 100.0, 30, HAM_20KHZ),
        "ITU2_125CM" => (220.0, 225.0, 100.0, 30, HAM_100KHZ),
        "ITU1_70CM" | "ITU3_70CM" => (430.0, 440.0, 100.0, 30, HAM_100KHZ),
        "ITU2_70CM" => (420.0, 450.0, 100.0, 30, HAM_100KHZ),
        "LORA_24" => (2400.0, 2483.5, 100.0, 10, STD),
        _ => (902.0, 928.0, 100.0, 30, STD),
    };
    let default_slot = match code {
        "EU_N_868" => 1,
        "ITU1_2M" => 26,
        "ITU2_2M" => 51,
        "ITU3_2M" => 33,
        "ITU2_125CM" | "ITU1_70CM" | "ITU3_70CM" => 37,
        "ITU2_70CM" => 137,
        _ => 0,
    };
    Region {
        start,
        end,
        duty_cycle,
        power_limit,
        spacing,
        padding,
        default_slot,
    }
}

/// A bandwidth's code as kHz.
pub fn khz(code: u32) -> f64 {
    match code {
        8 => 7.8,
        10 => 10.4,
        16 => 15.6,
        21 => 20.8,
        31 => 31.25,
        42 => 41.7,
        62 => 62.5,
        200 => 203.125,
        400 => 406.25,
        800 => 812.5,
        1600 => 1625.0,
        other => f64::from(other),
    }
}

/// How many frequency slots a region has at a bandwidth (its code), as the
/// firmware counts them; none in the unset region.
pub fn slots(code: &str, bandwidth: u32) -> u32 {
    if code == "UNSET" {
        return 0;
    }
    let region = region(code);
    let width = region.spacing + region.padding * 2.0 + khz(bandwidth) / 1000.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let slots = ((region.end - region.start + region.spacing) / width).round() as u32;
    slots
}

pub const SPREAD_FACTORS: std::ops::RangeInclusive<u32> = 5..=12;
pub const CODING_RATES: std::ops::RangeInclusive<u32> = 5..=8;

/// Every region's presets, by region, for the API.
pub fn presets_by_region(regions: &[String]) -> BTreeMap<String, Vec<String>> {
    regions
        .iter()
        .map(|region| {
            let presets = presets(region).iter().map(|&p| p.to_owned()).collect();
            (region.clone(), presets)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_takes_its_own_presets() {
        assert!(presets("US").contains(&"SHORT_TURBO"));
        assert!(!presets("EU_868").contains(&"SHORT_TURBO"));
        assert_eq!(presets("EU_N_868"), NARROW);
        assert_eq!(presets("EU_866")[0], "LITE_FAST");
    }

    #[test]
    fn a_region_has_its_slots() {
        assert_eq!(slots("US", 250), 104);
        assert_eq!(slots("EU_868", 250), 1);
        assert_eq!(slots("UNSET", 250), 0);
        assert_eq!(region("JP").power_limit, 13);
        assert!(known("EU_868") && !known("EU_917"));
    }

    #[test]
    fn a_preset_stands_for_its_parameters() {
        assert_eq!(params("LONG_FAST", false), (250, 11, 5));
        assert_eq!(params("LONG_FAST", true), (800, 11, 5));
        assert_eq!(params("NARROW_FAST", true), (62, 7, 6));
    }
}
