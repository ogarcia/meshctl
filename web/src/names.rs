// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's codes as people read them, in the interface's language, with
//! the code itself after, so that there is no doubt which it is: "Unión
//! Europea 868 MHz (EU_868)". A code not known here is shown as it is.

use crate::i18n::Locale;

/// A region's name, in Spanish and in English, by its code.
const REGIONS: &[(&str, &str, &str)] = &[
    ("UNSET", "Sin definir", "Unset"),
    ("US", "Estados Unidos", "United States"),
    ("EU_433", "Unión Europea 433 MHz", "European Union 433 MHz"),
    ("EU_868", "Unión Europea 868 MHz", "European Union 868 MHz"),
    ("CN", "China", "China"),
    ("JP", "Japón", "Japan"),
    (
        "ANZ",
        "Australia y Nueva Zelanda",
        "Australia and New Zealand",
    ),
    ("KR", "Corea", "Korea"),
    ("TW", "Taiwán", "Taiwan"),
    ("RU", "Rusia", "Russia"),
    ("IN", "India", "India"),
    ("NZ_865", "Nueva Zelanda 865 MHz", "New Zealand 865 MHz"),
    ("TH", "Tailandia", "Thailand"),
    ("LORA_24", "2,4 GHz en todo el mundo", "2.4 GHz worldwide"),
    ("UA_433", "Ucrania 433 MHz", "Ukraine 433 MHz"),
    ("UA_868", "Ucrania 868 MHz", "Ukraine 868 MHz"),
    ("MY_433", "Malasia 433 MHz", "Malaysia 433 MHz"),
    ("MY_919", "Malasia 919 MHz", "Malaysia 919 MHz"),
    ("SG_923", "Singapur 923 MHz", "Singapore 923 MHz"),
    ("PH_433", "Filipinas 433 MHz", "Philippines 433 MHz"),
    ("PH_868", "Filipinas 868 MHz", "Philippines 868 MHz"),
    ("PH_915", "Filipinas 915 MHz", "Philippines 915 MHz"),
    (
        "ANZ_433",
        "Australia y Nueva Zelanda 433 MHz",
        "Australia and New Zealand 433 MHz",
    ),
    ("KZ_433", "Kazajistán 433 MHz", "Kazakhstan 433 MHz"),
    ("KZ_863", "Kazajistán 863 MHz", "Kazakhstan 863 MHz"),
    ("NP_865", "Nepal 865 MHz", "Nepal 865 MHz"),
    ("BR_902", "Brasil 902 MHz", "Brazil 902 MHz"),
    (
        "ITU1_2M",
        "UIT Región 1, 2 m (radioaficionados)",
        "ITU Region 1, 2 m (amateur)",
    ),
    (
        "ITU2_2M",
        "UIT Región 2, 2 m (radioaficionados)",
        "ITU Region 2, 2 m (amateur)",
    ),
    (
        "ITU3_2M",
        "UIT Región 3, 2 m (radioaficionados)",
        "ITU Region 3, 2 m (amateur)",
    ),
    (
        "EU_866",
        "Unión Europea 866 MHz (Lite)",
        "European Union 866 MHz (Lite)",
    ),
    ("EU_874", "Unión Europea 874 MHz", "European Union 874 MHz"),
    ("EU_917", "Unión Europea 917 MHz", "European Union 917 MHz"),
    (
        "EU_N_868",
        "Unión Europea 868 MHz, estrecha",
        "European Union 868 MHz, narrow",
    ),
    (
        "ITU1_70CM",
        "UIT Región 1, 70 cm (radioaficionados)",
        "ITU Region 1, 70 cm (amateur)",
    ),
    (
        "ITU2_70CM",
        "UIT Región 2, 70 cm (radioaficionados)",
        "ITU Region 2, 70 cm (amateur)",
    ),
    (
        "ITU3_70CM",
        "UIT Región 3, 70 cm (radioaficionados)",
        "ITU Region 3, 70 cm (amateur)",
    ),
    (
        "ITU2_125CM",
        "UIT Región 2, 1,25 m (radioaficionados)",
        "ITU Region 2, 1.25 m (amateur)",
    ),
];

/// A role's name, in Spanish and in English, by its code.
const ROLES: &[(&str, &str, &str)] = &[
    ("CLIENT", "Cliente", "Client"),
    ("CLIENT_MUTE", "Cliente silencioso", "Client, muted"),
    ("CLIENT_HIDDEN", "Cliente oculto", "Client, hidden"),
    ("CLIENT_BASE", "Cliente base", "Client base"),
    ("ROUTER", "Router", "Router"),
    ("ROUTER_LATE", "Router tardío", "Router, late"),
    ("ROUTER_CLIENT", "Router y cliente", "Router and client"),
    ("REPEATER", "Repetidor", "Repeater"),
    ("TRACKER", "Rastreador", "Tracker"),
    ("SENSOR", "Sensor", "Sensor"),
    ("TAK", "TAK", "TAK"),
    ("TAK_TRACKER", "Rastreador TAK", "TAK tracker"),
    ("LOST_AND_FOUND", "Objetos perdidos", "Lost and found"),
];

/// A code from a table as read in a language, with the code after it.
fn named(table: &[(&str, &str, &str)], locale: Locale, code: &str) -> String {
    table
        .iter()
        .find(|(known, _, _)| *known == code)
        .map_or_else(
            || code.to_owned(),
            |(_, es, en)| {
                let name = match locale {
                    Locale::es => es,
                    Locale::en => en,
                };
                format!("{name} ({code})")
            },
        )
}

pub fn region(locale: Locale, code: &str) -> String {
    named(REGIONS, locale, code)
}

pub fn role(locale: Locale, code: &str) -> String {
    named(ROLES, locale, code)
}

/// A modem preset by the name it goes by everywhere, in no language of its
/// own, the one the firmware gives an unnamed channel: LongFast (LONG_FAST).
pub fn preset(_: Locale, code: &str) -> String {
    format!("{} ({code})", crate::chat::preset_name(code))
}

/// Codes in the order they come, the most used first, but for one apart,
/// as the unset region: first while it is the one chosen, to leave it for
/// any other the soonest; last otherwise, out of the way.
pub fn placed(mut codes: Vec<String>, apart: &str, chosen: &str) -> Vec<String> {
    if let Some(at) = codes.iter().position(|code| code == apart) {
        let code = codes.remove(at);
        if chosen == apart {
            codes.insert(0, code);
        } else {
            codes.push(code);
        }
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_reads_with_its_code() {
        assert_eq!(
            region(Locale::es, "EU_868"),
            "Unión Europea 868 MHz (EU_868)"
        );
        assert_eq!(region(Locale::en, "US"), "United States (US)");
        assert_eq!(region(Locale::es, "MARS_1"), "MARS_1");
        assert_eq!(
            role(Locale::es, "CLIENT_MUTE"),
            "Cliente silencioso (CLIENT_MUTE)"
        );
        assert_eq!(preset(Locale::es, "LONG_FAST"), "LongFast (LONG_FAST)");
        assert_eq!(
            preset(Locale::en, "LONG_MODERATE"),
            "LongMod (LONG_MODERATE)"
        );
    }

    #[test]
    fn regions_keep_their_order_unset_first_only_if_chosen() {
        let codes = ["UNSET", "US", "EU_433", "EU_868"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            placed(codes.clone(), "UNSET", "UNSET"),
            ["UNSET", "US", "EU_433", "EU_868"]
        );
        assert_eq!(
            placed(codes, "UNSET", "EU_868"),
            ["US", "EU_433", "EU_868", "UNSET"]
        );
    }
}
