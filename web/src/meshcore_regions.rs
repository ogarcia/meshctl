// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The regions a MeshCore radio's repeaters repeat on, by the region it
//! floods in: searched for, asking those it knows a path to; each found
//! offered to flood in; and a word when the one chosen goes nowhere, as no
//! repeater it hears straight passes it on.

use crate::app::Live;
use crate::daemon;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{MeshcoreRegions, Peer};
use std::time::Duration;

pub const WORLD: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/world.svg");
const CHECK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/check.svg");

/// The region a message floods in by default, its channel's or the radio's,
/// as said: `#name`, or none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Defaults {
    /// The channel's region, `*` for none, if it has one of its own.
    channel: Option<String>,
    /// The radio's default region, if it has one.
    radio: Option<String>,
    /// Every region known: found nearby, of a channel, or the radio's.
    known: Vec<String>,
}

/// The region of a MeshCore chat's next message: a button in the field,
/// whose menu says the one it goes in by default, its channel's or the
/// radio's, and picks another for it alone, or none. `choice` is that
/// one, none for the default.
#[component]
pub fn ScopePicker(peer: Peer, choice: RwSignal<Option<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let defaults = RwSignal::new(Defaults::default());
    // Read on showing, and as the radio is read again.
    Effect::new(move |_| {
        live.synced.track();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            let channels = daemon::meshcore_channels(radio).await.ok();
            let settings = daemon::meshcore_settings(radio).await.ok();
            let found = daemon::meshcore_regions(radio).await.ok();
            let channel = match peer {
                Peer::Channel { channel } => channels.as_ref().and_then(|c| {
                    c.channels
                        .iter()
                        .find(|c| c.index == channel)
                        .and_then(|c| c.scope.clone())
                }),
                Peer::Node { .. } => None,
            };
            let radio_default = settings.and_then(|s| s.default_scope);
            let mut known: Vec<String> = found
                .map(|f| f.regions.into_iter().map(|r| r.name).collect())
                .unwrap_or_default();
            known.extend(
                channels
                    .into_iter()
                    .flat_map(|c| c.channels)
                    .filter_map(|c| c.scope)
                    .filter(|s| s != "*"),
            );
            known.extend(radio_default.clone());
            let mut seen = std::collections::HashSet::new();
            known.retain(|name| seen.insert(name.clone()));
            defaults.set(Defaults {
                channel,
                radio: radio_default,
                known,
            });
        });
    });
    let closing = window_event_listener(leptos::ev::click, move |_| open.set(false));
    let escaping = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            open.set(false);
        }
    });
    on_cleanup(move || {
        closing.remove();
        escaping.remove();
    });
    let words = move |name: &str| {
        if name == "*" {
            t_string!(i18n, chat_scope_none).to_owned()
        } else {
            format!("#{name}")
        }
    };
    // What it goes in by default, in words.
    let default_words = move || {
        defaults.with(|d| match (&d.channel, &d.radio) {
            (Some(channel), _) => {
                t_string!(i18n, chat_scope_channel).replace("{r}", &words(channel))
            }
            (None, Some(radio)) => t_string!(i18n, chat_scope_radio).replace("{r}", &words(radio)),
            (None, None) => {
                t_string!(i18n, chat_scope_radio).replace("{r}", t_string!(i18n, chat_scope_none))
            }
        })
    };
    let title = move || match choice.get() {
        Some(name) => t_string!(i18n, chat_scope_once_title).replace("{r}", &words(&name)),
        None => default_words(),
    };
    let item = move |value: Option<String>, label: Signal<String>| {
        let on = {
            let value = value.clone();
            Signal::derive(move || choice.get() == value)
        };
        view! {
            <button
                type="button"
                role="menuitemradio"
                class:on=on
                aria-checked=move || on.get().to_string()
                on:click=move |_| {
                    choice.set(value.clone());
                    open.set(false);
                }
            >
                <span class="label">{label}</span>
                <span class="icon" inner_html=move || if on.get() { CHECK } else { "" }></span>
            </button>
        }
    };
    view! {
        <button
            type="button"
            class="icon-button scope-toggle"
            class:on=move || choice.get().is_some()
            aria-haspopup="menu"
            aria-expanded=move || open.get().to_string()
            title=title
            aria-label=title
            inner_html=WORLD
            on:click=move |ev| {
                ev.stop_propagation();
                open.update(|o| *o = !*o);
            }
        ></button>
        <Show when=move || open.get()>
            <div class="heading-menu scope-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                <p class="dim">{t!(i18n, chat_scope_menu)}</p>
                {item(None, Signal::derive(default_words))}
                {item(Some("*".to_owned()), Signal::derive(move || t_string!(i18n, chat_scope_none).to_owned()))}
                // Each other region known: the one it goes in by default is
                // the first already.
                {move || {
                    let (known, usual) =
                        defaults.with(|d| (d.known.clone(), d.channel.clone().or(d.radio.clone())));
                    known
                        .into_iter()
                        .filter(|name| Some(name) != usual.as_ref())
                        .map(|name| {
                            let label = format!("#{name}");
                            item(Some(name), Signal::derive(move || label.clone()))
                        })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}

/// How often a search going on is read again.
const FOLLOW: Duration = Duration::from_secs(2);

/// The regions found, to pick one from: `scope` is the region it floods in
/// now, none for none; `pick` chooses another, or none.
#[component]
pub fn RegionFinder(
    scope: Signal<Option<String>>,
    pick: impl Fn(Option<String>) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let found = RwSignal::new(None::<MeshcoreRegions>);
    let read = move || {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(read) = daemon::meshcore_regions(radio).await
                && live.selected.get_untracked() == Some(radio)
            {
                found.set(Some(read));
            }
        });
    };
    // On showing, as the radio is read again, which a search ends with, and
    // while one goes on.
    Effect::new(move |_| {
        live.synced.track();
        read();
    });
    if let Ok(every) = set_interval_with_handle(
        move || {
            if found.with_untracked(|f| f.as_ref().is_some_and(|f| f.searching)) {
                read();
            }
        },
        FOLLOW,
    ) {
        on_cleanup(move || every.clear());
    }
    let search = move |_| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(started) = daemon::search_regions(radio).await {
                found.set(Some(started));
            }
        });
    };
    let searching = move || found.with(|f| f.as_ref().is_some_and(|f| f.searching));
    let button = move || {
        found.with(|f| match f {
            Some(f) if f.searching => {
                t_string!(i18n, regions_searching).replace("{n}", &f.left.max(1).to_string())
            }
            _ => t_string!(i18n, regions_search).to_owned(),
        })
    };
    // Whether any repeater heard straight said anything: only then can it
    // be told that the region chosen goes nowhere.
    let direct_known = move || {
        found.with(|f| {
            f.as_ref()
                .is_some_and(|f| f.repeaters.iter().any(|r| r.hops == Some(0)))
        })
    };
    let warning = move || {
        if !direct_known() {
            return None;
        }
        found.with(|f| {
            let f = f.as_ref()?;
            match scope.get() {
                Some(name) => {
                    let served = f.regions.iter().any(|r| r.name == name && r.direct > 0);
                    (!served).then(|| t_string!(i18n, regions_unserved).replace("{name}", &name))
                }
                None => (f.unscoped_direct == 0)
                    .then(|| t_string!(i18n, regions_unscoped_unserved).to_owned()),
            }
        })
    };
    let chip = move |name: Option<String>, repeaters: u32, direct: u32| {
        let label = name.as_ref().map_or_else(
            || t_string!(i18n, regions_unscoped).to_owned(),
            |n| format!("#{n}"),
        );
        let title = t_string!(i18n, regions_served)
            .replace("{n}", &repeaters.to_string())
            .replace("{d}", &direct.to_string());
        let chosen = {
            let name = name.clone();
            move || scope.get() == name
        };
        view! {
            <button
                type="button"
                class="choice"
                class:on=chosen
                class:far=direct == 0
                title=title
                on:click=move |_| pick(name.clone())
            >
                {label}
                <span class="served">{format!("{direct}/{repeaters}")}</span>
            </button>
        }
    };
    let chips = move || {
        found.with(|f| {
            let f = f.as_ref()?;
            if f.repeaters.is_empty() {
                return None;
            }
            let mut all = Vec::new();
            if f.unscoped > 0 {
                all.push(chip(None, f.unscoped, f.unscoped_direct).into_any());
            }
            for region in &f.regions {
                all.push(
                    chip(Some(region.name.clone()), region.repeaters, region.direct).into_any(),
                );
            }
            Some(all)
        })
    };
    let asked = move || found.with(|f| f.as_ref().map_or(0, |f| f.repeaters.len()));
    view! {
        <div class="pref-row region-finder">
            <div class="pref-what">
                <span class="pref-title">{t!(i18n, regions_nearby)}</span>
                <span class="dim">{t!(i18n, regions_nearby_hint)}</span>
            </div>
            <div class="pref-control region-control">
                <button type="button" disabled=searching on:click=search>
                    {button}
                </button>
                <span class="dim">
                    {move || {
                        let n = asked();
                        (n > 0).then(|| t_string!(i18n, regions_answered).replace("{n}", &n.to_string()))
                    }}
                </span>
            </div>
            <div class="flag-chips region-chips">{chips}</div>
            {move || warning().map(|text| view! { <p class="region-warning">{text}</p> })}
        </div>
    }
}
