// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! MeshCore reactions, as its apps send them: MeshCore has no reaction
//! packet, and they send a short text that names the message reacted to by
//! a hash of it. Read in both forms in use, MeshCore One's and
//! meshcore-open's; sent in MeshCore One's, which both read.
//!
//! - MeshCore One: on a channel `@[Sender]emoji\nhash` (or, older,
//!   `emoji@[Sender]\nhash`), in a direct chat `emoji\nhash`. The hash is the
//!   first 40 bits of the SHA-256 of the message's text (on a channel, after
//!   its `Sender: `) and its sender's clock as a little-endian u32, in
//!   lowercase Crockford base32.
//! - meshcore-open: `r:hhhh:ii`, the low 16 bits of a string hash of the
//!   sender's clock in decimal, the sender's name on a channel, and the
//!   first five UTF-16 units of the text; and the emoji by its place in its
//!   table, below.

use sha2::{Digest, Sha256};
use unicode_segmentation::UnicodeSegmentation;

/// A reaction as sent: its emoji, and how it names the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    /// MeshCore One's, on a channel: to a message by whom, and its hash.
    Channel {
        emoji: String,
        sender: String,
        hash: String,
    },
    /// MeshCore One's, in a direct chat.
    Direct { emoji: String, hash: String },
    /// meshcore-open's.
    Open { emoji: String, hash: u16 },
}

impl Reaction {
    pub fn emoji(&self) -> &str {
        match self {
            Self::Channel { emoji, .. } | Self::Direct { emoji, .. } | Self::Open { emoji, .. } => {
                emoji
            }
        }
    }

    /// Whether it names a message: the message's text (on a channel, after
    /// its sender's name), its sender's name, as the radio knows it, and its
    /// sender's clock.
    pub fn names(&self, text: &str, sender: Option<&str>, sent_at: u32) -> bool {
        match self {
            Self::Channel {
                sender: wanted,
                hash,
                ..
            } => sender == Some(wanted.as_str()) && *hash == mc1_hash(&body(text), sent_at),
            Self::Direct { hash, .. } => *hash == mc1_hash(&body(text), sent_at),
            Self::Open { hash, .. } => *hash == open_hash(sent_at, sender, text),
        }
    }
}

/// Crockford's base32, lowercase.
const BASE32: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// MeshCore One's hash of a message: its text and its sender's clock.
pub fn mc1_hash(text: &str, sent_at: u32) -> String {
    let digest = Sha256::new()
        .chain_update(text.as_bytes())
        .chain_update(sent_at.to_le_bytes())
        .finalize();
    let bits = digest[..5]
        .iter()
        .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte));
    (0..8)
        .rev()
        .map(|at| BASE32[usize::try_from((bits >> (at * 5)) & 0x1f).unwrap_or(0)] as char)
        .collect()
}

/// A message's text as MeshCore One hashes it: spaces and tabs off both
/// ends.
fn body(text: &str) -> String {
    text.trim_matches([' ', '\t']).to_owned()
}

/// A hash as read: eight of Crockford's, either case, its look-alikes taken
/// for what they stand for; none if it is not one.
fn read_hash(text: &str) -> Option<String> {
    if text.chars().count() != 8 {
        return None;
    }
    text.chars()
        .map(|c| match c.to_ascii_lowercase() {
            'o' => Some('0'),
            'i' | 'l' => Some('1'),
            'u' => None,
            c if c.is_ascii_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// The Dart VM's string hash, as meshcore-open uses it: one at a time over
/// UTF-16 units, kept to 30 bits, never 0.
fn dart_hash(input: &str) -> u32 {
    let mut hash: u32 = 0;
    for unit in input.encode_utf16() {
        hash = hash.wrapping_add(u32::from(unit));
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash = hash.wrapping_add(hash << 15);
    hash &= 0x3fff_ffff;
    hash.max(1)
}

/// meshcore-open's hash of a message: its sender's clock in decimal, its
/// sender's name on a channel, the first five UTF-16 units of its text.
pub fn open_hash(sent_at: u32, sender: Option<&str>, text: &str) -> u16 {
    let first: Vec<u16> = text.encode_utf16().take(5).collect();
    let input = format!(
        "{sent_at}{}{}",
        sender.unwrap_or_default(),
        String::from_utf16_lossy(&first)
    );
    u16::try_from(dart_hash(&input) & 0xffff).unwrap_or(0)
}

/// Whether a text is one emoji: one grapheme, an emoji by its first scalar,
/// or a keycap or symbol made one by what follows it. Not a digit, a hash
/// sign or a letter alone.
pub fn is_emoji(text: &str) -> bool {
    if text.graphemes(true).count() != 1 {
        return false;
    }
    let mut scalars = text.chars();
    let Some(first) = scalars.next() else {
        return false;
    };
    let more = scalars.next().is_some();
    let code = u32::from(first);
    if code > 0x238c {
        return !first.is_alphanumeric();
    }
    more && (first.is_ascii_digit()
        || matches!(first, '#' | '*' | '©' | '®' | '‼' | '⁉' | '™' | 'ℹ')
        || (0x2194..=0x21aa).contains(&code)
        || (0x231a..=0x231b).contains(&code)
        || code == 0x2328)
}

/// Reads a reaction from a message's text, on a channel or in a direct
/// chat; none if it is not one.
pub fn parse(text: &str, channel: bool) -> Option<Reaction> {
    if let Some(open) = parse_open(text) {
        return Some(open);
    }
    // On a channel its new line may come back a space, as our own echoes
    // have been heard to.
    let (head, tail) = match text.rsplit_once('\n') {
        Some(split) => split,
        None if channel && text.contains("@[") => text.rsplit_once(' ')?,
        None => return None,
    };
    let hash = read_hash(tail)?;
    if !channel {
        if head.contains("@[") || !is_emoji(head) {
            return None;
        }
        return Some(Reaction::Direct {
            emoji: head.to_owned(),
            hash,
        });
    }
    let at = head.find("@[")?;
    let (sender, emoji) = if at == 0 {
        let close = head.find(']')?;
        (&head[2..close], &head[close + 1..])
    } else {
        // The older form: the emoji first.
        let sender = head[at + 2..].strip_suffix(']')?;
        (sender, &head[..at])
    };
    (!sender.is_empty() && is_emoji(emoji)).then(|| Reaction::Channel {
        emoji: emoji.to_owned(),
        sender: sender.to_owned(),
        hash,
    })
}

/// meshcore-open's form: `r:hhhh:ii`.
fn parse_open(text: &str) -> Option<Reaction> {
    let rest = text.strip_prefix("r:")?;
    let (hash, index) = rest.split_once(':')?;
    let lower_hex = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if hash.len() != 4 || index.len() != 2 || !lower_hex(hash) || !lower_hex(index) {
        return None;
    }
    let emoji = OPEN_EMOJIS.get(usize::from_str_radix(index, 16).ok()?)?;
    Some(Reaction::Open {
        emoji: (*emoji).to_owned(),
        hash: u16::from_str_radix(hash, 16).ok()?,
    })
}

/// A reaction to send, in MeshCore One's form: on a channel to a message by
/// whom; its text and its sender's clock name it. None if it cannot be
/// sent so: not one emoji, or a name that would not read back.
pub fn build(
    emoji: &str,
    channel_sender: Option<&str>,
    text: &str,
    sent_at: u32,
) -> Option<String> {
    if !is_emoji(emoji) {
        return None;
    }
    let hash = mc1_hash(&body(text), sent_at);
    match channel_sender {
        Some(sender) if sender.is_empty() || sender.contains(']') || sender.contains('\n') => None,
        Some(sender) => Some(format!("@[{sender}]{emoji}\n{hash}")),
        None => Some(format!("{emoji}\n{hash}")),
    }
}

/// meshcore-open's reaction emojis, by the place its reactions name them by:
/// the lists of its emoji picker in order (quick, smileys, gestures, hearts,
/// objects). From zjs81/meshcore-open's `lib/widgets/emoji_picker.dart`,
/// MIT, Copyright (c) 2025 zjs81.
pub const OPEN_EMOJIS: [&str; 184] = [
    "👍",
    "❤️",
    "😂",
    "🎉",
    "👏",
    "🔥",
    "😀",
    "😃",
    "😄",
    "😁",
    "😅",
    "😂",
    "🤣",
    "😊",
    "😇",
    "🙂",
    "🙃",
    "😉",
    "😌",
    "😍",
    "🥰",
    "😘",
    "😗",
    "😙",
    "😚",
    "😋",
    "😛",
    "😝",
    "😜",
    "🤪",
    "🤨",
    "🧐",
    "🤓",
    "😎",
    "🥸",
    "🤩",
    "🥳",
    "😏",
    "😒",
    "😞",
    "😔",
    "😟",
    "😕",
    "🙁",
    "😣",
    "😖",
    "😫",
    "😩",
    "🥺",
    "😢",
    "😭",
    "😤",
    "😠",
    "😡",
    "🤬",
    "🤯",
    "😳",
    "🥵",
    "🥶",
    "😱",
    "😨",
    "😰",
    "😥",
    "😓",
    "🤗",
    "🤔",
    "🤭",
    "🤫",
    "🤥",
    "😶",
    "👍",
    "👎",
    "👊",
    "✊",
    "🤛",
    "🤜",
    "🤞",
    "✌️",
    "🤟",
    "🤘",
    "👌",
    "🤌",
    "🤏",
    "👈",
    "👉",
    "👆",
    "👇",
    "☝️",
    "👋",
    "🤚",
    "🖐️",
    "✋",
    "🖖",
    "👏",
    "🙌",
    "👐",
    "🤲",
    "🤝",
    "🙏",
    "✍️",
    "💅",
    "🤳",
    "💪",
    "❤️",
    "🧡",
    "💛",
    "💚",
    "💙",
    "💜",
    "🖤",
    "🤍",
    "🤎",
    "💔",
    "❤️‍🔥",
    "❤️‍🩹",
    "💕",
    "💞",
    "💓",
    "💗",
    "💖",
    "💘",
    "💝",
    "💟",
    "💌",
    "💢",
    "💥",
    "💫",
    "💦",
    "💨",
    "🕳️",
    "💬",
    "👁️‍🗨️",
    "🗨️",
    "🗯️",
    "💭",
    "🎉",
    "🎊",
    "🎈",
    "🎁",
    "🎀",
    "🪅",
    "🪆",
    "🏆",
    "🥇",
    "🥈",
    "🥉",
    "⚽",
    "⚾",
    "🥎",
    "🏀",
    "🏐",
    "🏈",
    "🏉",
    "🎾",
    "🥏",
    "🎳",
    "🏏",
    "🏑",
    "🏒",
    "🥍",
    "🏓",
    "🏸",
    "🥊",
    "🥋",
    "🥅",
    "⛳",
    "🔥",
    "⭐",
    "🌟",
    "✨",
    "⚡",
    "💡",
    "🔦",
    "🏮",
    "🪔",
    "📱",
    "💻",
    "⌚",
    "📷",
    "📺",
    "📻",
    "🎵",
    "🎶",
    "🚀",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mc1_hashes_are_meshcore_ones() {
        // The vectors MeshCore One's format is published with.
        assert_eq!(mc1_hash("Hello", 1_704_067_200), "b45pc4ek");
        assert_eq!(mc1_hash("Hello world", 1_704_067_200), "7a6r3xy1");
        assert_eq!(mc1_hash("Hello", 1_704_067_201), "3wh8x8z5");
        assert_eq!(mc1_hash("World", 1_704_067_200), "vp1vxnf4");
        assert_eq!(mc1_hash("Message 0", 0), "7mt5pe1m");
        assert_eq!(mc1_hash("café 👍 ok", 1_760_000_000), "8b5qzwf6");
        assert_eq!(mc1_hash("", u32::MAX), "npah66y0");
    }

    #[test]
    fn reactions_read_in_every_form() {
        assert_eq!(
            parse("@[Alice]👍\nb45pc4ek", true),
            Some(Reaction::Channel {
                emoji: "👍".into(),
                sender: "Alice".into(),
                hash: "b45pc4ek".into()
            })
        );
        // The older form, a hash in capitals with look-alikes.
        assert_eq!(
            parse("👍@[Alice]\nB45PC4EK", true),
            parse("@[Alice]👍\nb45pc4ek", true)
        );
        assert_eq!(
            parse("❤️\nb45pc4ek", false),
            Some(Reaction::Direct {
                emoji: "❤️".into(),
                hash: "b45pc4ek".into()
            })
        );
        assert_eq!(
            parse("r:1a2b:00", true),
            Some(Reaction::Open {
                emoji: "👍".into(),
                hash: 0x1a2b
            })
        );
        // Not reactions: words, a digit, a channel form in a direct chat,
        // a hash a letter short, an emoji past the table.
        assert_eq!(parse("ok\nb45pc4ek", false), None);
        assert_eq!(parse("1\nb45pc4ek", false), None);
        assert_eq!(parse("@[Alice]👍\nb45pc4ek", false), None);
        assert_eq!(parse("@[Alice]👍\nb45pc4e", true), None);
        assert_eq!(parse("r:1a2b:ff", true), None);
        assert_eq!(parse("hola, ¿qué tal?", true), None);
        // A new line come back a space, on a channel only; a reply is not one.
        assert_eq!(
            parse("@[Alice]👍 b45pc4ek", true),
            parse("@[Alice]👍\nb45pc4ek", true)
        );
        assert_eq!(parse("👍 b45pc4ek", false), None);
        assert_eq!(parse("@[Alice]: hola b45pc4ek", true), None);
    }

    #[test]
    fn a_reaction_names_its_message() {
        let to = |text: &str| parse(text, true).unwrap();
        let reaction = to(&build("👍", Some("Alice"), "Hello", 1_704_067_200).unwrap());
        assert_eq!(reaction.emoji(), "👍");
        assert!(reaction.names(" Hello ", Some("Alice"), 1_704_067_200));
        assert!(!reaction.names("Hello", Some("Bob"), 1_704_067_200));
        assert!(!reaction.names("Hello", Some("Alice"), 1_704_067_201));
        let direct = parse(&build("👍", None, "Hello", 1_704_067_200).unwrap(), false).unwrap();
        assert!(direct.names("Hello", None, 1_704_067_200));
        // meshcore-open's, by its own hash.
        let hash = open_hash(1_704_067_200, Some("Alice"), "Hello world");
        let open = parse(&format!("r:{hash:04x}:05"), true).unwrap();
        assert_eq!(open.emoji(), "🔥");
        assert!(open.names("Hello there", Some("Alice"), 1_704_067_200));
        assert!(!open.names("Hello there", Some("Bob"), 1_704_067_200));
        // Not sent: words for an emoji, a name with a bracket.
        assert_eq!(build("ok", None, "Hello", 1), None);
        assert_eq!(build("👍", Some("A]b"), "Hello", 1), None);
    }

    #[test]
    fn emojis_are_told_from_text() {
        // A watch is an emoji with its variation selector, not without.
        for emoji in ["👍", "❤️", "👍🏽", "👨‍👩‍👧", "🇪🇸", "1️⃣", "©️", "⌚\u{fe0f}"]
        {
            assert!(is_emoji(emoji), "{emoji}");
        }
        for text in ["a", "1", "#", "©", "⌚", "日", "ok", "👍👍", ""] {
            assert!(!is_emoji(text), "{text}");
        }
        assert_eq!(OPEN_EMOJIS.len(), 184);
    }
}
