// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Automatic replies: a message heard that says what one of them waits for,
//! where it waits for it, is answered on its own, with a reaction, a text,
//! or both, from what the message came with. On either mesh alike: each
//! tells what it can of the message, and what it cannot is left empty.
//!
//! Kept in check, as a mesh is everyone's: never a message of the radio's
//! own, one come through MQTT, a reaction, a room's post, or one that looks
//! like another's automatic reply (it starts with 🤖); never one heard as the
//! radio catches up on what it kept while away, nor one from long ago; the
//! same node answered by the same reply once in a while; a radio's replies
//! few a minute; and none on a channel busier than the user lets be.

use crate::radios::{Change, Radios};
use crate::store::now;
use meshctl_api::auto_replies::{MAX_TEXT_BYTES, Told, fit, hops_emoji, render};
use meshctl_api::{
    AutoReply, AutoReplyMatch, AutoReplyPlace, AutoReplySettings, BROADCAST, Message, NewMessage,
    RadioId,
};
use meshctl_core::mesh::{self, LinkState};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;

/// How long a radio is taken to be catching up, after it is ready: what it
/// kept while nobody was there comes then, and is not answered.
const CATCHING_UP: Duration = Duration::from_secs(20);

/// The oldest a message is answered: by when it was heard.
const FRESH_SECS: i64 = 120;

/// How long before the same reply answers the same node again.
const AGAIN_AFTER: Duration = Duration::from_secs(5 * 60);

/// The most replies a radio sends in a minute.
const MOST_A_MINUTE: usize = 6;

/// What starts another's automatic reply, by custom: never answered, so
/// that two never answer each other.
const BOT: &str = "🤖";

/// How often a MeshCore radio is asked how long it has been on the air.
const AIRTIME_EVERY: Duration = Duration::from_secs(60);

/// Answers what the automatic replies wait for, from what the radios hear,
/// until they stop; and follows how busy each hears its channel.
pub fn spawn(radios: Radios) {
    tokio::spawn(follow_airtime(radios.clone()));
    tokio::spawn(async move {
        let mut changes = radios.subscribe();
        let mut answering = Answering::default();
        loop {
            let change = tokio::select! {
                change = changes.recv() => change,
                () = radios.stopped() => return,
            };
            match change {
                Ok(Change::Mesh(radio, mesh::Event::Link(LinkState::Ready))) => {
                    answering.ready.insert(radio, Instant::now());
                    // A Meshtastic radio's own node, as it was just read.
                    if let Some(busy) = radios.mesh(radio).and_then(|mesh| {
                        mesh.read(|s| s.nodes.get(&s.local.as_ref()?.num)?.channel_utilization)
                    }) {
                        radios.set_channel_busy(radio, busy);
                    }
                }
                // A Meshtastic radio's own node, as it tells of itself.
                Ok(Change::Mesh(radio, mesh::Event::Node(node))) => {
                    let own = radios
                        .mesh(radio)
                        .and_then(|mesh| mesh.read(|s| s.local.as_ref().map(|l| l.num)));
                    if own == Some(node.num)
                        && let Some(busy) = node.channel_utilization
                    {
                        radios.set_channel_busy(radio, busy);
                    }
                }
                Ok(Change::Message(radio, message)) => {
                    answering.heard(&radios, radio, message).await;
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    });
}

/// How busy each MeshCore radio hears its channel: what it has spent on
/// the air, sending and receiving, over the time between two askings.
async fn follow_airtime(radios: Radios) {
    let mut every = tokio::time::interval(AIRTIME_EVERY);
    let mut last: HashMap<RadioId, (u32, Instant)> = HashMap::new();
    loop {
        tokio::select! {
            _ = every.tick() => {}
            () = radios.stopped() => return,
        }
        let ready = radios.meshcore_ready();
        last.retain(|id, _| ready.iter().any(|(r, _)| r == id));
        for (radio, mesh) in ready {
            let Ok(Some((sent, received))) = mesh.meshcore_airtime().await else {
                continue;
            };
            let (aired, at) = (sent.saturating_add(received), Instant::now());
            if let Some(percent) = last
                .insert(radio, (aired, at))
                .and_then(|before| busy_percent(before, (aired, at)))
            {
                radios.set_channel_busy(radio, percent);
            }
        }
    }
}

/// The share of the time between two figures of seconds on the air that
/// was spent on it, in percent; none if the radio started again between
/// them, its count from naught, or no time went by.
fn busy_percent((before, then): (u32, Instant), (now, at): (u32, Instant)) -> Option<f32> {
    let elapsed = at.checked_duration_since(then)?.as_secs_f32();
    let aired = now.checked_sub(before)?;
    #[allow(clippy::cast_precision_loss)]
    (elapsed >= 1.0).then(|| (aired as f32 / elapsed * 100.0).min(100.0))
}

#[derive(Default)]
struct Answering {
    /// When each radio was last ready.
    ready: HashMap<RadioId, Instant>,
    /// When each reply last answered each sender on each radio.
    answered: HashMap<(i64, RadioId, Sender), Instant>,
    /// When each radio sent its latest replies.
    sent: HashMap<RadioId, VecDeque<Instant>>,
}

/// Who sent a message: its node, or, on a MeshCore channel, where none is
/// known, the name it came under.
type Sender = (u32, Option<String>);

impl Answering {
    async fn heard(&mut self, radios: &Radios, radio: RadioId, message: Message) {
        let at = Instant::now();
        if !answerable(&message, now())
            || self
                .ready
                .get(&radio)
                .is_none_or(|ready| at.duration_since(*ready) < CATCHING_UP)
        {
            return;
        }
        let replies = match radios.auto_replies().await {
            Ok(replies) => replies,
            Err(err) => {
                tracing::warn!(error = format_args!("{err:#}"), "automatic replies");
                return;
            }
        };
        let Some(reply) = replies
            .into_iter()
            .find(|reply| answers(reply, radio, &message))
        else {
            return;
        };
        // Not on a channel busier than it is let be.
        let limits = radios.auto_reply_limits().await.unwrap_or_default();
        if let (Some(most), Some(busy)) = (limits.busy_channel, radios.channel_busy(radio))
            && busy > f32::from(most)
        {
            tracing::info!(
                radio,
                reply = reply.id,
                busy,
                most,
                "automatic reply: channel busy"
            );
            return;
        }
        let sender = (message.from, message.sender_name.clone());
        let key = (reply.id, radio, sender);
        if self
            .answered
            .get(&key)
            .is_some_and(|last| at.duration_since(*last) < AGAIN_AFTER)
        {
            tracing::info!(
                radio,
                reply = reply.id,
                "automatic reply: answered not long ago"
            );
            return;
        }
        let sent = self.sent.entry(radio).or_default();
        while sent
            .front()
            .is_some_and(|first| at.duration_since(*first) >= Duration::from_secs(60))
        {
            sent.pop_front();
        }
        if sent.len() >= MOST_A_MINUTE {
            tracing::warn!(
                radio,
                reply = reply.id,
                "automatic reply: too many this minute"
            );
            return;
        }
        sent.push_back(at);
        self.answered
            .retain(|_, last| at.duration_since(*last) < AGAIN_AFTER);
        self.answered.insert(key, at);
        answer(radios, radio, &reply, &message).await;
    }
}

/// An automatic reply as it is kept: each trigger trimmed, and once, the
/// first of those a message could not tell apart; none that says nothing.
pub fn tidy(mut settings: AutoReplySettings) -> AutoReplySettings {
    let mut seen = Vec::new();
    settings.triggers = settings
        .triggers
        .into_iter()
        .map(|t| t.trim().to_owned())
        .filter(|t| {
            let folded = fold(t);
            let new = !folded.is_empty() && !seen.contains(&folded);
            seen.push(folded);
            new
        })
        .collect();
    settings
}

/// Whether an automatic reply is one: named, waiting for something, and
/// answering with something, its texts no longer than a message takes.
pub fn valid(settings: &AutoReplySettings) -> bool {
    let text = |text: &Option<String>| {
        text.as_ref()
            .is_none_or(|t| !t.trim().is_empty() && t.len() <= MAX_TEXT_BYTES)
    };
    !settings.name.trim().is_empty()
        && settings.triggers.iter().any(|t| !fold(t).is_empty())
        && (settings.react || settings.reply.is_some())
        && text(&settings.reply)
        && text(&settings.direct_reply)
}

/// Whether a message is one an automatic reply may answer at `now`.
fn answerable(message: &Message, now: i64) -> bool {
    !message.outgoing
        && !message.emoji
        && !message.via_mqtt
        && message.packet_id != 0
        && message.author.is_none()
        && now - message.at <= FRESH_SECS
        && !message.text.trim_start().starts_with(BOT)
}

/// Whether a reply answers a message heard by a radio: on, where the
/// message came, and the message says what it waits for.
fn answers(reply: &AutoReply, radio: RadioId, message: &Message) -> bool {
    let settings = &reply.settings;
    let channel = message.to == BROADCAST;
    settings.enabled
        && settings.places.iter().any(|place| match place {
            AutoReplyPlace::Directs { radio: r } => *r == radio && !channel,
            AutoReplyPlace::Node { radio: r, node } => {
                *r == radio && !channel && *node == message.from
            }
            AutoReplyPlace::Channel {
                radio: r,
                channel: slot,
                ..
            } => *r == radio && channel && *slot == Some(message.channel),
        })
        && triggered(&settings.triggers, settings.matching, &message.text)
}

/// Text to match: in lower case, without accents, and only its letters and
/// numbers, in words one space apart.
fn fold(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        folded.push(c);
    }
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a text says one of the triggers, as matched.
fn triggered(triggers: &[String], matching: AutoReplyMatch, text: &str) -> bool {
    let text = fold(text);
    let words = format!(" {text} ");
    triggers
        .iter()
        .map(|t| fold(t))
        .filter(|t| !t.is_empty())
        .any(|trigger| match matching {
            AutoReplyMatch::Whole => text == trigger,
            AutoReplyMatch::Word => words.contains(&format!(" {trigger} ")),
        })
}

/// What a message heard by a radio tells, with what the radio knows of its
/// sender and its channel.
fn told(radios: &Radios, radio: RadioId, message: &Message) -> Told {
    let local = jiff::Zoned::now();
    let mut told = Told {
        hops: message.hops_away,
        snr: message.snr,
        rssi: message.rssi,
        date: local.strftime("%Y-%m-%d").to_string(),
        time: local.strftime("%H:%M").to_string(),
        long_name: message.sender_name.clone(),
        ..Told::default()
    };
    let Some(mesh) = radios.mesh(radio) else {
        return told;
    };
    mesh.read(|state| {
        let meshcore = state.meshcore.is_some();
        if let Some(user) = state.nodes.get(&message.from).and_then(|n| n.user.as_ref()) {
            told.long_name = Some(user.long_name.clone()).filter(|n| !n.is_empty());
            told.short_name = Some(user.short_name.clone()).filter(|n| !n.is_empty());
            // MeshCore's nodes go by the start of their key.
            if meshcore && user.public_key.len() >= 4 {
                told.node_id = Some(
                    user.public_key[..4]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                );
            }
        }
        if !meshcore && message.from != 0 {
            told.node_id = Some(format!("!{:08x}", message.from));
        }
        if message.to == BROADCAST {
            told.channel = crate::channels::slots_of(state)
                .into_iter()
                .find(|slot| slot.index == message.channel)
                .map(|slot| slot.name);
        }
    });
    told
}

/// Answers a message as a reply says: its reaction first, then its text.
async fn answer(radios: &Radios, radio: RadioId, reply: &AutoReply, message: &Message) {
    let settings = &reply.settings;
    let (channel, node) = if message.to == BROADCAST {
        (Some(message.channel), None)
    } else {
        (None, Some(message.from))
    };
    let out = |text: String, emoji: bool| NewMessage {
        text,
        channel,
        node,
        reply_id: Some(message.packet_id),
        emoji,
        scope: None,
    };
    if settings.react
        && let Some(hops) = message.hops_away
        && let Err(err) = radios
            .send(radio, out(hops_emoji(hops).to_owned(), true))
            .await
    {
        tracing::warn!(
            radio,
            reply = reply.id,
            ?err,
            "automatic reply: cannot react"
        );
    }
    let direct = message.hops_away == Some(0);
    let text = match (&settings.direct_reply, &settings.reply) {
        (Some(direct_reply), _) if direct => Some(direct_reply),
        (_, reply) => reply.as_ref(),
    };
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return;
    };
    let text = fit(render(text, &told(radios, radio, message)));
    if text.is_empty() {
        return;
    }
    match radios.send(radio, out(text, false)).await {
        Ok(_) => tracing::info!(radio, reply = reply.id, "automatic reply sent"),
        Err(err) => tracing::warn!(
            radio,
            reply = reply.id,
            ?err,
            "automatic reply: cannot send"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshctl_api::MessageStatus;

    fn heard(text: &str) -> Message {
        Message {
            id: 1,
            packet_id: 7,
            from: 0x1234_5678,
            sender_name: None,
            to: BROADCAST,
            channel: 2,
            text: text.to_owned(),
            at: 1_000,
            outgoing: false,
            status: MessageStatus::Received,
            error: None,
            reply_id: None,
            emoji: false,
            snr: Some(7.5),
            rssi: Some(-95),
            hops_away: Some(3),
            via_mqtt: false,
            scope: None,
            heard: 1,
            sent_at: None,
            repeaters: 0,
            author: None,
        }
    }

    fn reply(places: Vec<AutoReplyPlace>) -> AutoReply {
        AutoReply {
            id: 1,
            settings: AutoReplySettings {
                name: "Ping".into(),
                enabled: true,
                triggers: vec!["ping".into(), "test".into()],
                matching: AutoReplyMatch::Whole,
                react: true,
                reply: Some("🤖 Pong".into()),
                direct_reply: None,
                places,
            },
        }
    }

    #[test]
    fn busy_is_the_share_of_the_time_on_the_air() {
        let then = Instant::now();
        let later = then + Duration::from_secs(60);
        assert_eq!(busy_percent((100, then), (115, later)), Some(25.0));
        assert_eq!(busy_percent((100, then), (100, later)), Some(0.0));
        // Started again, or no time between them: nothing to tell.
        assert_eq!(busy_percent((100, then), (5, later)), None);
        assert_eq!(busy_percent((100, then), (115, then)), None);
        assert_eq!(busy_percent((0, then), (600, later)), Some(100.0));
    }

    #[test]
    fn triggers_are_kept_once() {
        let mut settings = reply(Vec::new()).settings;
        settings.triggers = [
            "ping", " test", "Ping!", "", "!", "test", "prueba", "PRÚEBA",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(tidy(settings).triggers, ["ping", "test", "prueba"]);
    }

    #[test]
    fn triggers_match_however_written() {
        let both = ["ping".to_owned(), "Prueba".to_owned()];
        for yes in ["ping", "Ping!", " PING ", "ping 🙂", "¡Prueba!", "prúeba"] {
            assert!(triggered(&both, AutoReplyMatch::Whole, yes), "{yes}");
        }
        for no in ["ping me", "pinging", "", "🤖"] {
            assert!(!triggered(&both, AutoReplyMatch::Whole, no), "{no}");
        }
        assert!(triggered(&both, AutoReplyMatch::Word, "a ping, please"));
        assert!(triggered(&both, AutoReplyMatch::Word, "Ping."));
        assert!(!triggered(&both, AutoReplyMatch::Word, "pinging"));
        // An empty trigger matches nothing.
        assert!(!triggered(&[" ! ".to_owned()], AutoReplyMatch::Whole, "?"));
    }

    #[test]
    fn only_some_messages_are_answered() {
        assert!(answerable(&heard("ping"), 1_060));
        assert!(!answerable(&heard("ping"), 1_000 + FRESH_SECS + 1));
        assert!(!answerable(&heard("🤖 Pong"), 1_000));
        let changed = |change: fn(&mut Message)| {
            let mut message = heard("ping");
            change(&mut message);
            answerable(&message, 1_000)
        };
        assert!(!changed(|m| m.outgoing = true));
        assert!(!changed(|m| m.emoji = true));
        assert!(!changed(|m| m.via_mqtt = true));
        assert!(!changed(|m| m.author = Some(9)));
        assert!(!changed(|m| m.packet_id = 0));
    }

    #[test]
    fn a_reply_answers_where_it_is_placed() {
        let on_channel = reply(vec![AutoReplyPlace::Channel {
            radio: 1,
            channel: Some(2),
            name: None,
        }]);
        assert!(answers(&on_channel, 1, &heard("ping")));
        assert!(!answers(&on_channel, 2, &heard("ping")));
        assert!(!answers(&on_channel, 1, &heard("hola")));
        let mut elsewhere = heard("ping");
        elsewhere.channel = 3;
        assert!(!answers(&on_channel, 1, &elsewhere));
        let mut direct = heard("ping");
        direct.to = 0x9999;
        assert!(!answers(&on_channel, 1, &direct));

        let directs = reply(vec![AutoReplyPlace::Directs { radio: 1 }]);
        assert!(answers(&directs, 1, &direct));
        assert!(!answers(&directs, 1, &heard("ping")));
        let one = reply(vec![AutoReplyPlace::Node {
            radio: 1,
            node: 0x1234_5678,
        }]);
        assert!(answers(&one, 1, &direct));
        let mut other = direct.clone();
        other.from = 0x42;
        assert!(!answers(&one, 1, &other));

        let mut off = directs;
        off.settings.enabled = false;
        assert!(!answers(&off, 1, &direct));
    }
}
