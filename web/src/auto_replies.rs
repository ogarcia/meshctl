// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The automatic replies: their list, in the order they are tried in, and
//! the editor of each: what a message says to be answered, what it is
//! answered with, and where.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::radios::Switch;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::auto_replies::{MAX_TEXT_BYTES, Told, VARIABLES, hops_emoji, render};
use meshctl_api::{
    AutoReply, AutoReplyId, AutoReplyLimits, AutoReplyMatch, AutoReplyPlace, AutoReplySettings,
    Channel, ChannelBusy, ErrorCode, RadioId, RadioKind,
};
use std::collections::HashMap;

const GRIP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/grip-vertical.svg");
const UP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-up.svg");
const DOWN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const ROBOT: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/robot.svg");

/// What the replies' editor is open on, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyEditing {
    Closed,
    /// A new one: blank, or the ping's to start with.
    Adding(bool),
    Reply(AutoReplyId),
}

/// Reads the replies again, after a change.
async fn refresh(live: Live) {
    if let Ok(replies) = daemon::auto_replies().await {
        live.auto_replies.try_set(Some(replies));
    }
}

/// Changes one thing of a reply at once, from its row.
fn quick(live: Live, reply: &AutoReply, change: impl FnOnce(&mut AutoReplySettings)) {
    let id = reply.id;
    let mut settings = reply.settings.clone();
    change(&mut settings);
    spawn_local(async move {
        let _ = daemon::save_auto_reply(Some(id), &settings).await;
        refresh(live).await;
    });
}

/// Puts the reply at `from` where the one at `to` is, here at once and then
/// in the daemon.
fn reorder(live: Live, from: usize, to: usize) {
    let Some(mut replies) = live.auto_replies.get_untracked() else {
        return;
    };
    if from == to || from >= replies.len() || to >= replies.len() {
        return;
    }
    let moved = replies.remove(from);
    replies.insert(to, moved);
    let order: Vec<AutoReplyId> = replies.iter().map(|r| r.id).collect();
    live.auto_replies.set(Some(replies));
    spawn_local(async move {
        let _ = daemon::order_auto_replies(&order).await;
        refresh(live).await;
    });
}

/// A radio's name, by its id.
fn radio_name(live: Live, radio: RadioId) -> String {
    live.radios.with_untracked(|radios| {
        radios
            .iter()
            .flatten()
            .find(|r| r.id == radio)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| format!("#{radio}"))
    })
}

/// A node, by its name if the selected radio knows it.
fn node_label(live: Live, radio: RadioId, node: u32) -> String {
    if live.selected.get_untracked() == Some(radio) {
        crate::chat::node_name(live, node)
    } else {
        format!("!{node:08x}")
    }
}

/// Where a reply answers, in a few words: each radio's places, after its
/// name.
fn places_words(live: Live, places: &[AutoReplyPlace], directs: &str) -> String {
    let mut by_radio: Vec<(RadioId, Vec<String>)> = Vec::new();
    for place in places {
        let (radio, word) = match place {
            AutoReplyPlace::Directs { radio } => (*radio, directs.to_owned()),
            AutoReplyPlace::Node { radio, node } => (*radio, node_label(live, *radio, *node)),
            AutoReplyPlace::Channel { radio, name, .. } => {
                (*radio, format!("#{}", name.clone().unwrap_or_default()))
            }
        };
        match by_radio.iter_mut().find(|(r, _)| *r == radio) {
            Some((_, words)) => words.push(word),
            None => by_radio.push((radio, vec![word])),
        }
    }
    by_radio
        .into_iter()
        .map(|(radio, words)| format!("{} ({})", words.join(", "), radio_name(live, radio)))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[component]
pub fn AutoRepliesView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    spawn_local(refresh(live));
    let replies = move || {
        let replies = live.auto_replies.get().unwrap_or_default();
        let count = replies.len();
        replies
            .into_iter()
            .enumerate()
            .map(|(at, reply)| (at, count, reply))
            .collect::<Vec<_>>()
    };
    let empty = move || {
        live.auto_replies
            .with(|r| r.as_ref().is_some_and(Vec::is_empty))
    };
    let dragging = RwSignal::new(None::<usize>);

    view! {
        <section class="radios replies" aria-label=move || t_string!(i18n, replies_title)>
            <Show when=empty>
                <div class="card replies-empty">
                    <span class="icon" inner_html=ROBOT></span>
                    <h2>{t!(i18n, replies_empty_title)}</h2>
                    <p class="dim">{t!(i18n, replies_empty_text)}</p>
                    <div class="buttons">
                        <button
                            type="button"
                            on:click=move |_| live.reply_editing.set(ReplyEditing::Adding(false))
                        >
                            {t!(i18n, replies_start_blank)}
                        </button>
                        <button
                            type="button"
                            class="primary"
                            on:click=move |_| live.reply_editing.set(ReplyEditing::Adding(true))
                        >
                            {t!(i18n, replies_start_ping)}
                        </button>
                    </div>
                </div>
            </Show>
            <Show when=move || live.auto_replies.with(|r| r.as_ref().is_some_and(|r| !r.is_empty()))>
                <BusyLimit />
            </Show>
            <For
                each=replies
                key=|(at, count, reply)| (*at, *count, format!("{reply:?}"))
                let:((at, count, reply))
            >
                <ReplyRow reply at count dragging />
            </For>
            <Show when=move || !empty() && live.auto_replies.with(Option::is_some)>
                <p class="hint dim">
                    <span class="wide-only">{t!(i18n, replies_hint)}</span>
                    <span class="phone-only">{t!(i18n, replies_hint_phone)}</span>
                </p>
            </Show>
        </section>
        {move || match live.reply_editing.get() {
            ReplyEditing::Closed => ().into_any(),
            ReplyEditing::Adding(ping) => view! { <ReplyEditor id=None ping /> }.into_any(),
            ReplyEditing::Reply(id) => view! { <ReplyEditor id=Some(id) ping=false /> }.into_any(),
        }}
    }
}

/// How often how busy the radios hear their channels is read again.
const BUSY_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// The limit on a busy channel, for every reply: on or off, its threshold,
/// and how busy each radio hears its channel now.
#[component]
fn BusyLimit() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let limits = RwSignal::new(None::<AutoReplyLimits>);
    // The threshold last set, to turn it on again at.
    let last = RwSignal::new(AutoReplyLimits::default().busy_channel.unwrap_or(25));
    let busy = RwSignal::new(Vec::<ChannelBusy>::new());
    spawn_local(async move {
        if let Ok(read) = daemon::auto_reply_limits().await {
            if let Some(most) = read.busy_channel {
                last.try_set(most);
            }
            limits.try_set(Some(read));
        }
    });
    let read_busy = move || {
        spawn_local(async move {
            if let Ok(read) = daemon::channels_busy().await {
                busy.try_set(read);
            }
        });
    };
    read_busy();
    let every = set_interval_with_handle(read_busy, BUSY_EVERY).ok();
    on_cleanup(move || {
        if let Some(every) = every {
            every.clear();
        }
    });
    let save = move |changed: AutoReplyLimits| {
        limits.set(Some(changed));
        spawn_local(async move {
            if let Ok(saved) = daemon::save_auto_reply_limits(&changed).await {
                limits.try_set(Some(saved));
            }
        });
    };
    let on = move || limits.with(|l| l.is_some_and(|l| l.busy_channel.is_some()));
    let flip = move |_| {
        let busy_channel = if on() {
            None
        } else {
            Some(last.get_untracked())
        };
        save(AutoReplyLimits { busy_channel });
    };
    let set = move |ev: leptos::ev::Event| {
        let Ok(most) = event_target_value(&ev).trim().parse::<u8>() else {
            return;
        };
        let most = most.clamp(1, 100);
        last.set(most);
        save(AutoReplyLimits {
            busy_channel: Some(most),
        });
    };
    // Each radio's, by its name: paused if past the threshold.
    // Of the radios connected, and those that said lately: one that has not
    // and cannot now says nothing worth the room.
    let shown = move || {
        busy.get()
            .into_iter()
            .filter(|b| {
                b.percent.is_some()
                    || live.radios.with(|radios| {
                        radios
                            .iter()
                            .flatten()
                            .any(|r| r.id == b.radio && r.link == meshctl_api::LinkState::Ready)
                    })
            })
            .collect::<Vec<_>>()
    };
    let now = move || {
        let most = limits.with(|l| l.and_then(|l| l.busy_channel));
        shown()
            .into_iter()
            .map(|b| {
                let name = radio_name(live, b.radio);
                let paused = matches!((b.percent, most), (Some(p), Some(m)) if p > f32::from(m));
                let value = b.percent.map_or_else(
                    || t_string!(i18n, replies_busy_unknown).to_owned(),
                    |p| format!("{p:.0} %"),
                );
                view! {
                    <span class="busy-radio" class:paused=paused>
                        <b>{name}</b>
                        " "
                        {value}
                        {paused
                            .then(|| {
                                view! { <span class="tag">{t!(i18n, replies_busy_paused)}</span> }
                            })}
                    </span>
                }
            })
            .collect_view()
    };
    let off = move || !on();
    view! {
        <div class="preferences data busy-limit" class:gone=move || limits.with(Option::is_none)>
            <h2>{t!(i18n, replies_for_all)}</h2>
            <div class="pref-row">
                <div class="pref-what">
                    <span class="pref-title">{t!(i18n, replies_busy)}</span>
                    <span class="dim">{t!(i18n, replies_busy_hint)}</span>
                    <span class="busy-now dim" class:gone=move || shown().is_empty()>
                        {now}
                    </span>
                </div>
                <div class="pref-control retention">
                    <Switch
                        on=Signal::derive(on)
                        label=Signal::derive(move || t_string!(i18n, replies_busy).to_string())
                        flip
                    />
                    <span class:dim=off>{t!(i18n, replies_busy_above)}</span>
                    <span class="number-field">
                        <input
                            type="number"
                            min="1"
                            max="100"
                            inputmode="numeric"
                            disabled=off
                            prop:value=move || last.get().to_string()
                            on:change=set
                        />
                        <crate::stepper::Stepper />
                    </span>
                    <span class:dim=off>"%"</span>
                </div>
            </div>
            <h2>{t!(i18n, replies_each)}</h2>
        </div>
    }
}

#[component]
fn ReplyRow(
    reply: AutoReply,
    at: usize,
    count: usize,
    dragging: RwSignal<Option<usize>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let id = reply.id;
    let selected = move || live.reply_editing.get() == ReplyEditing::Reply(id);
    let over = RwSignal::new(false);
    let settings = reply.settings.clone();
    let toggle = {
        let reply = reply.clone();
        move |_| quick(live, &reply, |s| s.enabled = !s.enabled)
    };
    // What it waits for, and what it answers with.
    let what = {
        let mut answers = Vec::new();
        if settings.react {
            answers.push("1️⃣".to_owned());
        }
        if let Some(text) = &settings.reply {
            answers.push(format!("«{text}»"));
        }
        format!("{} → {}", settings.triggers.join(", "), answers.join(" "))
    };
    let places = settings.places.clone();
    let where_ = move || {
        if places.is_empty() {
            t_string!(i18n, replies_nowhere).to_owned()
        } else {
            let words = places_words(live, &places, t_string!(i18n, replies_directs));
            t_string!(i18n, replies_in).replace("{places}", &words)
        }
    };

    view! {
        <div
            class="radio-row reply-row"
            class:selected=selected
            class:hidden=!settings.enabled
            class:dragging=move || dragging.get() == Some(at)
            class:over=move || over.get()
            draggable="true"
            on:dragstart=move |ev| {
                if let Some(data) = ev.data_transfer() {
                    data.set_effect_allowed("move");
                    let _ = data.set_data("text/plain", &at.to_string());
                }
                dragging.set(Some(at));
            }
            on:dragover=move |ev| {
                ev.prevent_default();
                over.set(dragging.get_untracked().is_some_and(|from| from != at));
            }
            on:dragleave=move |_| over.set(false)
            on:drop=move |ev| {
                ev.prevent_default();
                over.set(false);
                if let Some(from) = dragging.get_untracked() {
                    reorder(live, from, at);
                }
                dragging.set(None);
            }
            on:dragend=move |_| dragging.set(None)
        >
            <span class="grip" title=move || t_string!(i18n, radio_drag) inner_html=GRIP></span>
            <span class="moves">
                <button
                    type="button"
                    disabled={at == 0}
                    title=move || t_string!(i18n, radio_up)
                    aria-label=move || t_string!(i18n, radio_up)
                    on:click=move |_| reorder(live, at, at.saturating_sub(1))
                    inner_html=UP
                ></button>
                <button
                    type="button"
                    disabled={at + 1 >= count}
                    title=move || t_string!(i18n, radio_down)
                    aria-label=move || t_string!(i18n, radio_down)
                    on:click=move |_| reorder(live, at, at + 1)
                    inner_html=DOWN
                ></button>
            </span>
            <button
                type="button"
                class="pick"
                on:click=move |_| live.reply_editing.set(ReplyEditing::Reply(id))
            >
                <span class="who">
                    <span class="name">
                        <b>{settings.name.clone()}</b>
                        {(!settings.enabled)
                            .then(|| view! { <span class="tag">{t!(i18n, replies_off)}</span> })}
                    </span>
                    <span class="mono dim">{what}</span>
                    <span class="dim places">{where_}</span>
                </span>
            </button>
            <Switch
                on=Signal::stored(settings.enabled)
                label=Signal::derive(move || t_string!(i18n, replies_enabled).to_string())
                flip=toggle
            />
        </div>
    }
}

/// A reply as the editor changes it.
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    name: String,
    enabled: bool,
    /// As written: separated by commas.
    triggers: String,
    matching: AutoReplyMatch,
    react: bool,
    /// Whether it answers with a text, and whether with another if direct:
    /// each text kept while off, to come back to.
    texts: bool,
    reply: String,
    direct: bool,
    direct_reply: String,
    places: Vec<AutoReplyPlace>,
}

impl Draft {
    fn new(settings: &AutoReplySettings) -> Self {
        Self {
            name: settings.name.clone(),
            enabled: settings.enabled,
            triggers: settings.triggers.join(", "),
            matching: settings.matching,
            react: settings.react,
            texts: settings.reply.is_some(),
            reply: settings.reply.clone().unwrap_or_default(),
            direct: settings.direct_reply.is_some(),
            direct_reply: settings.direct_reply.clone().unwrap_or_default(),
            places: settings.places.clone(),
        }
    }

    fn settings(&self) -> AutoReplySettings {
        let text = |on: bool, text: &str| (on && !text.trim().is_empty()).then(|| text.to_owned());
        AutoReplySettings {
            name: self.name.trim().to_owned(),
            enabled: self.enabled,
            triggers: {
                // Each once, however it was written again: as the daemon
                // keeps them.
                let mut triggers: Vec<String> = Vec::new();
                for trigger in self.triggers.split(',').map(str::trim) {
                    let lower = trigger.to_lowercase();
                    if !trigger.is_empty() && !triggers.iter().any(|t| t.to_lowercase() == lower) {
                        triggers.push(trigger.to_owned());
                    }
                }
                triggers
            },
            matching: self.matching,
            react: self.react,
            reply: text(self.texts, &self.reply),
            direct_reply: text(self.texts && self.direct, &self.direct_reply),
            places: self.places.clone(),
        }
    }

    fn flip(&mut self, place: AutoReplyPlace) {
        if let Some(at) = self.places.iter().position(|p| *p == place) {
            self.places.remove(at);
        } else {
            self.places.push(place);
        }
    }
}

/// A channel place as the editor tells it apart: by its radio and slot, or,
/// in none, by its name.
fn channel_place(radio: RadioId, channel: &Channel) -> AutoReplyPlace {
    AutoReplyPlace::Channel {
        radio,
        channel: Some(channel.index),
        name: None,
    }
}

/// A channel's name, as read with its preset's for one with none; by its
/// slot while none is known.
fn channel_name(channel: &Channel) -> String {
    if channel.name.is_empty() {
        let i18n = use_i18n();
        untrack(|| format!("{} {}", t_string!(i18n, channel_numbered), channel.index))
    } else {
        channel.name.clone()
    }
}

/// Places alike but for the name told with a channel in a slot, which the
/// editor does not keep up to date.
fn same_place(a: &AutoReplyPlace, b: &AutoReplyPlace) -> bool {
    match (a, b) {
        (
            AutoReplyPlace::Channel {
                radio: ra,
                channel: Some(ca),
                ..
            },
            AutoReplyPlace::Channel {
                radio: rb,
                channel: Some(cb),
                ..
            },
        ) => ra == rb && ca == cb,
        _ => a == b,
    }
}

/// What the sample message tells, to show a text as it will be.
fn sample(direct: bool, channel: Option<String>) -> Told {
    let now = js_sys::Date::new_0();
    Told {
        long_name: Some("Ana".into()),
        short_name: Some("ANA".into()),
        node_id: Some("!a1b2c3d4".into()),
        hops: Some(if direct { 0 } else { 3 }),
        snr: Some(if direct { 7.5 } else { -4.25 }),
        rssi: Some(if direct { -95 } else { -112 }),
        channel: Some(channel.unwrap_or_else(|| "LongFast".into())),
        date: format!(
            "{:04}-{:02}-{:02}",
            now.get_full_year(),
            now.get_month() + 1,
            now.get_date()
        ),
        time: format!("{:02}:{:02}", now.get_hours(), now.get_minutes()),
    }
}

/// Changes a reply, or adds one when given none: blank, or the ping's.
#[component]
fn ReplyEditor(id: Option<AutoReplyId>, ping: bool) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let start = id
        .and_then(|id| {
            live.auto_replies.with_untracked(|replies| {
                replies
                    .iter()
                    .flatten()
                    .find(|r| r.id == id)
                    .map(|r| Draft::new(&r.settings))
            })
        })
        // The texts of the language it is in as it opens, not followed.
        .unwrap_or_else(|| {
            untrack(|| {
                let blank = AutoReplySettings {
                    name: String::new(),
                    enabled: true,
                    triggers: Vec::new(),
                    matching: AutoReplyMatch::Whole,
                    react: false,
                    reply: Some(String::new()),
                    direct_reply: None,
                    places: Vec::new(),
                };
                if ping {
                    Draft::new(&AutoReplySettings {
                        name: "Ping".into(),
                        triggers: t_string!(i18n, reply_ping_triggers)
                            .split(',')
                            .map(|t| t.trim().to_owned())
                            .collect(),
                        react: true,
                        reply: Some(t_string!(i18n, reply_ping_text).to_owned()),
                        direct_reply: Some(t_string!(i18n, reply_ping_direct).to_owned()),
                        ..blank
                    })
                } else {
                    Draft::new(&blank)
                }
            })
        });
    let draft = RwSignal::new(start);
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    let close = move || live.reply_editing.set(ReplyEditing::Closed);
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            close();
        }
    });
    on_cleanup(move || escape.remove());
    let name_input = NodeRef::<leptos::html::Input>::new();
    if id.is_none() {
        Effect::new(move |_| {
            if let Some(input) = name_input.get() {
                let _ = input.focus();
            }
        });
    }

    // Each radio's channels, as it has them now; none for one that cannot
    // say.
    let channels = RwSignal::new(HashMap::<RadioId, Option<Vec<Channel>>>::new());
    let radios: Vec<(RadioId, String, RadioKind)> = live.radios.with_untracked(|radios| {
        radios
            .iter()
            .flatten()
            .map(|r| (r.id, r.name.clone(), r.kind))
            .collect()
    });
    for (radio, _, kind) in &radios {
        let (radio, kind) = (*radio, *kind);
        spawn_local(async move {
            let mut read = daemon::channels(radio).await.ok().map(|all| {
                all.into_iter()
                    .filter(|c| c.role != "DISABLED")
                    .collect::<Vec<_>>()
            });
            // A Meshtastic channel with no name goes by its radio's modem
            // preset, as the radio names it; Custom with none.
            if kind == RadioKind::Meshtastic
                && let Some(list) = read.as_mut()
                && list.iter().any(|c| c.name.is_empty())
                && let Ok(status) = daemon::status(radio).await
                && let Some(local) = status.local
            {
                let preset = local
                    .modem_preset
                    .as_deref()
                    .map_or_else(|| "Custom".to_owned(), crate::chat::preset_name);
                for channel in list.iter_mut().filter(|c| c.name.is_empty()) {
                    channel.name.clone_from(&preset);
                }
            }
            channels.try_update(|channels| {
                channels.insert(radio, read);
            });
        });
    }

    // The texts' fields, and the last one written in, for the variables to
    // go into.
    let reply_field = NodeRef::<leptos::html::Textarea>::new();
    let direct_field = NodeRef::<leptos::html::Textarea>::new();
    let last_direct = StoredValue::new(false);
    let insert = move |variable: &str| {
        let direct = last_direct.get_value() && draft.with_untracked(|d| d.direct);
        let field = if direct { direct_field } else { reply_field };
        let Some(field) = field.get_untracked() else {
            return;
        };
        let value = field.value();
        let units: Vec<u16> = value.encode_utf16().collect();
        let at = field
            .selection_start()
            .ok()
            .flatten()
            .map_or(units.len(), |at| (at as usize).min(units.len()));
        let end = field
            .selection_end()
            .ok()
            .flatten()
            .map_or(at, |end| (end as usize).clamp(at, units.len()));
        let token = format!("{{{variable}}}");
        let new = format!(
            "{}{token}{}",
            String::from_utf16_lossy(&units[..at]),
            String::from_utf16_lossy(&units[end..])
        );
        field.set_value(&new);
        let caret = (at + token.encode_utf16().count()) as u32;
        let _ = field.set_selection_range(caret, caret);
        let _ = field.focus();
        draft.update(|d| {
            if direct {
                d.direct_reply = new;
            } else {
                d.reply = new;
            }
        });
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let settings = draft.with_untracked(Draft::settings);
        busy.set(true);
        spawn_local(async move {
            match daemon::save_auto_reply(id, &settings).await {
                Ok(_) => {
                    refresh(live).await;
                    close();
                    return;
                }
                Err(failure) => {
                    error.try_set(Some(failure));
                }
            }
            busy.try_set(false);
        });
    };
    let remove = move |_| {
        let Some(id) = id else {
            return;
        };
        let sure = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(untrack(|| t_string!(i18n, reply_remove_confirm)))
                    .ok()
            })
            .unwrap_or(false);
        if sure {
            spawn_local(async move {
                if daemon::remove_auto_reply(id).await.is_ok() {
                    close();
                    refresh(live).await;
                }
            });
        }
    };
    let message = move || {
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::InvalidAutoReply) => {
                t!(i18n, error_invalid_auto_reply).into_any()
            }
            _ => t!(i18n, error_failed).into_any(),
        })
    };
    let title = move || match id {
        Some(_) => t!(i18n, reply_edit_title).into_any(),
        None => t!(i18n, reply_add_title).into_any(),
    };

    // A text as it will be, and how much of a message it takes.
    let preview = move |direct: bool| {
        let text = draft.with(|d| {
            if direct {
                d.direct_reply.clone()
            } else {
                d.reply.clone()
            }
        });
        // The channel of the first place on one, as the sample's: by the
        // name it was told with, or, just picked, as its radio has it.
        let channel = draft.with(|d| {
            d.places.iter().find_map(|p| match p {
                AutoReplyPlace::Channel {
                    name: Some(name), ..
                } => Some(name.clone()),
                AutoReplyPlace::Channel {
                    radio,
                    channel: Some(slot),
                    name: None,
                } => channels.with(|c| {
                    c.get(radio)?
                        .as_ref()?
                        .iter()
                        .find(|c| c.index == *slot)
                        .map(channel_name)
                }),
                _ => None,
            })
        });
        let shown = render(&text, &sample(direct, channel));
        let bytes = shown.len();
        let blank = shown.is_empty();
        let over = bytes > MAX_TEXT_BYTES;
        view! {
            <span class="reply-preview" class:gone=blank>
                <span class="dim">{t!(i18n, reply_preview)}</span>
                <span class="bubble">{shown}</span>
                <span class="count mono" class:over=over>
                    {format!("{bytes}/{MAX_TEXT_BYTES}")}
                </span>
            </span>
            <span class="error" class:gone=!over>{t!(i18n, reply_over)}</span>
        }
    };
    let matchings = [AutoReplyMatch::Whole, AutoReplyMatch::Word].map(|matching| {
        let on = move || draft.with(|d| d.matching == matching);
        view! {
            <button
                type="button"
                role="radio"
                class:on=on
                aria-checked=move || on().to_string()
                on:click=move |_| draft.update(|d| d.matching = matching)
            >
                {move || match matching {
                    AutoReplyMatch::Whole => t!(i18n, reply_match_whole).into_any(),
                    AutoReplyMatch::Word => t!(i18n, reply_match_word).into_any(),
                }}
            </button>
        }
    });
    let variables = VARIABLES.map(|variable| {
        let label = move || variable_hint(i18n, variable);
        view! {
            <button
                type="button"
                class="variable mono"
                title=label
                on:mousedown=|ev| ev.prevent_default()
                on:click=move |_| insert(variable)
            >
                {format!("{{{variable}}}")}
            </button>
        }
    });
    let any_meshcore = radios
        .iter()
        .any(|(_, _, kind)| *kind == RadioKind::Meshcore);
    let wheres = radios
        .into_iter()
        .map(|(radio, name, _)| view! { <RadioPlaces radio name draft channels /> })
        .collect_view();

    let form = view! {
        <form
            class="card radio-editor reply-editor modal"
            role="dialog"
            aria-modal="true"
            on:submit=submit
        >
            <header class="editor-head">
                <span class="reply-face" inner_html=ROBOT></span>
                <span class="who">
                    <h2>{title}</h2>
                    <span class="dim">{move || draft.with(|d| d.name.clone())}</span>
                </span>
                <button
                    type="button"
                    class="close"
                    title=move || t_string!(i18n, radio_cancel)
                    aria-label=move || t_string!(i18n, radio_cancel)
                    on:click=move |_| close()
                    inner_html=CLOSE
                ></button>
            </header>
            <div class="modal-body fields">
                <label>
                    <span class="dim">{t!(i18n, reply_name)}</span>
                    <input
                        required
                        maxlength="64"
                        node_ref=name_input
                        placeholder=move || t_string!(i18n, reply_name_hint)
                        prop:value=move || draft.with(|d| d.name.clone())
                        on:input=move |ev| {
                            let name = event_target_value(&ev);
                            draft.update(|d| d.name = name);
                        }
                    />
                </label>

                <div class="field">
                    <span class="dim">{t!(i18n, reply_triggers)}</span>
                    <input
                        required
                        placeholder="ping, test"
                        prop:value=move || draft.with(|d| d.triggers.clone())
                        on:input=move |ev| {
                            let triggers = event_target_value(&ev);
                            draft.update(|d| d.triggers = triggers);
                        }
                    />
                    <div
                        class="segmented"
                        role="radiogroup"
                        style=move || {
                            let at = draft.with(|d| usize::from(d.matching == AutoReplyMatch::Word));
                            format!("--n: 2; --at: {at}")
                        }
                    >
                        {matchings.into_iter().collect_view()}
                    </div>
                    <span class="hint dim">
                        {move || match draft.with(|d| d.matching) {
                            AutoReplyMatch::Whole => t!(i18n, reply_match_whole_hint).into_any(),
                            AutoReplyMatch::Word => t!(i18n, reply_match_word_hint).into_any(),
                        }}
                    </span>
                </div>

                <div class="field answers">
                    <span class="dim">{t!(i18n, reply_answer)}</span>
                    <div class="toggle">
                        <Switch
                            on=Signal::derive(move || draft.with(|d| d.react))
                            label=Signal::derive(move || t_string!(i18n, reply_react).to_string())
                            flip=move |_| draft.update(|d| d.react = !d.react)
                        />
                        <span>
                            {t!(i18n, reply_react)}
                            <span class="dim">
                                {move || format!(
                                    "{} {} · {} {} · {} {} · {}",
                                    hops_emoji(0),
                                    t_string!(i18n, reply_react_direct),
                                    hops_emoji(1),
                                    hops_emoji(2),
                                    hops_emoji(3),
                                    hops_emoji(10),
                                    t_string!(i18n, reply_react_more),
                                )}
                            </span>
                        </span>
                    </div>
                    <div class="toggle">
                        <Switch
                            on=Signal::derive(move || draft.with(|d| d.texts))
                            label=Signal::derive(move || t_string!(i18n, reply_text).to_string())
                            flip=move |_| draft.update(|d| d.texts = !d.texts)
                        />
                        <span>{t!(i18n, reply_text)}</span>
                    </div>
                    <div class="reply-texts" class:gone=move || !draft.with(|d| d.texts)>
                        <textarea
                            rows="2"
                            node_ref=reply_field
                            placeholder=move || t_string!(i18n, reply_ping_text)
                            prop:value=move || draft.with(|d| d.reply.clone())
                            on:focus=move |_| last_direct.set_value(false)
                            on:input=move |ev| {
                                let text = event_target_value(&ev);
                                draft.update(|d| d.reply = text);
                            }
                        ></textarea>
                        {move || preview(false)}
                        <div class="toggle">
                            <Switch
                                on=Signal::derive(move || draft.with(|d| d.direct))
                                label=Signal::derive(move || {
                                    t_string!(i18n, reply_direct).to_string()
                                })
                                flip=move |_| draft.update(|d| d.direct = !d.direct)
                            />
                            <span>
                                {t!(i18n, reply_direct)}
                                <span class="dim">{t!(i18n, reply_direct_hint)}</span>
                            </span>
                        </div>
                        <div class="reply-texts" class:gone=move || !draft.with(|d| d.direct)>
                            <textarea
                                rows="2"
                                node_ref=direct_field
                                placeholder=move || t_string!(i18n, reply_ping_direct)
                                prop:value=move || draft.with(|d| d.direct_reply.clone())
                                on:focus=move |_| last_direct.set_value(true)
                                on:input=move |ev| {
                                    let text = event_target_value(&ev);
                                    draft.update(|d| d.direct_reply = text);
                                }
                            ></textarea>
                            {move || preview(true)}
                        </div>
                        <div class="variables">
                            <span class="dim">{t!(i18n, reply_variables)}</span>
                            <div class="chips">{variables.into_iter().collect_view()}</div>
                            <span class="hint dim">
                                {t!(i18n, reply_variables_hint)}
                                {any_meshcore
                                    .then(|| {
                                        view! { " " {t!(i18n, reply_meshcore_note)} }
                                    })}
                            </span>
                        </div>
                    </div>
                </div>

                <div class="field">
                    <span class="dim">{t!(i18n, reply_where)}</span>
                    <div class="reply-places">{wheres}</div>
                    <span class="hint dim">{t!(i18n, reply_where_hint)}</span>
                </div>

                <div class="toggle">
                    <Switch
                        on=Signal::derive(move || draft.with(|d| d.enabled))
                        label=Signal::derive(move || t_string!(i18n, replies_enabled).to_string())
                        flip=move |_| draft.update(|d| d.enabled = !d.enabled)
                    />
                    <span>
                        {t!(i18n, replies_enabled)}
                        <span class="dim">{t!(i18n, reply_enabled_hint)}</span>
                    </span>
                </div>
            </div>

            <p class="error">{message}</p>
            <div class="buttons">
                {id
                    .is_some()
                    .then(|| {
                        view! {
                            <button type="button" class="danger" on:click=remove>
                                {t!(i18n, radio_remove)}
                            </button>
                        }
                    })}
                <button type="button" on:click=move |_| close()>
                    {t!(i18n, radio_cancel)}
                </button>
                <button type="submit" class="primary" disabled=move || busy.get()>
                    {move || match id {
                        Some(_) => t!(i18n, radio_save).into_any(),
                        None => t!(i18n, radio_add).into_any(),
                    }}
                </button>
            </div>
        </form>
    };
    view! { <div class="modal-back">{form}</div> }
}

/// What a variable says, in a few words.
fn variable_hint(i18n: leptos_i18n::I18nContext<crate::i18n::Locale>, variable: &str) -> String {
    match variable {
        "LONG_NAME" => t_string!(i18n, reply_var_long_name),
        "SHORT_NAME" => t_string!(i18n, reply_var_short_name),
        "NODE_ID" => t_string!(i18n, reply_var_node_id),
        "HOPS" => t_string!(i18n, reply_var_hops),
        "KANGAROO_HOPS" => t_string!(i18n, reply_var_kangaroo_hops),
        "SNR" => t_string!(i18n, reply_var_snr),
        "RSSI" => t_string!(i18n, reply_var_rssi),
        "CHANNEL" => t_string!(i18n, reply_var_channel),
        "DATE" => t_string!(i18n, reply_var_date),
        _ => t_string!(i18n, reply_var_time),
    }
    .to_owned()
}

/// Where a reply answers on one radio: its direct conversations, each of its
/// channels, and what else it was given there.
#[component]
fn RadioPlaces(
    radio: RadioId,
    name: String,
    draft: RwSignal<Draft>,
    channels: RwSignal<HashMap<RadioId, Option<Vec<Channel>>>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let check = move |place: AutoReplyPlace, label: Signal<String>, note: Option<String>| {
        let on = {
            let place = place.clone();
            move || draft.with(|d| d.places.iter().any(|p| same_place(p, &place)))
        };
        view! {
            <label class="check">
                <input
                    type="checkbox"
                    prop:checked=on
                    on:change=move |_| {
                        let place = place.clone();
                        draft.update(|d| {
                            match d.places.iter().position(|p| same_place(p, &place)) {
                                Some(at) => {
                                    d.places.remove(at);
                                }
                                None => d.flip(place),
                            }
                        });
                    }
                />
                <span>{label}</span>
                {note.map(|note| view! { <span class="dim">{note}</span> })}
            </label>
        }
    };
    let directs = check(
        AutoReplyPlace::Directs { radio },
        Signal::derive(move || t_string!(i18n, reply_directs).to_owned()),
        None,
    );
    // Its channels as it has them, and those it was given that it has not
    // now; then the nodes it was given.
    let rest = move || {
        let known = channels.with(|c| c.get(&radio).cloned());
        let mut views = Vec::new();
        match &known {
            Some(Some(list)) if !list.is_empty() => {
                for channel in list {
                    let place = channel_place(radio, channel);
                    let name = channel_name(channel);
                    views.push(check(place, Signal::stored(format!("#{name}")), None).into_any());
                }
            }
            Some(_) => {
                views.push(
                    view! { <span class="hint dim">{t!(i18n, reply_no_channels)}</span> }
                        .into_any(),
                );
            }
            None => {}
        }
        let given: Vec<AutoReplyPlace> = draft.with_untracked(|d| d.places.clone());
        for place in given {
            match &place {
                AutoReplyPlace::Channel {
                    radio: r,
                    channel,
                    name,
                } if *r == radio => {
                    let listed = matches!(&known, Some(Some(list))
                        if channel.is_some_and(|c| list.iter().any(|l| l.index == c)));
                    if !listed {
                        let label = format!("#{}", name.clone().unwrap_or_default());
                        let note = channel
                            .is_none()
                            .then(|| t_string!(i18n, reply_away).to_owned());
                        views.push(check(place.clone(), Signal::stored(label), note).into_any());
                    }
                }
                AutoReplyPlace::Node { radio: r, node } if *r == radio => {
                    let label = t_string!(i18n, reply_with_node)
                        .replace("{node}", &node_label(live, radio, *node));
                    views.push(check(place.clone(), Signal::stored(label), None).into_any());
                }
                _ => {}
            }
        }
        views.into_iter().collect_view()
    };
    view! {
        <div class="radio-places">
            <b>{name}</b>
            {directs}
            {rest}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ping() -> AutoReplySettings {
        AutoReplySettings {
            name: "Ping".into(),
            enabled: true,
            triggers: vec!["ping".into(), "test".into()],
            matching: AutoReplyMatch::Whole,
            react: true,
            reply: Some("🤖 Pong, {HOPS} 🦘".into()),
            direct_reply: Some("🤖 Pong 🎯".into()),
            places: vec![AutoReplyPlace::Directs { radio: 1 }],
        }
    }

    #[test]
    fn a_draft_gives_back_what_it_was_given() {
        assert_eq!(Draft::new(&ping()).settings(), ping());
        let mut only_react = ping();
        only_react.reply = None;
        only_react.direct_reply = None;
        assert_eq!(Draft::new(&only_react).settings(), only_react);
    }

    #[test]
    fn a_draft_keeps_a_text_switched_off_but_sends_none() {
        let mut draft = Draft::new(&ping());
        draft.texts = false;
        let settings = draft.settings();
        assert_eq!((settings.reply, settings.direct_reply), (None, None));
        draft.texts = true;
        assert_eq!(draft.settings(), ping());
        draft.direct = false;
        assert_eq!(draft.settings().direct_reply, None);
        // Triggers written loosely, and again.
        draft.triggers = " ping ,, test, Ping, test".into();
        assert_eq!(draft.settings().triggers, ["ping", "test"]);
    }
}
