// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! meshctl's notifications, as a page of its settings: whether it notifies
//! at all, with what messages say or not, and where, each radio's places
//! picked as an automatic reply's are; a channel, maybe only when it
//! mentions the radio.
//!
//! Each change is kept as it is made.

use crate::app::Live;
use crate::auto_replies::{channel_name, channel_place, node_label, radio_channels, same_place};
use crate::daemon;
use crate::i18n::{t, t_string, use_i18n};
use crate::notify::{self, Permission};
use crate::radios::Switch;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{AutoReplyPlace, Channel, NotificationPlace, Notifications, RadioId};
use std::collections::HashMap;

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
    // A line of its own under what the switch does.
    let browser = move || {
        if notify::in_app() || !enabled() {
            return ().into_any();
        }
        match permission.get() {
            Permission::Granted => view! {
                <span class="notify-browser dim">{t!(i18n, notifications_browser_granted)}</span>
            }
            .into_any(),
            Permission::Ask => view! {
                <span class="notify-browser notify-ask">
                    <span class="dim">{t!(i18n, notifications_browser_ask)}</span>
                    <button type="button" on:click=move |_| ask()>
                        {t!(i18n, notifications_browser_allow)}
                    </button>
                </span>
            }
            .into_any(),
            Permission::Denied => view! {
                <span class="notify-browser error">{t!(i18n, notifications_browser_denied)}</span>
            }
            .into_any(),
            Permission::Unsupported => view! {
                <span class="notify-browser error">
                    {t!(i18n, notifications_browser_unsupported)}
                </span>
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
            class="preferences data notifications"
            class:gone=move || settings.with(Option::is_none)
        >
            <div class="pref-row">
                <div class="pref-what">
                    <span class="pref-title">{t!(i18n, notifications_on)}</span>
                    <span class="dim">{t!(i18n, notifications_on_hint)}</span>
                    {browser}
                </div>
                <div class="pref-control switch-control">
                    <Switch
                        on=Signal::derive(enabled)
                        label=Signal::derive(move || t_string!(i18n, notifications_on).to_string())
                        flip=flip_enabled
                    />
                </div>
            </div>
            <div class="pref-row" class:muted=move || !enabled()>
                <div class="pref-what">
                    <span class="pref-title">{t!(i18n, notifications_show_text)}</span>
                    <span class="dim">{t!(i18n, notifications_show_text_hint)}</span>
                </div>
                <div class="pref-control switch-control">
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
            </div>
            <h2>{t!(i18n, notifications_where)}</h2>
            <div class="notify-places" class:muted=move || !enabled()>
                {wheres}
                {empty.then(|| view! { <p class="hint dim">{t!(i18n, notifications_no_radios)}</p> })}
            </div>
        </section>
    }
}

/// Where messages are notified of on one radio: its direct conversations,
/// each of its channels, everything said there or only mentions, and what
/// else it was given there.
#[component]
fn RadioPlaces(
    radio: RadioId,
    name: String,
    settings: RwSignal<Option<Notifications>>,
    channels: RwSignal<HashMap<RadioId, Option<Vec<Channel>>>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let given = move |place: &AutoReplyPlace| {
        settings.with(|s| {
            s.as_ref()
                .and_then(|s| s.places.iter().find(|p| same_place(&p.place, place)))
                .cloned()
        })
    };
    // Each change is a place in or out, or a channel's mentions flipped.
    let set = move |place: AutoReplyPlace, now: Option<bool>| {
        let Some(mut changed) = settings.get_untracked() else {
            return;
        };
        let at = changed
            .places
            .iter()
            .position(|p| same_place(&p.place, &place));
        match (at, now) {
            (Some(at), None) => {
                changed.places.remove(at);
            }
            (Some(at), Some(mentions)) => changed.places[at].mentions = mentions,
            (None, Some(mentions)) => changed.places.push(NotificationPlace { place, mentions }),
            (None, None) => return,
        }
        keep(settings, changed);
    };
    let check = move |place: AutoReplyPlace, label: String, note: Option<String>, channel: bool| {
        let on = {
            let place = place.clone();
            move || given(&place).is_some()
        };
        let mentions = {
            let place = place.clone();
            move || given(&place).is_some_and(|p| p.mentions)
        };
        let flip = {
            let place = place.clone();
            move |_| {
                let now = if given(&place).is_some() {
                    None
                } else {
                    Some(false)
                };
                set(place.clone(), now);
            }
        };
        let pick = move |ev: leptos::ev::Event| {
            set(place.clone(), Some(event_target_value(&ev) == "mentions"));
        };
        view! {
            <div class="notify-place">
                <label class="check">
                    <input type="checkbox" prop:checked=on.clone() on:change=flip />
                    <span>{label}</span>
                    {note.map(|note| view! { <span class="dim">{note}</span> })}
                </label>
                {channel
                    .then(|| {
                        view! {
                            <select
                                class="notify-mode"
                                class:gone=move || !on()
                                prop:value=move || if mentions() { "mentions" } else { "all" }
                                on:change=pick
                            >
                                <option value="all">{t!(i18n, notifications_all)}</option>
                                <option value="mentions">{t!(i18n, notifications_mentions)}</option>
                            </select>
                        }
                    })}
            </div>
        }
    };
    let directs = move || {
        check(
            AutoReplyPlace::Directs { radio },
            t_string!(i18n, reply_directs).to_owned(),
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
                    let label = format!("#{}", channel_name(channel));
                    views.push(check(place, label, None, true).into_any());
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
                        let label = format!("#{}", name.clone().unwrap_or_default());
                        let note = channel
                            .is_none()
                            .then(|| t_string!(i18n, reply_away).to_owned());
                        views.push(check(given.place.clone(), label, note, true).into_any());
                    }
                }
                AutoReplyPlace::Node { radio: r, node } if *r == radio => {
                    let label = t_string!(i18n, reply_with_node)
                        .replace("{node}", &node_label(live, radio, *node));
                    views.push(check(given.place.clone(), label, None, false).into_any());
                }
                _ => {}
            }
        }
        views.into_iter().collect_view()
    };
    view! {
        <div class="card radio-places notify-radio">
            <b>{name}</b>
            {directs}
            {rest}
        </div>
    }
}
