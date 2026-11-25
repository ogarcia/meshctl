// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What an automatic reply says, as the daemon sends it and a client shows
//! it before: its text with its variables told, and the emoji that tells
//! the hops a message took.

/// The longest text a message takes, in bytes.
pub const MAX_TEXT_BYTES: usize = 200;

/// The variables a reply's text takes, as written in it.
pub const VARIABLES: [&str; 10] = [
    "LONG_NAME",
    "SHORT_NAME",
    "NODE_ID",
    "HOPS",
    "KANGAROO_HOPS",
    "SNR",
    "RSSI",
    "CHANNEL",
    "DATE",
    "TIME",
];

/// What a message answered tells the variables; what it does not, none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Told {
    pub long_name: Option<String>,
    pub short_name: Option<String>,
    pub node_id: Option<String>,
    pub hops: Option<u32>,
    pub snr: Option<f32>,
    pub rssi: Option<i32>,
    pub channel: Option<String>,
    pub date: String,
    pub time: String,
}

/// The reaction that tells the hops a message took: 🎯 for none, a number
/// up to ten, and 🦘 for more.
pub fn hops_emoji(hops: u32) -> &'static str {
    const KEYCAPS: [&str; 10] = ["🎯", "1️⃣", "2️⃣", "3️⃣", "4️⃣", "5️⃣", "6️⃣", "7️⃣", "8️⃣", "9️⃣"];
    match hops {
        0..=9 => KEYCAPS[hops as usize],
        10 => "🔟",
        _ => "🦘",
    }
}

/// A reply's text with its variables told: one not told left empty, and
/// what is no variable as written. Not cut to what a message takes.
pub fn render(text: &str, told: &Told) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let value = match &after[..close] {
                "LONG_NAME" => told.long_name.clone().unwrap_or_default(),
                "SHORT_NAME" => told.short_name.clone().unwrap_or_default(),
                "NODE_ID" => told.node_id.clone().unwrap_or_default(),
                // MeshMonitor's name for it too.
                "HOPS" | "NUMBER_HOPS" => told.hops.map(|h| h.to_string()).unwrap_or_default(),
                "KANGAROO_HOPS" => told
                    .hops
                    .map(|h| "🦘".repeat(h as usize))
                    .unwrap_or_default(),
                "SNR" => told.snr.map(|s| format!("{s:.1}")).unwrap_or_default(),
                "RSSI" => told.rssi.map(|r| r.to_string()).unwrap_or_default(),
                "CHANNEL" => told.channel.clone().unwrap_or_default(),
                "DATE" => told.date.clone(),
                "TIME" => told.time.clone(),
                _ => return None,
            };
            Some((value, close))
        });
        match value {
            Some((value, close)) => {
                out.push_str(&value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.trim_end().to_owned()
}

/// What an automatic reply signs its texts with, and what tells another's.
pub const SIGNATURE: &str = "🤖";

/// A reply's text, signed if it is to be, unless it starts signed already.
pub fn signed(text: String, sign: bool) -> String {
    if sign && !text.trim_start().starts_with(SIGNATURE) && !text.is_empty() {
        format!("{SIGNATURE} {text}")
    } else {
        text
    }
}

/// A text cut to what a message takes, on a character.
pub fn fit(mut text: String) -> String {
    let mut end = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_are_told() {
        let told = Told {
            long_name: Some("Pastoriza".into()),
            short_name: Some("PSTZ".into()),
            node_id: Some("!12345678".into()),
            hops: Some(3),
            snr: Some(7.5),
            rssi: Some(-95),
            channel: Some("Galicia".into()),
            date: "2026-11-25".into(),
            time: "09:30".into(),
        };
        assert_eq!(
            render(
                "🤖 Pong, {HOPS} 🦘 {KANGAROO_HOPS}, {LONG_NAME} ({SHORT_NAME}, {NODE_ID})",
                &told
            ),
            "🤖 Pong, 3 🦘 🦘🦘🦘, Pastoriza (PSTZ, !12345678)"
        );
        assert_eq!(
            render(
                "SNR: {SNR}dB RSSI: {RSSI}dBm #{CHANNEL} {DATE} {TIME}",
                &told
            ),
            "SNR: 7.5dB RSSI: -95dBm #Galicia 2026-11-25 09:30"
        );
        assert_eq!(render("{NUMBER_HOPS}", &told), "3");
        // Unknown or unclosed: as written; one not told: empty.
        assert_eq!(render("{NOPE} {HOPS", &told), "{NOPE} {HOPS");
        assert_eq!(render("hops: {HOPS}", &Told::default()), "hops:");
        // Cut to what a message takes, on a character.
        let long = fit("ñ".repeat(150));
        assert!(long.len() <= MAX_TEXT_BYTES && long.chars().all(|c| c == 'ñ'));
    }

    #[test]
    fn texts_are_signed_once() {
        assert_eq!(signed("Pong".into(), true), "🤖 Pong");
        assert_eq!(signed("🤖 Pong".into(), true), "🤖 Pong");
        assert_eq!(signed(" 🤖Pong".into(), true), " 🤖Pong");
        assert_eq!(signed("Hola".into(), false), "Hola");
        assert_eq!(signed(String::new(), true), "");
    }

    #[test]
    fn hops_are_told_as_an_emoji() {
        assert_eq!(hops_emoji(0), "🎯");
        assert_eq!(hops_emoji(3), "3️⃣");
        assert_eq!(hops_emoji(10), "🔟");
        assert_eq!(hops_emoji(12), "🦘");
    }
}
