// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The chat: the selected radio's conversations, the open one, and writing.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{Locale, t, t_string, use_i18n};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use meshctl_api::{
    BROADCAST, ErrorCode, LinkState, Message, MessageId, MessageStatus, NewMessage, Peer, RadioKind,
};
use std::time::Duration;
use wasm_bindgen::JsCast;

/// What sends a message, as an icon.
const SEND: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/send.svg");
const SMILE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/mood-smile.svg");
const REPLY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/arrow-back-up.svg");
const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const TRASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/trash.svg");
const RESEND: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/repeat.svg");
const DOWN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
const INFO: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/info-circle.svg");
const COPY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/copy.svg");
/// How far a message went.
const CLOCK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/clock.svg");
const CHECK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/check.svg");
const CHECKS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/checks.svg");
const ALERT: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/alert-triangle.svg");
const UNHEARD: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/antenna-off.svg");

/// How long, in seconds, ours on a MeshCore channel may go with no repeater
/// heard passing it on before it is taken as gone nowhere.
const UNHEARD_AFTER: u32 = 30;

/// The longest text the daemon takes, in bytes.
const MAX_TEXT_BYTES: usize = 200;

/// How many emojis a reaction offers at once, before the rest.
const REACTIONS_OFFERED: usize = 7;

/// How many reactions a message shows, the most given, before the rest.
const REACTIONS_SHOWN: usize = 4;

/// How many of the emojis most used the chat offers first: four rows.
const FREQUENT_SHOWN: usize = 36;

/// From how many bytes a message is near what it takes, and its field says
/// so.
const NEAR_TEXT_BYTES: usize = MAX_TEXT_BYTES - 20;

/// A text split where it nears what a message takes and where it goes
/// past, in bytes, each at a character: what is well within, what is near
/// the end, and what does not fit.
fn past_limit(text: &str) -> (String, String, String) {
    let at = |limit: usize| {
        text.char_indices()
            .find(|(at, c)| at + c.len_utf8() > limit)
            .map_or(text.len(), |(at, _)| at)
    };
    let (near, over) = (at(NEAR_TEXT_BYTES), at(MAX_TEXT_BYTES));
    (
        text[..near].to_owned(),
        text[near..over].to_owned(),
        text[over..].to_owned(),
    )
}

/// Who a message was exchanged with.
pub fn peer_of(message: &Message) -> Peer {
    if message.to == BROADCAST {
        Peer::Channel {
            channel: message.channel,
        }
    } else if message.outgoing {
        Peer::Node { node: message.to }
    } else {
        Peer::Node { node: message.from }
    }
}

/// A modem preset's name as the firmware gives it to an unnamed channel, in
/// any slot: `LONG_FAST` is LongFast.
pub fn preset_name(preset: &str) -> String {
    match preset {
        "LONG_MODERATE" => return "LongMod".to_owned(),
        // One the firmware has no name for.
        "VERY_LONG_SLOW" => return "Invalid".to_owned(),
        _ => {}
    }
    preset
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect()
}

/// What a conversation is called.
pub fn title(live: Live, peer: Peer) -> String {
    match peer {
        Peer::Channel { channel } => {
            let named = live.channels.with(|channels| {
                channels
                    .iter()
                    .find(|c| c.index == channel)
                    .map(|c| c.name.clone())
                    .filter(|name| !name.is_empty())
            });
            // An unnamed one, in any slot, goes by its modem preset, as the
            // radio names it, or by Custom with none; by its slot while
            // neither is known.
            let name = named
                .or_else(|| {
                    if live.selected_kind() != Some(RadioKind::Meshtastic) {
                        return None;
                    }
                    live.status.with(|s| {
                        let local = s.as_ref()?.local.as_ref()?;
                        Some(
                            local
                                .modem_preset
                                .as_deref()
                                .map_or_else(|| "Custom".to_owned(), preset_name),
                        )
                    })
                })
                .unwrap_or_else(|| {
                    let i18n = use_i18n();
                    format!("{} {channel}", t_string!(i18n, channel_numbered))
                });
            format!("# {name}")
        }
        Peer::Node { node } => node_name(live, node),
    }
}

pub fn node_name(live: Live, num: u32) -> String {
    live.nodes.with(|nodes| {
        nodes
            .get(&num)
            .and_then(|n| n.long_name.clone())
            .unwrap_or_else(|| format!("!{num:08x}"))
    })
}

/// A node's short name: its own, or, with none (as MeshCore nodes have
/// none), its name's first letters or emoji, as the list of nodes draws it;
/// with no name known either, the end of its number.
pub fn short_name(live: Live, num: u32) -> String {
    live.nodes.with(|nodes| {
        let node = nodes.get(&num);
        node.and_then(|n| n.short_name.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                node.and_then(|n| n.long_name.as_deref())
                    .map(crate::badge::initials)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| format!("{:04x}", num & 0xffff))
    })
}

/// What is being done with the open conversation's messages: which one is
/// answered, which one has its reactions open (beside it, with a mouse),
/// which one has its menu open (by touch).
#[derive(Clone, Copy)]
struct Thread {
    replying: RwSignal<Option<u32>>,
    reacting: RwSignal<Option<u32>>,
    menu: RwSignal<Option<i64>>,
    /// Which one shows all its reactions, until a click anywhere else.
    every: RwSignal<Option<i64>>,
    /// Which one has its details open.
    details: RwSignal<Option<i64>>,
    /// When each of ours on a MeshCore channel was last sent again.
    retried: RwSignal<std::collections::HashMap<i64, u32>>,
}

/// What became of ours on a MeshCore channel, which nothing acknowledges:
/// waited on, heard carried by so many repeaters, or, after a while with
/// none, taken as gone nowhere.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Carried {
    Waiting,
    By(u32),
    Nowhere,
}

/// How far one's own message went, as its mark says it, the same on both
/// kinds of radio: not in the radio yet; out, with nobody known to have
/// heard it; got by someone, a node that passed it on along a channel or
/// the one it went to; heard by nobody; or failed otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Went {
    Pending,
    Out,
    /// Out, a Meshtastic direct message a relay passed on: still on its way.
    OnItsWay,
    Got,
    Delivered,
    Unheard,
    Failed,
}

impl Went {
    /// A status read so. A Meshtastic radio has it the moment it is handed
    /// over; another node passing on a channel message of it is someone who
    /// got it. Given up on as nobody answered, it was heard by nobody.
    fn of(meshcore: bool, status: MessageStatus, error: Option<&str>, to_channel: bool) -> Self {
        match status {
            MessageStatus::Sending if meshcore => Self::Pending,
            MessageStatus::Sending => Self::Out,
            MessageStatus::Sent if meshcore => Self::Out,
            MessageStatus::Sent if to_channel => Self::Got,
            MessageStatus::Sent => Self::OnItsWay,
            MessageStatus::Delivered => Self::Delivered,
            MessageStatus::Failed
                if matches!(error, Some("MAX_RETRANSMIT" | "NO_RESPONSE" | "TIMEOUT")) =>
            {
                Self::Unheard
            }
            _ => Self::Failed,
        }
    }

    /// Its mark: the icon, its class, and its words.
    fn look(self, i18n: I18nContext<Locale>) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Pending => (CLOCK, "pending", t_string!(i18n, chat_sending)),
            Self::Out => (CHECK, "sent", t_string!(i18n, chat_sent)),
            Self::OnItsWay => (CHECK, "sent", t_string!(i18n, chat_on_its_way)),
            Self::Got => (CHECKS, "delivered", t_string!(i18n, chat_got)),
            Self::Delivered => (CHECKS, "delivered", t_string!(i18n, chat_delivered)),
            Self::Unheard => (UNHEARD, "unheard", t_string!(i18n, chat_nobody)),
            Self::Failed => (ALERT, "failed", t_string!(i18n, chat_failed)),
        }
    }
}

/// How far one of ours went, as the selected radio tells it.
fn went(live: Live, message: &meshctl_api::Message) -> Went {
    let meshcore = untrack(|| live.selected_kind()) == Some(RadioKind::Meshcore);
    Went::of(
        meshcore,
        message.status,
        message.error.as_deref(),
        message.to == meshctl_api::BROADCAST,
    )
}

/// What became of a message, if it is ours on a MeshCore channel: counted
/// from when it was sent, or sent again.
fn carried(live: Live, thread: Thread, id: i64) -> Option<Carried> {
    let meshcore = untrack(|| live.selected_kind()) == Some(RadioKind::Meshcore);
    let (at, repeaters) = live.thread.with(|t| {
        t.iter()
            .find(|m| m.id == id)
            .filter(|m| {
                meshcore
                    && m.outgoing
                    && m.to == meshctl_api::BROADCAST
                    && m.status == MessageStatus::Sent
            })
            .map(|m| (u32::try_from(m.at).unwrap_or(0), m.repeaters))
    })?;
    if repeaters > 0 {
        return Some(Carried::By(repeaters));
    }
    let from = thread
        .retried
        .with(|r| r.get(&id).copied())
        .unwrap_or(at)
        .max(at);
    Some(if live.now.get().saturating_sub(from) >= UNHEARD_AFTER {
        Carried::Nowhere
    } else {
        Carried::Waiting
    })
}

/// Sends again, the same packet, one of ours on a MeshCore channel taken as
/// gone nowhere; then waits on it as long again.
fn send_again(live: Live, thread: Thread, id: i64) {
    let Some(radio) = live.selected.get_untracked() else {
        return;
    };
    thread
        .retried
        .update(|r| _ = r.insert(id, crate::app::now()));
    spawn_local(async move {
        let _ = daemon::resend_message(radio, id).await;
    });
}

/// Whether the screen is touched rather than pointed at with a mouse.
fn by_touch() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Reacts to a message, once with each emoji: counted among the most used,
/// and gone out as a message that answers it.
fn react_to(live: Live, peer: Peer, packet: u32, emoji: String) {
    // One of each: the same again says nothing new.
    let given = live.thread.with_untracked(|t| {
        t.iter()
            .any(|m| m.outgoing && m.emoji && m.reply_id == Some(packet) && m.text == emoji)
    });
    if given {
        return;
    }
    spawn_local(async move {
        if let Ok(emojis) = daemon::use_emoji(&emoji).await {
            live.emojis.set(Some(emojis));
        }
        let _ = send(live, peer, emoji, Some(packet), true, None).await;
    });
}

/// Answers a message: said over the field, and into it, but not by touch,
/// where the keyboard would cover what is answered.
fn reply_to(thread: Thread, packet: u32) {
    thread.replying.set(Some(packet));
    if !by_touch()
        && let Some(field) = document()
            .query_selector(".composer textarea")
            .ok()
            .flatten()
            .and_then(|f| f.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = field.focus();
    }
}

/// Sends one's own message again, here: the same text, answering what it
/// answered, in the region it went in, as a new message.
fn resend(live: Live, peer: Peer, text: String, reply_id: Option<u32>, scope: Option<String>) {
    spawn_local(async move {
        let _ = send(live, peer, text, reply_id, false, scope).await;
    });
}

/// Forgets a message, once sure: only here, as what went out on the mesh
/// cannot be taken back.
fn forget(live: Live, i18n: I18nContext<Locale>, id: MessageId) {
    let sure = web_sys::window()
        .and_then(|w| {
            w.confirm_with_message(t_string!(i18n, chat_delete_confirm))
                .ok()
        })
        .unwrap_or(false);
    let Some(radio) = live.selected.get_untracked() else {
        return;
    };
    if sure {
        spawn_local(async move {
            if daemon::forget_message(radio, id).await.is_ok() {
                live.thread.update(|t| t.retain(|m| m.id != id));
            }
        });
    }
}

/// Whether the radio has replies and reactions: Meshtastic has them in its
/// packets; MeshCore's apps write them in the text, and so does meshctl.
fn answerable(live: Live) -> bool {
    live.selected_kind().is_some()
}

/// Whether a conversation has reactions: not a room, as MeshCore's rooms
/// have none.
fn reactable(live: Live, peer: Peer) -> bool {
    !matches!(peer, Peer::Node { node } if live.is_room(node))
}

/// A message's text, each `@[Name]` in it said as a mention; and, for a
/// reply shown under its quote, the one it starts with left out, as the
/// quote says whom it answers.
fn said(text: &str, quoted: bool) -> AnyView {
    let mut text = text;
    if quoted
        && text.starts_with("@[")
        && let Some(close) = text.find(']')
    {
        let rest = &text[close + 1..];
        text = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    }
    let mut parts: Vec<AnyView> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("@[") {
        let Some(close) = rest[open..].find(']').map(|c| open + c) else {
            break;
        };
        let name = &rest[open + 2..close];
        if name.is_empty() || name.contains('\n') {
            break;
        }
        parts.push(rest[..open].to_owned().into_any());
        parts.push(view! { <span class="mention">"@" {name.to_owned()}</span> }.into_any());
        rest = &rest[close + 1..];
    }
    parts.push(rest.to_owned().into_any());
    parts.into_iter().collect_view().into_any()
}

/// Sends a message, and puts it in the open conversation as sent.
async fn send(
    live: Live,
    peer: Peer,
    text: String,
    reply_id: Option<u32>,
    emoji: bool,
    scope: Option<String>,
) -> Result<(), Failure> {
    let radio = live.selected.get_untracked().ok_or(Failure::Unreachable)?;
    let (channel, node) = match peer {
        Peer::Channel { channel } => (Some(channel), None),
        Peer::Node { node } => (None, Some(node)),
    };
    let message = NewMessage {
        text,
        channel,
        node,
        reply_id,
        emoji,
        scope,
    };
    let sent = daemon::send_message(radio, &message).await?;
    live.thread.update(|thread| {
        if !thread.iter().any(|m| m.id == sent.id) {
            thread.push(sent);
        }
    });
    Ok(())
}

/// Who wrote a message, as the conversation shows it.
fn author_of(live: Live, message: &Message) -> String {
    if message.outgoing {
        return live.status.with(|s| {
            s.as_ref()
                .and_then(|s| s.local.as_ref())
                .map_or_else(String::new, |l| node_name(live, l.num))
        });
    }
    match (&message.sender_name, message.from) {
        (Some(name), 0) => name.clone(),
        _ => node_name(live, message.author.unwrap_or(message.from)),
    }
}

/// What a conversation shows: the day, where it changes; where what was not
/// read starts; and what is said.
#[derive(Clone)]
enum Item {
    /// A day, as [`day_of`] gives it.
    Day(i64),
    Unread,
    /// A message, and whether it starts a run of its author's: only that
    /// one says who it is.
    Said(Message, bool),
}

impl Item {
    fn key(&self) -> (i64, i64, MessageStatus, bool) {
        match self {
            Self::Day(day) => (*day, 0, MessageStatus::Received, false),
            Self::Unread => (-1, 0, MessageStatus::Received, false),
            Self::Said(m, first) => (0, m.id, m.status, *first),
        }
    }
}

/// How long apart two messages of the same author may be and still be one
/// run of theirs.
const RUN: i64 = 10 * 60;

/// How far from the newest, in pixels, reads as away from it.
const AWAY: i32 = 240;

/// The local day something happened, as year, month and day in a number:
/// 20261123 for 23 November 2026.
fn day_of(at: i64) -> i64 {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64 * 1000.0));
    i64::from(date.get_full_year()) * 10_000
        + i64::from(date.get_month() + 1) * 100
        + i64::from(date.get_date())
}

/// The hour and minute of something, as the time a message shows.
fn clock(at: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64 * 1000.0));
    format!("{:02}:{:02}", date.get_hours(), date.get_minutes())
}

/// A day as a conversation names it: today, yesterday, the day of the week
/// within a week, the day and month this year, and with the year before.
fn day_name(i18n: I18nContext<Locale>, day: i64, now: u32) -> String {
    let (year, month, date) = (day / 10_000, day / 100 % 100, day % 100);
    let midnight = |y: f64, m: f64, d: f64| {
        js_sys::Date::new_with_year_month_day(y as u32, m as i32, d as i32)
    };
    let then = midnight(year as f64, (month - 1) as f64, date as f64);
    let today = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(f64::from(now) * 1000.0));
    let today = midnight(
        f64::from(today.get_full_year()),
        f64::from(today.get_month()),
        f64::from(today.get_date()),
    );
    let days = ((today.get_time() - then.get_time()) / 86_400_000.0).round() as i64;
    let language = match i18n.get_locale() {
        Locale::es => "es-ES",
        Locale::en => "en-GB",
    };
    let words = |fields: &[(&str, &str)]| {
        let options = js_sys::Object::new();
        for (key, value) in fields {
            let _ = js_sys::Reflect::set(&options, &(*key).into(), &(*value).into());
        }
        String::from(then.to_locale_date_string(language, &options))
    };
    let named = match days {
        0 => return t_string!(i18n, chat_today).to_owned(),
        1 => return t_string!(i18n, chat_yesterday).to_owned(),
        2..=6 => words(&[("weekday", "long")]),
        _ if i64::from(today.get_full_year()) == year => {
            words(&[("day", "numeric"), ("month", "long")])
        }
        _ => words(&[("day", "numeric"), ("month", "long"), ("year", "numeric")]),
    };
    // "sábado" as a heading is "Sábado".
    let mut letters = named.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// Where the day changes, a faint line and the day's name, which follows
/// the clock: today becomes yesterday at midnight.
#[component]
fn DayLine(day: i64) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    view! {
        <div class="day-line" role="separator">
            <span>{move || day_name(i18n, day, live.now.get())}</span>
        </div>
    }
}

/// The open conversation. Which ones there are is in the sidebar.
#[component]
pub fn ChatView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let list = NodeRef::<html::Div>::new();
    provide_context(Thread {
        replying: RwSignal::new(None),
        reacting: RwSignal::new(None),
        menu: RwSignal::new(None),
        every: RwSignal::new(None),
        details: RwSignal::new(None),
        retried: RwSignal::new(std::collections::HashMap::new()),
    });
    // A click anywhere else closes the reactions; another conversation
    // starts with nothing answered.
    let state = expect_context::<Thread>();
    let closing = window_event_listener(leptos::ev::click, move |_| {
        state.reacting.set(None);
        state.every.set(None);
    });
    on_cleanup(move || closing.remove());
    Effect::new(move |_| {
        live.open.track();
        state.replying.set(None);
        state.reacting.set(None);
        state.menu.set(None);
    });
    // Where what was not read started when the conversation was opened: it
    // stays there while it is, though it is read meanwhile.
    let band = RwSignal::new(None::<MessageId>);
    Effect::new(move |_| {
        let open = live.open.get();
        let unread = live.conversations.with_untracked(|c| {
            c.iter()
                .find(|c| Some(c.peer) == open)
                .filter(|c| c.unread > 0)
                .map(|c| c.read_up_to.unwrap_or(0))
        });
        band.set(unread);
    });
    // A reaction goes under what it reacts to; one to a message not here,
    // as a message. Where the day changes, the day; where what was not read
    // starts, a line.
    let shown = move || {
        let band = band.get();
        live.thread.with(|thread| {
            let mut items = Vec::new();
            let mut last_day = None;
            let mut banded = false;
            // Who said the one before, and when, while it is a run.
            let mut before: Option<(u32, Option<String>, i64)> = None;
            for m in thread.iter().filter(|m| {
                !(m.emoji
                    && m.reply_id
                        .is_some_and(|to| thread.iter().any(|t| t.packet_id == to)))
            }) {
                let day = day_of(m.at);
                if last_day != Some(day) {
                    items.push(Item::Day(day));
                    last_day = Some(day);
                    before = None;
                }
                if !banded && band.is_some_and(|read| m.id > read && !m.outgoing && !m.emoji) {
                    items.push(Item::Unread);
                    banded = true;
                    before = None;
                }
                let author =
                    (!m.outgoing).then(|| (m.author.unwrap_or(m.from), m.sender_name.clone()));
                let first = match (&before, &author) {
                    (Some((from, name, at)), Some((m_from, m_name))) => {
                        from != m_from || name != m_name || m.at - at > RUN
                    }
                    _ => true,
                };
                before = author.map(|(from, name)| (from, name, m.at));
                items.push(Item::Said(m.clone(), first));
            }
            items
        })
    };

    // Down at the newest as they come, while there: away from it, reading
    // what came before, the thread stays, and what comes is counted. What
    // is sent, and another conversation, go down anyway.
    let away = RwSignal::new(false);
    let fresh = RwSignal::new(0_usize);
    let seen = StoredValue::new((None::<Peer>, 0_usize));
    // Whether the open conversation was placed yet, once its messages came:
    // on the line where what was not read starts, or at its newest.
    let placed = StoredValue::new(false);
    let to_latest = move |smooth: bool| {
        if let Some(list) = list.try_get_untracked().flatten() {
            let options = web_sys::ScrollToOptions::new();
            options.set_top(f64::from(list.scroll_height()));
            if smooth {
                options.set_behavior(web_sys::ScrollBehavior::Smooth);
            }
            list.scroll_to_with_scroll_to_options(&options);
        }
        fresh.set(0);
    };
    Effect::new(move |_| {
        let (said, ours) = live.thread.with(|t| {
            (
                t.iter().filter(|m| !m.emoji).count(),
                t.last().is_some_and(|m| m.outgoing),
            )
        });
        let open = live.open.get();
        list.track();
        let (before, counted) = seen.get_value();
        seen.set_value((open, said));
        let elsewhere = before != open;
        if elsewhere {
            away.set(false);
            placed.set_value(false);
        }
        if said > 0 && !placed.get_value() {
            placed.set_value(true);
            request_animation_frame(move || {
                let line = document()
                    .query_selector(".messages .unread-line")
                    .ok()
                    .flatten();
                match (line, list.try_get_untracked().flatten()) {
                    (Some(line), Some(list)) => {
                        let line: web_sys::HtmlElement = line.unchecked_into();
                        list.set_scroll_top(line.offset_top() - 12);
                        let left = list.scroll_height() - list.scroll_top() - list.client_height();
                        away.set(left > AWAY);
                    }
                    _ => to_latest(false),
                }
            });
        } else if elsewhere || !away.get_untracked() || ours {
            request_animation_frame(move || to_latest(false));
        } else if said > counted {
            fresh.update(|f| *f += said - counted);
        }
    });
    let shown_now = RwSignal::new(!document().hidden());
    let showing = window_event_listener_untyped("visibilitychange", move |_| {
        shown_now.set(!document().hidden());
    });
    on_cleanup(move || showing.remove());
    // Read as it goes: up to the last message seen whole, while shown; down
    // at the newest, all of it. Each conversation its own: how far it was
    // asked to be read, not to ask twice.
    let marked = StoredValue::new((None::<Peer>, 0_i64));
    let mark_seen = move || {
        let (Some(peer), Some(radio), Some(list)) = (
            live.open.get_untracked(),
            live.selected.get_untracked(),
            list.try_get_untracked().flatten(),
        ) else {
            return;
        };
        if !shown_now.get_untracked() {
            return;
        }
        let seen = if away.get_untracked() {
            let bottom = list.get_bounding_client_rect().bottom() + 1.0;
            let rows = list.query_selector_all(".bubble-row[data-id]").ok();
            let mut seen = None;
            for i in 0..rows.as_ref().map_or(0, web_sys::NodeList::length) {
                let Some(row) = rows
                    .as_ref()
                    .and_then(|r| r.item(i))
                    .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
                else {
                    continue;
                };
                if row.get_bounding_client_rect().bottom() <= bottom {
                    let id = row
                        .get_attribute("data-id")
                        .and_then(|id| id.parse::<i64>().ok());
                    seen = seen.max(id);
                }
            }
            seen
        } else {
            live.thread.with_untracked(|t| t.iter().map(|m| m.id).max())
        };
        let read = live.conversations.with_untracked(|c| {
            c.iter()
                .find(|c| c.peer == peer)
                .and_then(|c| c.read_up_to)
                .unwrap_or(0)
        });
        let (asked_for, asked) = marked.get_value();
        let asked = if asked_for == Some(peer) { asked } else { 0 };
        if let Some(seen) = seen.filter(|seen| *seen > read.max(asked)) {
            marked.set_value((Some(peer), seen));
            spawn_local(async move {
                let _ = daemon::mark_read(radio, peer, seen).await;
            });
        }
    };
    // While scrolled, at most every so often.
    let waiting = StoredValue::new(false);
    let mark_soon = move || {
        if !waiting.get_value() {
            waiting.set_value(true);
            set_timeout(
                move || {
                    waiting.set_value(false);
                    mark_seen();
                },
                Duration::from_millis(400),
            );
        }
    };
    Effect::new(move |_| {
        let reading = live.open.get().is_some() && !away.get() && shown_now.get();
        live.reading.set(reading);
        live.thread.track();
        // Once laid out.
        request_animation_frame(mark_seen);
    });
    on_cleanup(move || live.reading.set(false));
    // A room open: its login read, and made with the password kept if the
    // radio has not logged in since it connected; again each time it syncs.
    let open_room = Memo::new(move |_| match live.open.get() {
        Some(Peer::Node { node }) if live.is_room(node) => Some(node),
        _ => None,
    });
    // What the open room allows, drawn anew only when that changes: not at
    // each node heard, nor as a login is under way, which would lose what
    // was typed.
    let room_gone =
        Memo::new(move |_| open_room.get().is_some_and(|n| crate::rooms::gone(live, n)));
    let room_posting = Memo::new(move |_| {
        open_room
            .get()
            .is_some_and(|n| crate::rooms::can_post(live, n))
    });
    Effect::new(move |_| {
        live.synced.track();
        match open_room.get() {
            Some(node) => live.read_room(node),
            None => live.room.set(None),
        }
    });
    let scrolled = move |_| {
        mark_soon();
        let Some(list) = list.try_get_untracked().flatten() else {
            return;
        };
        let left = list.scroll_height() - list.scroll_top() - list.client_height();
        let now_away = left > AWAY;
        away.set(now_away);
        if !now_away {
            fresh.set(0);
        }
    };

    view! {
        <div class="thread">
            {move || match live.open.get() {
                None => view! { <p class="empty dim">{t!(i18n, chat_pick)}</p> }.into_any(),
                Some(peer) => view! {
                    <Show when=move || away.get()>
                        <button type="button" class="to-latest" on:click=move |_| to_latest(true)>
                            <span class="icon" inner_html=DOWN></span>
                            {move || match fresh.get() {
                                0 => t_string!(i18n, chat_to_latest).to_owned(),
                                1 => t_string!(i18n, chat_new_one).to_owned(),
                                n => t_string!(i18n, chat_new_many).replace("{n}", &n.to_string()),
                            }}
                        </button>
                    </Show>
                    // A login to the room under way: in the middle, counting down.
                    {move || match open_room.get() {
                        Some(node) if crate::rooms::logging_in(live, node) => {
                            Some(view! { <crate::rooms::LoggingIn node /> })
                        }
                        _ => None,
                    }}
                    <div class="messages" node_ref=list on:scroll=scrolled>
                        <For each=shown key=Item::key let:item>
                            {match item {
                                Item::Day(day) => view! { <DayLine day /> }.into_any(),
                                Item::Unread => view! {
                                    <div class="unread-line" role="separator">
                                        <span>{t!(i18n, chat_unread)}</span>
                                    </div>
                                }
                                .into_any(),
                                Item::Said(message, first) => {
                                    view! { <Bubble message peer first /> }.into_any()
                                }
                            }}
                        </For>
                    </div>
                    {move || match peer {
                        // A room the radio no longer has: said so, and offered
                        // to be removed.
                        Peer::Node { node } if room_gone.get() && open_room.get() == Some(node) => {
                            view! { <crate::rooms::RoomGone node /> }.into_any()
                        }
                        // A room, while the radio cannot post in it: why, and
                        // its password.
                        Peer::Node { node } if !room_posting.get() && open_room.get() == Some(node) => {
                            view! { <crate::rooms::RoomBar node /> }.into_any()
                        }
                        _ => view! { <Composer peer /> }.into_any(),
                    }}
                    <MessageMenu peer />
                    <MessageDetails peer />
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn Bubble(message: Message, peer: Peer, first: bool) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let at = u32::try_from(message.at).unwrap_or_default();
    let mine = message.outgoing;
    // In a group, a channel or a room, who said it, on the first of a run of
    // theirs; in a direct conversation it is obvious. A room's post comes
    // from the room, and says who wrote it.
    let group = !mine && (matches!(peer, Peer::Channel { .. }) || message.author.is_some());
    // A sender known only by the name the message gives, as on MeshCore
    // channels, is shown by that name. As the node becomes known, its names.
    let from = message.author.unwrap_or(message.from);
    let known = from != 0;
    let given = message.sender_name.clone();
    let name = move || match (&given, known) {
        (Some(name), false) => name.clone(),
        _ => node_name(live, from),
    };
    let author = (group && first).then(|| view! { <span class="author">{name.clone()}</span> });
    // Their short name in an avatar, as the list of nodes draws it, coloured
    // by how far they are: their page, a click away. The rest of a run
    // leaves its room.
    let avatar = group.then(|| {
        if !first {
            return view! { <span class="bubble-avatar gap" aria-hidden="true"></span> }.into_any();
        }
        let name = name.clone();
        let short = {
            let name = name.clone();
            move || {
                let short = if known {
                    short_name(live, from)
                } else {
                    String::new()
                };
                let short = if short.is_empty() {
                    crate::badge::initials(&name())
                } else {
                    short
                };
                crate::badge::fit(&short)
            }
        };
        let class = {
            let short = short.clone();
            move || {
                let kind = live.nodes.with(|nodes| {
                    nodes
                        .get(&from)
                        .map(|node| crate::mesh::avatar_class(node, false))
                });
                let mut class = kind.unwrap_or_else(|| "node-avatar".to_owned());
                if crate::badge::is_emoji(&short()) {
                    class.push_str(" emoji");
                }
                format!("bubble-avatar {class}")
            }
        };
        let page = move |ev: leptos::ev::MouseEvent| {
            ev.stop_propagation();
            if known && live.nodes.with_untracked(|nodes| nodes.contains_key(&from)) {
                live.open_node(from);
            }
        };
        view! {
            <button
                type="button"
                class=class
                title=name.clone()
                aria-label=name
                disabled=!known
                on:click=page
            >
                {short}
            </button>
        }
        .into_any()
    });
    // How it came, in words: through MQTT, directly with its signal, or
    // through so many hops.
    let reception = (!mine).then(|| {
        let (mqtt, hops, snr) = (message.via_mqtt, message.hops_away, message.snr);
        move || reach(i18n, mqtt, hops, snr)
    });
    let thread = expect_context::<Thread>();
    // Ours on a MeshCore channel, which nothing acknowledges: what became of
    // it, as heard after it was drawn; drawn anew only when that changes,
    // not at each tick.
    let sent_id = message.id;
    let carried = (mine && message.status == MessageStatus::Sent)
        .then(|| Memo::new(move |_| carried(live, thread, sent_id)))
        .filter(|c| c.get_untracked().is_some());
    // How far it went, as an icon: waiting, out, received, or not; its words
    // on hovering, and why it failed, when it did. Ours on a MeshCore
    // channel, out and waited on, then carried by so many repeaters or by
    // none.
    let status = message.status;
    let error = message.error.clone();
    let mark = (status != MessageStatus::Received).then(|| {
        if let Some(carried) = carried {
            return (move || match carried.get() {
                Some(Carried::By(n)) => {
                    let words = t_string!(i18n, chat_repeaters).replace("{n}", &n.to_string());
                    view! {
                        <span class="repeated" title=words.clone() aria-label=words>
                            <span class="mark delivered" inner_html=CHECKS></span>
                            {n}
                        </span>
                    }
                    .into_any()
                }
                Some(Carried::Nowhere) => {
                    let words = t_string!(i18n, chat_unheard);
                    view! {
                        <span class="mark unheard" title=words aria-label=words inner_html=UNHEARD></span>
                    }
                    .into_any()
                }
                _ => {
                    let words = t_string!(i18n, chat_waiting_repeaters);
                    view! {
                        <span class="mark sent" title=words aria-label=words inner_html=CHECK></span>
                    }
                    .into_any()
                }
            })
            .into_any();
        }
        let went = went(live, &message);
        let (icon, class, _) = went.look(i18n);
        let words = move || {
            let (_, _, words) = went.look(i18n);
            match &error {
                Some(error) if went == Went::Failed => format!("{words}: {error}"),
                _ => words.to_owned(),
            }
        };
        view! {
            <span class=format!("mark {class}") title=words.clone() aria-label=words inner_html=icon></span>
        }
        .into_any()
    });

    let id = message.id;
    let packet = message.packet_id;
    // Replies and reactions refer to its packet: one it has, on a radio
    // that has them.
    let can_answer = packet != 0 && untrack(move || answerable(live));
    let can_react = can_answer && untrack(move || reactable(live, peer));
    // What it answers, quoted: who, and the start of it.
    let quote = message.reply_id.filter(|_| !message.emoji).map(|to| {
        move || {
            let answered = live
                .thread
                .with(|t| t.iter().find(|m| m.packet_id == to).cloned());
            match answered {
                Some(answered) => view! {
                    <div class="quote">
                        <span class="who">{author_of(live, &answered)}</span>
                        <span class="what">{answered.text.clone()}</span>
                    </div>
                }
                .into_any(),
                None => view! {
                    <div class="quote">
                        <span class="what dim">{t!(i18n, chat_reply_unknown)}</span>
                    </div>
                }
                .into_any(),
            }
        }
    });
    let react = move |emoji: String| {
        thread.reacting.set(None);
        react_to(live, peer, packet, emoji);
    };
    // The reactions to it, each emoji once, the most given first, how many
    // and who on hovering; one more of the same at a touch, unless it is
    // ours already. A few, and the rest behind one more.
    let reactions = move || {
        let mut by: Vec<(String, Vec<String>, bool)> = Vec::new();
        live.thread.with(|t| {
            for m in t
                .iter()
                .filter(|m| packet != 0 && m.emoji && m.reply_id == Some(packet))
            {
                let who = author_of(live, m);
                match by.iter_mut().find(|(emoji, _, _)| *emoji == m.text) {
                    Some(seen) => {
                        seen.1.push(who);
                        seen.2 |= m.outgoing;
                    }
                    None => by.push((m.text.clone(), vec![who], m.outgoing)),
                }
            }
        });
        if by.is_empty() {
            return None;
        }
        by.sort_by_key(|(_, who, _)| std::cmp::Reverse(who.len()));
        let hidden = if thread.every.get() == Some(id) {
            0
        } else {
            by.len().saturating_sub(REACTIONS_SHOWN)
        };
        let shown = by.len() - hidden;
        let chips = by
            .into_iter()
            .take(shown)
            .map(|(emoji, who, ours)| {
                let count = who.len();
                let again = emoji.clone();
                view! {
                    <button
                        type="button"
                        class="reaction"
                        class:ours=ours
                        title=who.join(", ")
                        disabled=ours || !can_react
                        on:click=move |ev| {
                            ev.stop_propagation();
                            react(again.clone());
                        }
                    >
                        {emoji}
                        {(count > 1).then(|| view! { <span class="count">{count}</span> })}
                    </button>
                }
            })
            .collect_view();
        let more = (hidden > 0).then(|| {
            view! {
                <button
                    type="button"
                    class="reaction more"
                    on:click=move |ev| {
                        ev.stop_propagation();
                        thread.every.set(Some(id));
                    }
                >
                    {format!("+{hidden}")}
                </button>
            }
        });
        Some(view! { <div class="reactions">{chips} {more}</div> })
    };
    // With a mouse, what can be done with it, at its foot on the right, on
    // hovering: answering it, reacting to it, forgetting it; one at a time,
    // any of the others closing the emojis to react with. By touch, its
    // menu instead.
    let actions = view! {
        <div class="answer" on:click=|ev| ev.stop_propagation()>
            {if let Some(carried) = carried {
                // Ours on a MeshCore channel: sent again, the same packet,
                // only once taken as gone nowhere.
                Some(
                    (move || {
                        (carried.get() == Some(Carried::Nowhere))
                            .then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="icon-button"
                                        title=move || t_string!(i18n, chat_resend)
                                        aria-label=move || t_string!(i18n, chat_resend)
                                        inner_html=RESEND
                                        on:click=move |_| {
                                            thread.reacting.set(None);
                                            send_again(live, thread, sent_id);
                                        }
                                    ></button>
                                }
                            })
                    })
                        .into_any(),
                )
            } else if mine && status == MessageStatus::Failed {
                // Ours that got nowhere: written anew, as a new message.
                let (again, answered, region) =
                    (message.text.clone(), message.reply_id, message.scope.clone());
                Some(
                    view! {
                        <button
                            type="button"
                            class="icon-button"
                            title=move || t_string!(i18n, chat_resend)
                            aria-label=move || t_string!(i18n, chat_resend)
                            inner_html=RESEND
                            on:click=move |_| {
                                thread.reacting.set(None);
                                resend(live, peer, again.clone(), answered, region.clone());
                            }
                        ></button>
                    }
                    .into_any(),
                )
            } else {
                // Others' answered; one's own, not.
                (can_answer && !mine).then(|| {
                    view! {
                        <button
                            type="button"
                            class="icon-button"
                            title=move || t_string!(i18n, chat_reply)
                            aria-label=move || t_string!(i18n, chat_reply)
                            inner_html=REPLY
                            on:click=move |_| {
                                thread.reacting.set(None);
                                reply_to(thread, packet);
                            }
                        ></button>
                    }
                    .into_any()
                })
            }}
            {can_react
                .then(|| {
                    view! {
                        <button
                            type="button"
                            class="icon-button"
                            class:on=move || thread.reacting.get() == Some(packet)
                            title=move || t_string!(i18n, chat_react)
                            aria-label=move || t_string!(i18n, chat_react)
                            inner_html=SMILE
                            on:click=move |_| {
                                thread.reacting.update(|r| {
                                    *r = if *r == Some(packet) { None } else { Some(packet) };
                                });
                            }
                        ></button>
                    }
                })}
            <button
                type="button"
                class="icon-button"
                title=move || t_string!(i18n, chat_delete)
                aria-label=move || t_string!(i18n, chat_delete)
                inner_html=TRASH
                on:click=move |_| {
                    // Its question asked once the emojis are gone from the
                    // screen, a frame drawn after: it holds the page still.
                    thread.reacting.set(None);
                    request_animation_frame(move || {
                        request_animation_frame(move || forget(live, i18n, id));
                    });
                }
            ></button>
        </div>
    };
    let picker = move || {
        (thread.reacting.get() == Some(packet))
            .then(|| view! { <ReactionPicker pick=Callback::new(react) /> })
    };

    view! {
        <div
            class="bubble-row"
            data-id=id
            class:mine=mine
            class:with-avatar=group
            class:goes-on=group && !first
            class:reacting=move || thread.reacting.get() == Some(packet) && packet != 0
        >
            {avatar}
            <div
                class=if mine { "bubble mine" } else { "bubble" }
                class:held=move || thread.menu.get() == Some(id)
                on:click=move |ev| {
                    // With a mouse, its details; by touch, its menu, which
                    // has them too.
                    if !by_touch() {
                        // Not when text of it was being selected.
                        let selecting = web_sys::window()
                            .and_then(|w| w.get_selection().ok().flatten())
                            .is_some_and(|s| !s.is_collapsed());
                        if !selecting {
                            thread.details.set(Some(id));
                        }
                    } else {
                        ev.stop_propagation();
                        thread.menu.set(Some(id));
                        // Another one's reactions, all shown, fold.
                        if thread.every.get_untracked() != Some(id) {
                            thread.every.set(None);
                        }
                    }
                }
            >
                {author}
                {quote}
                {if message.emoji {
                    // A reaction to a message not here: what it is.
                    view! {
                        <p class="orphan-reaction">
                            {message.text.clone()}
                            <span class="dim">{move || t_string!(i18n, chat_reaction_orphan)}</span>
                        </p>
                    }
                    .into_any()
                } else {
                    view! { <p>{said(&message.text, message.reply_id.is_some())}</p> }.into_any()
                }}
                // Its reactions on the line of its time, at the left.
                <div class="foot">
                    {reactions}
                    <span class="meta dim">
                        <span title=when(i64::from(at))>{clock(i64::from(at))}</span>
                        {reception.map(|r| move || r().map(|r| format!(" · {r}")))}
                        {message.scope.clone().map(|s| format!(" · #{s}"))}
                        {mark}
                    </span>
                </div>
                {actions}
            </div>
            {picker}
        </div>
    }
}

/// How a message came, in words: through MQTT, directly with its signal,
/// or through so many hops; none when that is not known.
fn reach(
    i18n: I18nContext<Locale>,
    mqtt: bool,
    hops: Option<u32>,
    snr: Option<f32>,
) -> Option<String> {
    if mqtt {
        return Some("MQTT".to_owned());
    }
    Some(match hops? {
        0 => match snr {
            Some(snr) => format!("{} · {snr:.1} dB", t_string!(i18n, chat_direct)),
            None => t_string!(i18n, chat_direct).to_owned(),
        },
        1 => t_string!(i18n, chat_hop).to_owned(),
        hops => t_string!(i18n, chat_hops).replace("{n}", &hops.to_string()),
    })
}

/// When, as people write it: day, month, year and time to the second.
fn when(at: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64 * 1000.0));
    format!(
        "{:02}/{:02}/{} {:02}:{:02}:{:02}",
        date.get_date(),
        date.get_month() + 1,
        date.get_full_year(),
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

/// All of a message, over the page: when, from whom and to whom, how far it
/// went or how it came (out of how many hops it could take, through whom
/// last, its signal, from the packet that carried it), and its reactions.
/// Escape, its ✕ or a click off it close it.
#[component]
fn MessageDetails(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let thread = expect_context::<Thread>();
    let packet = RwSignal::new(None::<meshctl_api::Packet>);
    // The ways it came, on a MeshCore radio: read again as more copies come.
    let routes = RwSignal::new(Vec::<meshctl_api::MessageRoute>::new());
    Effect::new(move |_| {
        let Some(id) = thread.details.get() else {
            routes.set(Vec::new());
            return;
        };
        let heard = live
            .thread
            .with(|t| t.iter().find(|m| m.id == id).map_or(0, |m| m.heard));
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        if heard == 0 {
            routes.set(Vec::new());
            return;
        }
        spawn_local(async move {
            if let Ok(read) = daemon::message_routes(radio, id).await
                && thread.details.get_untracked() == Some(id)
            {
                routes.set(read);
            }
        });
    });
    Effect::new(move |_| {
        packet.set(None);
        let (Some(id), Some(radio)) = (thread.details.get(), live.selected.get_untracked()) else {
            return;
        };
        // What was sent says no more than its status: only what was heard;
        // and a MeshCore radio keeps no packets, its routes tell instead.
        let heard = live
            .thread
            .with_untracked(|t| t.iter().any(|m| m.id == id && !m.outgoing));
        if !heard || untrack(|| live.selected_kind()) == Some(RadioKind::Meshcore) {
            return;
        }
        spawn_local(async move {
            if let Ok(heard) = daemon::message_packet(radio, id).await
                && thread.details.get_untracked() == Some(id)
            {
                packet.set(Some(heard));
            }
        });
    });
    let escaping = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            thread.details.set(None);
        }
    });
    on_cleanup(move || escaping.remove());
    let close = move || thread.details.set(None);
    move || {
        let id = thread.details.get()?;
        let message = live
            .thread
            .with(|t| t.iter().find(|m| m.id == id).cloned())?;
        let fact = |term: AnyView, value: AnyView| {
            view! {
                <div>
                    <dt>{term}</dt>
                    <dd>{value}</dd>
                </div>
            }
        };
        let from = if message.outgoing {
            t_string!(i18n, chat_this_radio).to_owned()
        } else {
            // By its name and its id; by its id alone while it has no name.
            let name = author_of(live, &message);
            let id = format!("!{:08x}", message.from);
            if message.from == 0 || name == id {
                name
            } else {
                format!("{name} ({id})")
            }
        };
        // The region it went flooded in, for one sent scoped to one.
        let scope_fact = message
            .scope
            .clone()
            .map(|s| fact(t!(i18n, chat_scope).into_any(), format!("#{s}").into_any()));
        let status = message.outgoing.then(|| {
            let went = went(live, &message);
            let (_, _, words) = went.look(i18n);
            let words = match &message.error {
                Some(error) if went == Went::Failed => format!("{words}: {error}"),
                _ => words.to_owned(),
            };
            fact(t!(i18n, chat_status).into_any(), words.into_any())
        });
        // What the packet says: received, how it came; sent, nothing more
        // than the status says.
        let heard = move || {
            let heard = packet.get()?;
            if message.outgoing {
                return None;
            }
            let hops = (!heard.via_mqtt && heard.hop_start > 0).then(|| {
                let taken = heard.hop_start.saturating_sub(heard.hop_limit);
                let of = heard.hop_start.to_string();
                let words = if taken == 0 {
                    t_string!(i18n, chat_direct_of).replace("{of}", &of)
                } else {
                    t_string!(i18n, chat_hops_of)
                        .replace("{n}", &taken.to_string())
                        .replace("{of}", &of)
                };
                fact(t!(i18n, chat_hops_taken).into_any(), words.into_any())
            });
            let relay = (heard.hop_start > heard.hop_limit && !heard.via_mqtt).then(|| {
                fact(
                    t!(i18n, packets_relay).into_any(),
                    heard
                        .relay_node
                        .map_or_else(
                            || t_string!(i18n, packets_relay_unknown).to_owned(),
                            |relay| {
                                // By its name and its byte; by its byte alone
                                // while it has no name.
                                let name = crate::packets::relay_name(live, relay);
                                let byte = format!("··{relay:02x}");
                                if name == byte {
                                    name
                                } else {
                                    format!("{name} ({byte})")
                                }
                            },
                        )
                        .into_any(),
                )
            });
            let signal = (!heard.via_mqtt)
                .then(|| {
                    let parts: Vec<String> = [
                        heard.snr.map(|s| format!("SNR {s:.1} dB")),
                        heard.rssi.map(|r| format!("RSSI {r} dBm")),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    (!parts.is_empty()).then(|| {
                        fact(
                            t!(i18n, chat_signal).into_any(),
                            parts.join(" · ").into_any(),
                        )
                    })
                })
                .flatten();
            let mqtt = heard
                .via_mqtt
                .then(|| fact(t!(i18n, chat_via).into_any(), "MQTT".into_any()));
            Some(view! { {hops} {relay} {signal} {mqtt} })
        };
        // Who reacted with what.
        let reactions = live.thread.with(|t| {
            t.iter()
                .filter(|m| {
                    message.packet_id != 0 && m.emoji && m.reply_id == Some(message.packet_id)
                })
                .map(|m| format!("{} {}", m.text, author_of(live, m)))
                .collect::<Vec<_>>()
        });
        let reactions = (!reactions.is_empty()).then(|| {
            fact(
                t!(i18n, chat_reactions).into_any(),
                view! {
                    <ul class="reacted">
                        {reactions.into_iter().map(|r| view! { <li>{r}</li> }).collect_view()}
                    </ul>
                }
                .into_any(),
            )
        });
        // Each copy heard, and the way it came.
        let outgoing = message.outgoing;
        // Where it set out from: the radio, for one of ours; its sender, if
        // known and placed.
        let local = live
            .status
            .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
        let origin = if outgoing {
            local
        } else {
            match peer {
                Peer::Node { node } => Some(node),
                Peer::Channel { .. } => (message.from != 0).then_some(message.from),
            }
        };
        let route_list = view! { <RouteList routes origin outgoing /> };
        let packet_id = (message.packet_id != 0).then(|| {
            fact(
                t!(i18n, chat_packet).into_any(),
                view! { <span class="mono">{format!("{:08x}", message.packet_id)}</span> }
                    .into_any(),
            )
        });
        Some(view! {
            <div class="modal-back" on:click=move |_| close()>
                <div
                    class="card message-details"
                    role="dialog"
                    aria-modal="true"
                    on:click=|ev| ev.stop_propagation()
                >
                    <header>
                        <h2>{t!(i18n, chat_details)}</h2>
                        <button
                            class="icon-button"
                            title=move || t_string!(i18n, drawer_close)
                            aria-label=move || t_string!(i18n, drawer_close)
                            inner_html=CLOSE
                            on:click=move |_| close()
                        ></button>
                    </header>
                    <div class="modal-body">
                        <p class="said">{message.text.clone()}</p>
                        <dl class="facts compact">
                            {fact(
                                if message.outgoing { t!(i18n, chat_sent_at).into_any() } else { t!(i18n, chat_heard_at).into_any() },
                                when(message.at).into_any(),
                            )}
                            {fact(t!(i18n, chat_from).into_any(), from.into_any())}
                            {fact(t!(i18n, chat_to).into_any(), title(live, peer).into_any())}
                            {status}
                            {scope_fact}
                            {heard}
                            {reactions}
                            {packet_id}
                        </dl>
                        {route_list}
                    </div>
                </div>
            </div>
        })
    }
}

/// What a hop of a route is known to be: a node only one known has its
/// hash, of two bytes or more, and the route agrees; a node guessed, among
/// several, or the only one known of a hash so short that it may be
/// another; or one not known.
enum HopIs {
    Sure(u32),
    Guess(u32),
    Unknown,
}

fn hop_is(hop: &meshctl_api::RouteHop) -> HopIs {
    match (hop.likely, hop.nodes.as_slice()) {
        (Some(num), [only]) if *only == num && hop.hash.len() >= 4 => HopIs::Sure(num),
        (Some(num), _) => HopIs::Guess(num),
        (None, _) => HopIs::Unknown,
    }
}

/// A route's hops one after the other: each by its node, sure, guessed, or
/// by its hash when it is not known which, with a title saying why; a
/// direct route's, which is not guessed, by how many it may be.
pub fn hop_chain(
    live: Live,
    i18n: I18nContext<Locale>,
    hops: &[meshctl_api::RouteHop],
    flooded: bool,
) -> impl IntoView + use<> {
    hops.iter()
        .enumerate()
        .map(|(at, hop)| {
            let known = hop.nodes.len();
            let fill = |text: &str| {
                text.replace("{n}", &known.to_string())
                    .replace("{hash}", &hop.hash)
            };
            let (name, title, guess, unsure) = match hop_is(hop) {
                HopIs::Sure(num) => (node_name(live, num), None, false, false),
                HopIs::Guess(num) => {
                    let why = if known == 1 {
                        fill(t_string!(i18n, route_guess_only))
                    } else {
                        fill(t_string!(i18n, route_guess))
                    };
                    (node_name(live, num), Some(why), true, false)
                }
                HopIs::Unknown if known == 0 => (
                    hop.hash.clone(),
                    Some(t_string!(i18n, route_unknown).to_owned()),
                    false,
                    true,
                ),
                // A direct route's, which is not guessed: how
                // many it may be.
                HopIs::Unknown if !flooded => (
                    fill(t_string!(i18n, route_many)),
                    Some(
                        hop.nodes
                            .iter()
                            .map(|n| node_name(live, *n))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    false,
                    true,
                ),
                HopIs::Unknown => (
                    hop.hash.clone(),
                    Some(fill(t_string!(i18n, route_misfit))),
                    false,
                    true,
                ),
            };
            view! {
                {(at > 0).then(|| view! { <span class="arrow" aria-hidden="true">"→"</span> })}
                <span class="hop" class:guess=guess class:unsure=unsure title=title>{name}</span>
            }
        })
        .collect_view()
}

/// Whether any of a route's hops is a guess, for the note that says so.
pub fn any_guessed(hops: &[meshctl_api::RouteHop]) -> bool {
    hops.iter().any(|h| matches!(hop_is(h), HopIs::Guess(_)))
}

/// Each copy heard of a message or a packet, and the way it came: every
/// repeater by its name, or by its hash and how many it may be; the one
/// picked on a small map, from where it set out, if known, to the radio.
#[component]
pub fn RouteList(
    routes: RwSignal<Vec<meshctl_api::MessageRoute>>,
    origin: Option<u32>,
    outgoing: bool,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let shown_route = RwSignal::new(0_usize);
    // One that is no more, as the copies are read again: the first.
    Effect::new(move |_| {
        let count = routes.with(Vec::len);
        if shown_route.get_untracked() >= count {
            shown_route.set(0);
        }
    });
    let local = live
        .status
        .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
    // The route shown, as points on the map: from its origin, through the
    // repeaters that can be placed, to the radio.
    let points = Signal::derive(move || {
        let at = shown_route.get();
        let Some(route) = routes.with(|r| r.get(at).cloned()) else {
            return Vec::new();
        };
        live.nodes.with(|nodes| {
            let point = |num: u32, gap: bool, guess: bool| {
                let node = nodes.get(&num)?;
                let at = node.position?;
                Some(crate::map::RoutePoint {
                    lat: at.latitude,
                    lon: at.longitude,
                    pin: crate::mesh::pin(node, Some(num) == local),
                    gap,
                    guess,
                })
            };
            let mut points: Vec<crate::map::RoutePoint> = Vec::new();
            let mut gap = false;
            let start = origin.and_then(|num| point(num, false, false));
            if start.is_none() {
                gap = true;
            }
            points.extend(start);
            for hop in &route.hops {
                let (num, guess) = match hop_is(hop) {
                    HopIs::Sure(num) => (num, false),
                    HopIs::Guess(num) => (num, true),
                    HopIs::Unknown => {
                        gap = true;
                        continue;
                    }
                };
                match point(num, gap && !points.is_empty(), guess) {
                    Some(p) => {
                        points.push(p);
                        gap = false;
                    }
                    None => gap = true,
                }
            }
            if let Some(end) = local.and_then(|num| point(num, gap && !points.is_empty(), false)) {
                points.push(end);
            }
            points
        })
    });
    move || {
        let all = routes.get();
        if all.is_empty() {
            return None;
        }
        let count = all.len();
        let guessed = all.iter().any(|r| any_guessed(&r.hops));
        let lines = all
            .into_iter()
            .enumerate()
            .map(|(index, route)| {
                let hops = hop_chain(live, i18n, &route.hops, route.flooded);
                let way = if route.hops.is_empty() {
                    if route.flooded {
                        t_string!(i18n, route_straight).to_owned()
                    } else {
                        t_string!(i18n, route_direct).to_owned()
                    }
                } else {
                    String::new()
                };
                let mut facts: Vec<String> = Vec::new();
                if let Some(snr) = route.snr {
                    facts.push(format!("{snr:.1} dB"));
                }
                if let Some(rssi) = route.rssi {
                    facts.push(format!("{rssi} dBm"));
                }
                match route.scope.as_deref() {
                    Some("?") => facts.push(t_string!(i18n, route_region_unknown).to_owned()),
                    Some(name) => facts.push(format!("#{name}")),
                    None => {}
                }
                // Picked for the map, when there are more than one.
                view! {
                    <li
                        class:pickable={count > 1}
                        class:on={move || count > 1 && shown_route.get() == index}
                        on:click=move |_| shown_route.set(index)
                    >
                        <span class="hops">
                            {(!way.is_empty()).then(|| view! { <span class="way">{way}</span> })}
                            {hops}
                        </span>
                        <span class="dim">{facts.join(" · ")}</span>
                    </li>
                }
            })
            .collect_view();
        Some(view! {
            <section class="routes">
                <h3>
                    {move || {
                        match (outgoing, count) {
                            (true, 1) => t_string!(i18n, route_echoes_one).to_owned(),
                            (false, 1) => t_string!(i18n, route_heard_one).to_owned(),
                            (true, n) => t_string!(i18n, route_echoes).replace("{n}", &n.to_string()),
                            (false, n) => t_string!(i18n, route_heard).replace("{n}", &n.to_string()),
                        }
                    }}
                </h3>
                <p class="hint dim">
                    {move || if outgoing { t!(i18n, route_echoes_hint).into_any() } else { t!(i18n, route_hint).into_any() }}
                </p>
                <ol>{lines}</ol>
                {guessed.then(|| view! { <p class="hint dim guessed">{t!(i18n, route_guess_hint)}</p> })}
                // On the map, while two of its points can be placed.
                {move || {
                    (points.with(Vec::len) >= 2)
                        .then(|| view! { <crate::map::RouteMap points /> })
                }}
                {move || {
                    (points.with(Vec::len) < 2)
                        .then(|| view! { <p class="hint dim">{t!(i18n, route_unplaced)}</p> })
                }}
            </section>
        })
    }
}

/// By touch, what can be done with a message, from below: reacting to it,
/// answering it, copying it, forgetting it.
#[component]
fn MessageMenu(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let thread = expect_context::<Thread>();
    let close = move || thread.menu.set(None);
    move || {
        let id = thread.menu.get()?;
        let message = live
            .thread
            .with_untracked(|t| t.iter().find(|m| m.id == id).cloned())?;
        let packet = message.packet_id;
        let can_answer = packet != 0 && answerable(live);
        let can_react = can_answer && reactable(live, peer);
        let text = message.text.clone();
        // Ours on a MeshCore channel: sent again, the same packet, only once
        // taken as gone nowhere; as the menu opened.
        let carried = untrack(|| carried(live, thread, id));
        Some(view! {
            <div class="sheet-back" on:click=move |_| close()>
                <div class="message-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                    {can_react
                        .then(|| {
                            view! {
                                <ReactionPicker pick=Callback::new(move |emoji: String| {
                                    close();
                                    react_to(live, peer, packet, emoji);
                                }) />
                            }
                        })}
                    {if let Some(carried) = carried {
                        (carried == Carried::Nowhere).then(|| {
                            view! {
                                <button role="menuitem" on:click=move |_| {
                                    close();
                                    send_again(live, thread, id);
                                }>
                                    <span class="icon" inner_html=RESEND></span>
                                    {t!(i18n, chat_resend)}
                                </button>
                            }
                            .into_any()
                        })
                    } else if message.outgoing && message.status == MessageStatus::Failed {
                        let (again, answered, region) =
                            (message.text.clone(), message.reply_id, message.scope.clone());
                        Some(
                            view! {
                                <button role="menuitem" on:click=move |_| {
                                    close();
                                    resend(live, peer, again.clone(), answered, region.clone());
                                }>
                                    <span class="icon" inner_html=RESEND></span>
                                    {t!(i18n, chat_resend)}
                                </button>
                            }
                            .into_any(),
                        )
                    } else {
                        // Others' answered; one's own, not.
                        (can_answer && !message.outgoing).then(|| {
                            view! {
                                <button role="menuitem" on:click=move |_| {
                                    close();
                                    reply_to(thread, packet);
                                }>
                                    <span class="icon" inner_html=REPLY></span>
                                    {t!(i18n, chat_reply)}
                                </button>
                            }
                            .into_any()
                        })
                    }}
                    <button role="menuitem" on:click=move |_| {
                        close();
                        crate::clipboard::copy(&text);
                    }>
                        <span class="icon" inner_html=COPY></span>
                        {t!(i18n, chat_copy)}
                    </button>
                    <button role="menuitem" on:click=move |_| {
                        close();
                        thread.details.set(Some(id));
                    }>
                        <span class="icon" inner_html=INFO></span>
                        {t!(i18n, chat_details)}
                    </button>
                    <button role="menuitem" class="danger-item" on:click=move |_| {
                        close();
                        forget(live, i18n, id);
                    }>
                        <span class="icon" inner_html=TRASH></span>
                        {t!(i18n, chat_delete)}
                    </button>
                </div>
            </div>
        })
    }
}

/// The emojis to react with: the most used, at a touch, and all of them
/// behind the last button.
/// How large the reaction picker gets with all the emojis, in pixels, a
/// margin beside: as the style sheet makes it, its grid always as tall.
const ALL_HEIGHT: f64 = 320.0;
const ALL_WIDTH: f64 = 350.0;

/// How far over or under the message's foot the picker starts, clear of
/// the buttons that hang from it, as the style sheet places it.
const CLEAR_ABOVE: f64 = 22.0;
const CLEAR_BELOW: f64 = 18.0;

#[component]
fn ReactionPicker(pick: Callback<String>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let all = RwSignal::new(false);
    // Over the message, or under it when the messages' scroll has no room
    // above; from its right edge leftwards, or from its left edge when there
    // is no room on the left, as for a narrow one of someone else's. Worked
    // out once, as it opens, for all of them, as large as it gets: the few
    // open on the side all would, so that nothing jumps from one to the
    // other.
    let element = NodeRef::<leptos::html::Div>::new();
    let below = RwSignal::new(false);
    let rightwards = RwSignal::new(false);
    Effect::new(move |_| {
        let Some(picker) = element.get() else {
            return;
        };
        request_animation_frame(move || {
            let picker: &web_sys::Element = &picker;
            let Some(message) = picker.parent_element() else {
                return;
            };
            let Some(scroll) = picker.closest(".messages").ok().flatten() else {
                return;
            };
            let (message, scroll) = (
                message.get_bounding_client_rect(),
                scroll.get_bounding_client_rect(),
            );
            // Clear of the buttons at the message's foot, either way.
            let above = message.bottom() - CLEAR_ABOVE - scroll.top();
            let under = scroll.bottom() - (message.bottom() + CLEAR_BELOW);
            below.set(ALL_HEIGHT > above && under > above);
            let leftwards = message.right() - scroll.left();
            let onwards = scroll.right() - message.left();
            rightwards.set(ALL_WIDTH > leftwards && onwards > leftwards);
        });
    });
    if live.emojis.get_untracked().is_none() {
        spawn_local(async move {
            if let Ok(emojis) = daemon::emojis().await {
                live.emojis.set(Some(emojis));
            }
        });
    }
    let offered = move || {
        let mut offered = live.emojis.get().unwrap_or_default();
        for common in crate::emoji::COMMON {
            if !offered.iter().any(|e| e == common) {
                offered.push(common.to_owned());
            }
        }
        offered
    };
    view! {
        <div
            class="reaction-picker"
            class:all=move || all.get()
            class:below=move || below.get()
            class:rightwards=move || rightwards.get()
            node_ref=element
            on:click=|ev| ev.stop_propagation()
        >
            {move || {
                if all.get() {
                    let frequent = Signal::derive(offered);
                    view! {
                        <crate::emoji::EmojiPicker
                            picked=Signal::stored(None::<String>)
                            pick=pick
                            frequent=frequent
                        />
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="quick">
                            {offered()
                                .into_iter()
                                .filter(|emoji| crate::emoji::draws(emoji))
                                .take(REACTIONS_OFFERED)
                                .map(|emoji| {
                                    let picked = emoji.clone();
                                    view! {
                                        <button type="button" on:click=move |_| pick.run(picked.clone())>
                                            {emoji}
                                        </button>
                                    }
                                })
                                .collect_view()}
                            <button
                                type="button"
                                class="more"
                                title=move || t_string!(i18n, chat_more_emojis)
                                aria-label=move || t_string!(i18n, chat_more_emojis)
                                inner_html=PLUS
                                on:click=move |_| all.set(true)
                            ></button>
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn Composer(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let thread = expect_context::<Thread>();
    let text = RwSignal::new(String::new());
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    let bytes = move || text.with(String::len);
    let over = move || bytes() > MAX_TEXT_BYTES;
    let connected = move || {
        live.status
            .with(|s| s.as_ref().is_some_and(|s| s.link == LinkState::Ready))
    };
    let can_send = move || {
        connected()
            && !busy.get()
            && !text.with(|t| t.trim().is_empty())
            && bytes() <= MAX_TEXT_BYTES
    };

    // As tall as what is written, wrapped even where it has no spaces, up to
    // a few lines; scrolled within past them. Again as it is emptied.
    let field = NodeRef::<html::Textarea>::new();
    let backdrop = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        text.track();
        field.track();
        // Measured once the field holds the text, which it takes after this.
        request_animation_frame(move || {
            // Unless it went in the meantime, as with its conversation.
            let Some(field) = field.try_get_untracked().flatten() else {
                return;
            };
            let style = web_sys::HtmlElement::style(&field);
            let _ = style.set_property("height", "auto");
            let border = field.offset_height() - field.client_height();
            let _ = style.set_property("height", &format!("{}px", field.scroll_height() + border));
        });
    });
    // By touch the keyboard's return writes a new line, as in the phones'
    // apps, and the button sends; with a keyboard, return sends, and with
    // Ctrl or Shift writes a new line.
    let touch = web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches());
    let keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() != "Enter" || ev.is_composing() || touch {
            return;
        }
        ev.prevent_default();
        if ev.ctrl_key() || ev.shift_key() || ev.meta_key() {
            // As if typed: where the caret is, undone as typing is.
            let _ = document().dyn_into::<web_sys::HtmlDocument>().map(|d| {
                d.exec_command_with_show_ui_and_value(
                    "insertText",
                    false,
                    "
",
                )
            });
        } else if let Some(form) = field.get().and_then(|f| f.form()) {
            let _ = form.request_submit();
        }
    };

    // The emojis, over the bar: each one picked goes where the caret is,
    // and they stay open for more, as in the phones' apps. Escape, a click
    // anywhere else or sending close them.
    let picking = RwSignal::new(false);
    let closing = window_event_listener(leptos::ev::click, move |_| picking.set(false));
    let escaping = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            // The emojis first, if open; what is answered, if not.
            if picking.get_untracked() {
                picking.set(false);
            } else {
                thread.replying.set(None);
            }
        }
    });
    on_cleanup(move || {
        closing.remove();
        escaping.remove();
    });
    // The most used, as they were when the emojis opened: what is picked
    // does not move under the finger, and comes forward the next time. The
    // common ones after, as many as make four rows.
    let frequent = RwSignal::new(Vec::<String>::new());
    let offer = move || {
        let mut offered = live.emojis.get_untracked().unwrap_or_default();
        for common in crate::emoji::COMMON {
            if offered.len() >= FREQUENT_SHOWN {
                break;
            }
            if !offered.iter().any(|e| e == common) {
                offered.push(common.to_owned());
            }
        }
        offered.truncate(FREQUENT_SHOWN);
        frequent.set(offered);
    };
    Effect::new(move |opened: Option<bool>| {
        let open = picking.get();
        if open && opened != Some(true) {
            offer();
            if live.emojis.get_untracked().is_none() {
                spawn_local(async move {
                    if let Ok(emojis) = daemon::emojis().await {
                        live.emojis.set(Some(emojis));
                        offer();
                    }
                });
            }
        }
        open
    });
    let insert = move |emoji: String| {
        let counted = emoji.clone();
        spawn_local(async move {
            if let Ok(emojis) = daemon::use_emoji(&counted).await {
                live.emojis.set(Some(emojis));
            }
        });
        let Some(field) = field.get_untracked() else {
            return;
        };
        let value = field.value();
        // Where the caret is, in the UTF-16 units the field counts in.
        let at = |units: Option<u32>| {
            let units = units.unwrap_or(u32::MAX) as usize;
            let mut counted = 0;
            value
                .char_indices()
                .find(|(_, c)| {
                    counted += c.len_utf16();
                    counted > units
                })
                .map_or(value.len(), |(at, _)| at)
        };
        let (start, end) = (
            at(field.selection_start().ok().flatten()),
            at(field.selection_end().ok().flatten()),
        );
        let written = format!("{}{emoji}{}", &value[..start], &value[end..]);
        let caret =
            u32::try_from(value[..start].encode_utf16().count() + emoji.encode_utf16().count())
                .unwrap_or_default();
        field.set_value(&written);
        text.set(written);
        let _ = field.set_selection_range(caret, caret);
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if !can_send() {
            return;
        }
        let reply_id = thread.replying.get_untracked();
        let written = text.get_untracked();
        busy.set(true);
        spawn_local(async move {
            match send(live, peer, written, reply_id, false, None).await {
                Ok(()) => {
                    text.set(String::new());
                    picking.set(false);
                    error.set(None);
                    thread.replying.set(None);
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };
    // What is answered, over the field: who, the start of it, and a way out.
    let replying = move || {
        let to = thread.replying.get()?;
        let answered = live
            .thread
            .with(|t| t.iter().find(|m| m.packet_id == to).cloned())?;
        let who = author_of(live, &answered);
        Some(view! {
            <div class="replying">
                <span class="icon" inner_html=REPLY></span>
                <div class="quote">
                    <span class="who">
                        {move || t_string!(i18n, chat_replying).replace("{name}", &who)}
                    </span>
                    <span class="what">{answered.text.clone()}</span>
                </div>
                <button
                    type="button"
                    class="icon-button"
                    title=move || t_string!(i18n, chat_reply_cancel)
                    aria-label=move || t_string!(i18n, chat_reply_cancel)
                    inner_html=CLOSE
                    on:click=move |_| thread.replying.set(None)
                ></button>
            </div>
        })
    };

    let problem = move || {
        if !connected() {
            return Some(t!(i18n, chat_not_connected).into_any());
        }
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::RadioNotConnected) => {
                t!(i18n, chat_not_connected).into_any()
            }
            Failure::Rejected(ErrorCode::InvalidMessage) => t!(i18n, chat_invalid).into_any(),
            _ => t!(i18n, error_failed).into_any(),
        })
    };

    // A node that tells nobody reads what is sent to it: said over where it
    // is written, and the button says it goes all the same.
    let unmessagable = move || match peer {
        Peer::Node { node } => live
            .nodes
            .with(|nodes| nodes.get(&node).is_some_and(|n| n.unmessagable)),
        Peer::Channel { .. } => false,
    };
    let send_label = move || {
        if unmessagable() {
            t_string!(i18n, chat_send_anyway)
        } else {
            t_string!(i18n, chat_send)
        }
    };

    view! {
        {move || {
            unmessagable()
                .then(|| {
                    view! {
                        <p class="unmessagable">
                            <span class="icon" inner_html=crate::mesh::MESSAGE_OFF></span>
                            {t!(i18n, chat_unmessagable)}
                        </p>
                    }
                })
        }}
        <form class="composer" on:submit=submit>
            {replying}
            // Behind what is written, the same text unseen but for what
            // goes past what a message takes, in red: a field cannot colour
            // a part of its own text.
            <div
                class="field"
                class:over=over
            >
                <div class="backdrop" node_ref=backdrop aria-hidden="true">
                    {move || {
                        let (fits, near, over) = text.with(|t| past_limit(t));
                        view! {
                            {fits}
                            <mark class="near">{near}</mark>
                            <mark class="over">{over}</mark>
                            // A last new line takes a line too.
                            " "
                        }
                    }}
                </div>
                <textarea
                    node_ref=field
                    rows="1"
                    placeholder=move || t_string!(i18n, chat_write)
                    prop:value=move || text.get()
                    on:input=move |ev| text.set(event_target_value(&ev))
                    on:keydown=keydown
                    on:scroll=move |_| {
                        if let (Some(field), Some(backdrop)) = (field.get_untracked(), backdrop.get_untracked()) {
                            backdrop.set_scroll_top(field.scroll_top());
                        }
                    }
                ></textarea>
                <button
                    type="button"
                    class="icon-button emoji-toggle"
                    class:on=move || picking.get()
                    aria-expanded=move || picking.get().to_string()
                    title=move || t_string!(i18n, chat_emoji)
                    aria-label=move || t_string!(i18n, chat_emoji)
                    inner_html=SMILE
                    on:click=move |ev| {
                        ev.stop_propagation();
                        picking.update(|p| *p = !*p);
                    }
                ></button>
            </div>
            <Show when=move || picking.get()>
                <div class="emoji-popover" on:click=|ev| ev.stop_propagation()>
                    <crate::emoji::EmojiPicker
                        picked=Signal::stored(None::<String>)
                        pick=Callback::new(insert)
                        frequent=Signal::from(frequent)
                    />
                </div>
            </Show>
            <span class=move || {
                if bytes() > MAX_TEXT_BYTES { "count over" } else { "count dim" }
            }>{move || format!("{}/{MAX_TEXT_BYTES}", bytes())}</span>
            // Its icon, and its word where there is room: sending anyway, to a
            // node that takes no messages, plain and not in the accent.
            <button
                type="submit"
                class="send"
                class:primary=move || !unmessagable()
                disabled=move || !can_send()
                title=send_label
                aria-label=send_label
            >
                <span class="icon" inner_html=SEND></span>
                <span class="label">{send_label}</span>
            </button>
            <p class="error">{problem}</p>
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TEXT_BYTES, NEAR_TEXT_BYTES, past_limit, preset_name};

    #[test]
    fn what_nears_and_goes_past_a_message_splits_at_a_character() {
        let fits = "a".repeat(NEAR_TEXT_BYTES);
        assert_eq!(
            past_limit(&fits),
            (fits.clone(), String::new(), String::new())
        );
        let near = "a".repeat(MAX_TEXT_BYTES);
        assert_eq!(
            past_limit(&near),
            (
                fits.clone(),
                "a".repeat(MAX_TEXT_BYTES - NEAR_TEXT_BYTES),
                String::new()
            )
        );
        let over = format!("{}ñb", "a".repeat(MAX_TEXT_BYTES - 1));
        // The ñ takes two bytes, one past: it goes, whole.
        assert_eq!(
            past_limit(&over),
            (
                fits,
                "a".repeat(MAX_TEXT_BYTES - NEAR_TEXT_BYTES - 1),
                "ñb".to_owned()
            )
        );
    }

    #[test]
    fn presets_read_as_people_write_them() {
        assert_eq!(preset_name("LONG_FAST"), "LongFast");
        assert_eq!(preset_name("NARROW_FAST"), "NarrowFast");
        assert_eq!(preset_name("LONG_MODERATE"), "LongMod");
    }
}
