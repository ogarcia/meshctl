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
use crate::preferences::{Choice, pills};
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
            permission.set(notify::ask().await);
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
                <div class="notify-head">
                    <div class="notify-what">
                        <h3>{t!(i18n, notifications_show_text)}</h3>
                        <span class="dim">{t!(i18n, notifications_show_text_hint)}</span>
                    </div>
                    <Switch
                        on=Signal::derive(show_text)
                        label=Signal::derive(move || {
                            t_string!(i18n, notifications_show_text).to_string()
                        })
                        flip=move |_| {
                            let on = !show_text();
                            change(&|n| n.show_text = on);
                        }
                    />
                </div>
                <div class="notify-sample" aria-hidden="true">
                    <img class="sample-app" src="/favicon.svg" alt="" />
                    <div class="sample-body">
                        <span class="sample-top">
                            <span>"meshctl"</span>
                            <span class="dim">{t!(i18n, notifications_sample_when)}</span>
                        </span>
                        <b>{t!(i18n, notifications_sample_title)}</b>
                        {move || {
                            if show_text() {
                                view! { <span>{t!(i18n, notifications_sample_text)}</span> }
                                    .into_any()
                            } else {
                                view! {
                                    <span class="dim hidden-text">
                                        {t!(i18n, notifications_sample_hidden)}
                                    </span>
                                }
                                    .into_any()
                            }
                        }}
                    </div>
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
        // where it was, to be put back.
        settings.with(|s| s.is_some());
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
    let style = badge::style(&badge);
    let class = format!("rail-button radio notify-badge {}", badge::class(&badge));
    view! {
        <div class="card notify-radio">
            <header>
                <span class=class style=style>
                    <Face name=name.clone() badge=badge short=short />
                </span>
                <span class="notify-radio-name">
                    <b>{name}</b>
                    <span class="dim">{summary}</span>
                </span>
            </header>
            {directs}
            {rest}
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
