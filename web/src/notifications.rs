// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! meshctl's notifications, as a page of its settings: whether it notifies
//! at all, and who shows them; with what messages say or not, as a sample
//! shows; and where, each radio's conversations as never, everything, or,
//! on a channel, only when it mentions the radio.
//!
//! Each change is kept as it is made.

use crate::app::Live;
use crate::auto_replies::{channel_name, channel_place, node_label, radio_channels, same_place};
use crate::badge::{self, Face};
use crate::daemon;
use crate::i18n::{t, t_string, use_i18n};
use crate::notify::{self, Permission};
use crate::preferences::{Choice, pills, slide};
use crate::radios::Switch;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{AutoReplyPlace, Badge, Channel, NotificationPlace, Notifications, RadioId};
use std::collections::HashMap;

const BELL: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/bell-ringing.svg");
const MESSAGES: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/messages.svg");
const HASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/hash.svg");
const USER: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/user.svg");
const CHECK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/check.svg");
// A radio's none, some and all, on a phone: a circle crossed out, dashed,
// and full.
const NONE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/circle-off.svg");
const SOME: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/circle-dashed.svg");
const ALL: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/filled/circle.svg");
const CHEVRON: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
const WARNING: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/alert-triangle.svg");

/// Keeps the notifications as they are now, and reads them back as kept:
/// as they were, if they could not be.
fn keep(settings: RwSignal<Option<Notifications>>, changed: Notifications) {
    settings.set(Some(changed.clone()));
    spawn_local(async move {
        let kept = match daemon::save_notifications(&changed).await {
            Ok(kept) => Some(kept),
            Err(_) => daemon::notifications().await.ok(),
        };
        if let Some(kept) = kept {
            settings.try_set(Some(kept));
        }
    });
}

#[component]
pub fn NotificationsView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let settings = RwSignal::new(None::<Notifications>);
    spawn_local(async move {
        if let Ok(read) = daemon::notifications().await {
            settings.try_set(Some(read));
        }
    });
    let change = move |edit: &dyn Fn(&mut Notifications)| {
        let Some(mut changed) = settings.get_untracked() else {
            return;
        };
        edit(&mut changed);
        keep(settings, changed);
    };
    let enabled = move || settings.with(|s| s.as_ref().is_some_and(|s| s.enabled));
    let show_text = move || settings.with(|s| s.as_ref().is_some_and(|s| s.show_text));

    // What this browser lets, out of an app: asked as notifications are
    // switched on, or on its button.
    let permission = RwSignal::new(notify::permission());
    let ask = move || {
        spawn_local(async move {
            let _ = permission.try_set(notify::ask().await);
        });
    };
    let flip_enabled = move |_| {
        let on = !enabled();
        change(&|n| n.enabled = on);
        if on && !notify::in_app() {
            ask();
        }
    };
    // Who shows them, and whether they can: a line under the switch.
    let status = move || {
        if !enabled() {
            return ().into_any();
        }
        if notify::in_app() {
            return view! {
                <p class="notify-status">
                    <span class="icon" inner_html=CHECK></span>
                    <span>{t!(i18n, notifications_status_app)}</span>
                </p>
            }
            .into_any();
        }
        match permission.get() {
            Permission::Granted => view! {
                <p class="notify-status ok">
                    <span class="icon" inner_html=CHECK></span>
                    <span>{t!(i18n, notifications_browser_granted)}</span>
                </p>
            }
            .into_any(),
            Permission::Ask => view! {
                <p class="notify-status ask">
                    <span class="icon" inner_html=WARNING></span>
                    <span>{t!(i18n, notifications_browser_ask)}</span>
                    <button type="button" class="primary" on:click=move |_| ask()>
                        {t!(i18n, notifications_browser_allow)}
                    </button>
                </p>
            }
            .into_any(),
            Permission::Denied => view! {
                <p class="notify-status bad">
                    <span class="icon" inner_html=WARNING></span>
                    <span>{t!(i18n, notifications_browser_denied)}</span>
                </p>
            }
            .into_any(),
            Permission::Unsupported => view! {
                <p class="notify-status bad">
                    <span class="icon" inner_html=WARNING></span>
                    <span>{t!(i18n, notifications_browser_unsupported)}</span>
                </p>
            }
            .into_any(),
        }
    };

    // A notification as it will look, with its text or without: the one
    // picked, as notifications are to be.
    let sample = move |with_text: bool| {
        let on = move || show_text() == with_text;
        let caption = move || {
            if with_text {
                t_string!(i18n, notifications_with_text)
            } else {
                t_string!(i18n, notifications_without_text)
            }
        };
        view! {
            <button
                type="button"
                role="radio"
                class="notify-choice"
                class:on=on
                aria-checked=move || on().to_string()
                aria-label=caption
                on:click=move |_| {
                    if !on() {
                        change(&|n| n.show_text = with_text);
                    }
                }
            >
                <span class="notify-sample" aria-hidden="true">
                    <img class="sample-app" src="/favicon.svg" alt="" />
                    <span class="sample-body">
                        <span class="sample-top">
                            <span>"meshctl"</span>
                            <span class="dim">{t!(i18n, notifications_sample_when)}</span>
                        </span>
                        <b>{t!(i18n, notifications_sample_title)}</b>
                        {if with_text {
                            view! { <span>{t!(i18n, notifications_sample_text)}</span> }
                                .into_any()
                        } else {
                            view! {
                                <span class="dim hidden-text">
                                    {t!(i18n, notifications_sample_hidden)}
                                </span>
                            }
                                .into_any()
                        }}
                    </span>
                    <span class="tick" inner_html=CHECK></span>
                </span>
                <span class="choice-caption">{caption}</span>
            </button>
        }
    };

    let (radios, channels) = radio_channels(live);
    let empty = radios.is_empty();
    let wheres = radios
        .into_iter()
        .map(|(radio, name, _)| view! { <RadioPlaces radio name settings channels /> })
        .collect_view();

    view! {
        <section
            class="notifications"
            class:off=move || !enabled()
            class:gone=move || settings.with(Option::is_none)
        >
            <div class="card notify-hero">
                <div class="notify-head">
                    <span class="reply-face" inner_html=BELL></span>
                    <div class="notify-what">
                        <h2>{t!(i18n, notifications_on)}</h2>
                        <span class="dim">{t!(i18n, notifications_on_hint)}</span>
                    </div>
                    <Switch
                        on=Signal::derive(enabled)
                        label=Signal::derive(move || t_string!(i18n, notifications_on).to_string())
                        flip=flip_enabled
                    />
                </div>
                {status}
            </div>

            <div class="card notify-look">
                <div class="notify-what">
                    <h3>{t!(i18n, notifications_look)}</h3>
                    <span class="dim">{t!(i18n, notifications_look_hint)}</span>
                </div>
                <div
                    class="notify-samples"
                    role="radiogroup"
                    aria-label=move || t_string!(i18n, notifications_look)
                >
                    {sample(true)}
                    {sample(false)}
                </div>
            </div>

            <header class="notify-where">
                <h2>{t!(i18n, notifications_where)}</h2>
                <span class="dim">{t!(i18n, notifications_where_hint)}</span>
            </header>
            <div class="notify-places">
                {wheres}
                {empty.then(|| view! { <p class="hint dim">{t!(i18n, notifications_no_radios)}</p> })}
            </div>
        </section>
    }
}

/// How a conversation is notified of: never, only mentions, everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Never,
    Mentions,
    All,
}

/// Where messages are notified of on one radio: its direct conversations,
/// each of its channels, and what else it was given there, each a row with
/// how much of it is.
#[component]
fn RadioPlaces(
    radio: RadioId,
    name: String,
    settings: RwSignal<Option<Notifications>>,
    channels: RwSignal<HashMap<RadioId, Option<Vec<Channel>>>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let (badge, short) = live.radios.with_untracked(|radios| {
        radios
            .iter()
            .flatten()
            .find(|r| r.id == radio)
            .map(|r| (r.badge.clone(), r.short_name.clone()))
            .unwrap_or_else(|| (Badge::default(), None))
    });
    let level_of = move |place: &AutoReplyPlace| {
        settings.with(|s| {
            match s
                .as_ref()
                .and_then(|s| s.places.iter().find(|p| same_place(&p.place, place)))
            {
                None => Level::Never,
                Some(p) if p.mentions => Level::Mentions,
                Some(_) => Level::All,
            }
        })
    };
    let set = move |place: AutoReplyPlace, level: Level| {
        let Some(mut changed) = settings.get_untracked() else {
            return;
        };
        let at = changed
            .places
            .iter()
            .position(|p| same_place(&p.place, &place));
        match (at, level) {
            (Some(at), Level::Never) => {
                changed.places.remove(at);
            }
            (Some(at), level) => changed.places[at].mentions = level == Level::Mentions,
            (None, Level::Never) => return,
            (None, level) => changed.places.push(NotificationPlace {
                place,
                mentions: level == Level::Mentions,
            }),
        }
        keep(settings, changed);
    };
    let row = move |place: AutoReplyPlace,
                    icon: &'static str,
                    label: String,
                    note: Option<String>,
                    channel: bool| {
        let levels: &[Level] = if channel {
            &[Level::Never, Level::Mentions, Level::All]
        } else {
            &[Level::Never, Level::All]
        };
        let choices: Vec<Choice> = levels
            .iter()
            .map(|&level| {
                let on = {
                    let place = place.clone();
                    Signal::derive(move || level_of(&place) == level)
                };
                let pick = {
                    let place = place.clone();
                    Box::new(move || set(place.clone(), level)) as Box<dyn Fn() + Send + Sync>
                };
                let word = Signal::derive(move || {
                    match level {
                        Level::Never => t_string!(i18n, notifications_never),
                        Level::Mentions => t_string!(i18n, notifications_mentions),
                        Level::All => t_string!(i18n, notifications_all),
                    }
                    .to_owned()
                });
                (on, pick, word)
            })
            .collect();
        let quiet = {
            let place = place.clone();
            move || level_of(&place) == Level::Never
        };
        view! {
            <div class="notify-row" class:quiet=quiet>
                <span class="row-icon" inner_html=icon></span>
                <span class="row-name">
                    <span>{label}</span>
                    {note.map(|note| view! { <span class="dim">{note}</span> })}
                </span>
                {pills(choices)}
            </div>
        }
    };
    let directs = move || {
        row(
            AutoReplyPlace::Directs { radio },
            MESSAGES,
            t_string!(i18n, notifications_directs).to_owned(),
            None,
            false,
        )
    };
    let loaded = Memo::new(move |_| settings.with(Option::is_some));
    // Its channels as it has them, and those it was given that it has not
    // now; then the nodes it was given.
    let rest = move || {
        let known = channels.with(|c| c.get(&radio).cloned());
        let mut views = Vec::new();
        match &known {
            Some(Some(list)) if !list.is_empty() => {
                for channel in list {
                    let place = channel_place(radio, channel);
                    views.push(row(place, HASH, channel_name(channel), None, true).into_any());
                }
            }
            Some(_) => {
                views.push(
                    view! { <p class="hint dim">{t!(i18n, reply_no_channels)}</p> }.into_any(),
                );
            }
            None => {}
        }
        // Again once read, not as each is picked: a place taken out stays
        // where it was, to be put back, and each pill slides as it is.
        loaded.get();
        let others: Vec<NotificationPlace> =
            settings.with_untracked(|s| s.as_ref().map(|s| s.places.clone()).unwrap_or_default());
        for given in others {
            match &given.place {
                AutoReplyPlace::Channel {
                    radio: r,
                    channel,
                    name,
                } if *r == radio => {
                    let listed = matches!(&known, Some(Some(list))
                        if channel.is_some_and(|c| list.iter().any(|l| l.index == c)));
                    if !listed {
                        let label = name.clone().unwrap_or_default();
                        let note = channel
                            .is_none()
                            .then(|| t_string!(i18n, reply_away).to_owned());
                        views.push(row(given.place.clone(), HASH, label, note, true).into_any());
                    }
                }
                AutoReplyPlace::Node { radio: r, node } if *r == radio => {
                    let label = node_label(live, radio, *node);
                    views.push(row(given.place.clone(), USER, label, None, false).into_any());
                }
                _ => {}
            }
        }
        views.into_iter().collect_view()
    };
    // How many of its conversations are notified of, under its name.
    let summary = move || {
        let count = settings.with(|s| {
            s.as_ref().map_or(0, |s| {
                s.places
                    .iter()
                    .filter(|p| place_radio(&p.place) == radio)
                    .count()
            })
        });
        match count {
            0 => t_string!(i18n, notifications_radio_none).to_owned(),
            1 => t_string!(i18n, notifications_radio_single).to_owned(),
            n => t_string!(i18n, notifications_radio_count).replace("{count}", &n.to_string()),
        }
    };
    // How much of it is notified of: none, some, or all, every channel
    // it has and its direct conversations, everything said. None or all is
    // picked at once; some, by opening it.
    let level = Memo::new(move |_| {
        settings.with(|s| {
            let Some(s) = s else { return 0 };
            let mine: Vec<&NotificationPlace> = s
                .places
                .iter()
                .filter(|p| place_radio(&p.place) == radio)
                .collect();
            if mine.is_empty() {
                return 0;
            }
            let every = mine.iter().all(|p| !p.mentions)
                && mine
                    .iter()
                    .any(|p| p.place == AutoReplyPlace::Directs { radio })
                && channels.with(|c| match c.get(&radio) {
                    Some(Some(list)) => list.iter().all(|channel| {
                        let place = channel_place(radio, channel);
                        mine.iter().any(|p| same_place(&p.place, &place))
                    }),
                    _ => true,
                });
            if every { 2 } else { 1 }
        })
    });
    let set_all = move |on: bool| {
        let Some(mut changed) = settings.get_untracked() else {
            return;
        };
        changed.places.retain(|p| place_radio(&p.place) != radio);
        if on {
            let all = |place| NotificationPlace {
                place,
                mentions: false,
            };
            changed.places.push(all(AutoReplyPlace::Directs { radio }));
            channels.with_untracked(|c| {
                if let Some(Some(list)) = c.get(&radio) {
                    for channel in list {
                        changed.places.push(all(channel_place(radio, channel)));
                    }
                }
            });
        }
        keep(settings, changed);
    };
    // Folded until opened, to pick each of its conversations.
    let open = RwSignal::new(false);
    let style = badge::style(&badge);
    let class = format!("rail-button radio notify-badge {}", badge::class(&badge));
    view! {
        <div class="card notify-radio" class:open=move || open.get()>
            <header>
                <button
                    type="button"
                    class="notify-radio-head"
                    aria-expanded=move || open.get().to_string()
                    on:click=move |_| open.update(|o| *o = !*o)
                >
                    <span class=class style=style>
                        <Face name=name.clone() badge=badge short=short />
                    </span>
                    <span class="notify-radio-name">
                        <b>{name}</b>
                        <span class="dim">{summary}</span>
                    </span>
                    <span class="chevron" inner_html=CHEVRON></span>
                </button>
                <Level level set_all />
            </header>
            <Show when=move || open.get()>
                <div class="notify-radio-body">{directs} {rest}</div>
            </Show>
        </div>
    }
}

/// How much of a radio is notified of, as a pill of three: none and all
/// are picked; some is so when it is, and is not picked but by picking
/// within the radio. Words where there is room; on a phone, circles.
#[component]
fn Level(level: Memo<usize>, set_all: impl Fn(bool) + Copy + 'static) -> impl IntoView {
    let i18n = use_i18n();
    let strip = NodeRef::<leptos::html::Div>::new();
    let pill = slide(strip, level);
    let word = move |at: usize| {
        match at {
            0 => t_string!(i18n, notifications_level_none),
            1 => t_string!(i18n, notifications_level_some),
            _ => t_string!(i18n, notifications_level_all),
        }
        .to_owned()
    };
    let buttons = [NONE, SOME, ALL]
        .into_iter()
        .enumerate()
        .map(|(at, icon)| {
            let on = move || level.get() == at;
            view! {
                <button
                    type="button"
                    role="radio"
                    class:on=on
                    class:some=at == 1
                    aria-checked=move || on().to_string()
                    aria-disabled=(at == 1).then_some("true")
                    data-label=move || word(at)
                    title=move || word(at)
                    aria-label=move || word(at)
                    on:click=move |_| match at {
                        0 => set_all(false),
                        1 => {}
                        _ => set_all(true),
                    }
                >
                    <span class="level-icon" inner_html=icon></span>
                    <span class="level-word">{move || word(at)}</span>
                </button>
            }
        })
        .collect_view();
    view! {
        <div
            class="segmented fitted notify-level"
            role="radiogroup"
            node_ref=strip
            style=move || format!("--n: 3; --at: {}; {}", level.get(), pill.get())
        >
            {buttons}
        </div>
    }
}

fn place_radio(place: &AutoReplyPlace) -> RadioId {
    match place {
        AutoReplyPlace::Directs { radio }
        | AutoReplyPlace::Node { radio, .. }
        | AutoReplyPlace::Channel { radio, .. } => *radio,
    }
}
