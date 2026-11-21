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
    fn a_preset_stands_for_its_parameters() {
        assert_eq!(params("LONG_FAST", false), (250, 11, 5));
        assert_eq!(params("LONG_FAST", true), (800, 11, 5));
        assert_eq!(params("NARROW_FAST", true), (62, 7, 6));
    }
}
