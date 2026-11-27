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
const SEARCH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/search.svg");

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

/// The regions a chat can flood in, read: its channel's, the radio's, and
/// every one known.
async fn defaults_of(radio: meshctl_api::RadioId, peer: Peer) -> Defaults {
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
    Defaults {
        channel,
        radio: radio_default,
        known,
    }
}

/// A channel's regions, as its chat's heading shows them: read once for its
/// subtitle and its button, again as the radio is read.
#[derive(Clone, Copy)]
pub struct ChannelRegions(RwSignal<Defaults>, FoundRegions);

impl ChannelRegions {
    /// Read for a channel, while the radio is a MeshCore one.
    pub fn of(channel: u32) -> Self {
        let live = expect_context::<Live>();
        let defaults = RwSignal::new(Defaults::default());
        Effect::new(move |_| {
            live.synced.track();
            let Some(radio) = live.selected.get() else {
                return;
            };
            if live.selected_kind() != Some(meshctl_api::RadioKind::Meshcore) {
                return;
            }
            spawn_local(async move {
                let read = defaults_of(radio, Peer::Channel { channel }).await;
                if live.selected.get_untracked() == Some(radio) {
                    let _ = defaults.try_set(read);
                }
            });
        });
        Self(defaults, FoundRegions::read())
    }
}

/// The region what is sent on a channel floods in, in words: its own, the
/// radio's said as such, or none.
#[component]
pub fn RegionWords(regions: ChannelRegions) -> impl IntoView {
    let i18n = use_i18n();
    let ChannelRegions(defaults, found) = regions;
    let words = move || {
        defaults.with(|d| match (&d.channel, &d.radio) {
            (Some(own), _) if own == "*" => t_string!(i18n, regions_subtitle_none).to_owned(),
            (Some(own), _) => format!("#{own}"),
            (None, Some(radio)) => t_string!(i18n, regions_subtitle_radio).replace("{r}", radio),
            (None, None) => t_string!(i18n, regions_subtitle_none).to_owned(),
        })
    };
    // What is sent floods in a region no repeater heard straight serves:
    // said beside it, as it goes nowhere.
    let unserved = move || {
        let effective = defaults.with(|d| {
            d.channel
                .clone()
                .or_else(|| d.radio.clone())
                .unwrap_or_else(|| "*".to_owned())
        });
        found.unserved(&effective)
    };
    view! {
        {words}
        {move || {
            unserved().then(|| {
                view! {
                    <span class="region-unserved" title=move || t_string!(i18n, regions_subtitle_unserved)>
                        " · " {t!(i18n, regions_subtitle_unserved_short)}
                    </span>
                }
            })
        }}
    }
}

/// A MeshCore channel's own region, in its chat's heading: the one what is
/// sent on it floods in, picked for good, the radio's by default.
#[component]
pub fn ChannelRegion(channel: u32, regions: ChannelRegions) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let ChannelRegions(defaults, found) = regions;
    let closing = window_event_listener(leptos::ev::click, move |_| open.set(false));
    on_cleanup(move || closing.remove());
    let words = move |name: &str| {
        if name == "*" {
            t_string!(i18n, chat_scope_none).to_owned()
        } else {
            format!("#{name}")
        }
    };
    let radio_words = move || {
        defaults.with(|d| {
            t_string!(i18n, chat_scope_radio).replace(
                "{r}",
                &d.radio
                    .as_deref()
                    .map_or_else(|| t_string!(i18n, chat_scope_none).to_owned(), words),
            )
        })
    };
    let title = move || {
        defaults.with(|d| match &d.channel {
            Some(own) => t_string!(i18n, chat_scope_channel).replace("{r}", &words(own)),
            None => radio_words(),
        })
    };
    let pick = move |scope: Option<String>| {
        open.set(false);
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        defaults.update(|d| d.channel.clone_from(&scope));
        spawn_local(async move {
            let _ = daemon::set_channel_scope(radio, channel, scope).await;
        });
    };
    let item = move |value: Option<String>, text: Signal<String>| {
        let on = {
            let value = value.clone();
            Signal::derive(move || defaults.with(|d| d.channel == value))
        };
        // How many repeaters serve it, heard straight and in all: the
        // radio's default counted as the region it is.
        let region = {
            let value = value.clone();
            move || {
                value
                    .clone()
                    .or_else(|| defaults.with(|d| d.radio.clone()))
                    .unwrap_or_else(|| "*".to_owned())
            }
        };
        let served = {
            let region = region.clone();
            Signal::derive(move || found.served(&region()))
        };
        let far = Signal::derive(move || found.unserved(&region()));
        let served = move || served.get();
        let title = move || {
            served().map(|(direct, all)| {
                t_string!(i18n, regions_served)
                    .replace("{n}", &all.to_string())
                    .replace("{d}", &direct.to_string())
            })
        };
        view! {
            <button
                role="menuitemradio"
                class:on=on
                class:far=far
                aria-checked=move || on.get().to_string()
                title=title
                on:click=move |_| pick(value.clone())
            >
                <span class="icon" inner_html=WORLD></span>
                <span class="label">{text}</span>
                <span class="served">
                    {move || served().map(|(direct, all)| format!("{direct}/{all}"))}
                </span>
                <span class="check" inner_html=move || if on.get() { CHECK } else { "" }></span>
            </button>
        }
    };
    view! {
        <span class="channel-region">
            <button
                type="button"
                class="icon-button"
                class:on=move || open.get()
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
                <div class="heading-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                    <p class="dim">{t!(i18n, mc_channel_scope_menu)}</p>
                    {item(None, Signal::derive(radio_words))}
                    {item(Some("*".to_owned()), Signal::derive(move || t_string!(i18n, chat_scope_none).to_owned()))}
                    {move || {
                        defaults
                            .with(|d| d.known.clone())
                            .into_iter()
                            .map(|name| {
                                let shown = format!("#{name}");
                                item(Some(name), Signal::derive(move || shown.clone()))
                            })
                            .collect_view()
                    }}
                    // And a search of them, from here, set apart.
                    <div class="separator" role="separator"></div>
                    <button
                        role="menuitem"
                        class="region-search"
                        disabled=move || found.searching()
                        on:click=move |ev| {
                            ev.stop_propagation();
                            found.search();
                        }
                    >
                        <span class="icon" inner_html=SEARCH></span>
                        <span class="label">{move || found.button_words(i18n)}</span>
                    </button>
                </div>
            </Show>
        </span>
    }
}

/// How often a search going on is read again.
const FOLLOW: Duration = Duration::from_secs(2);

/// The regions found nearby, as the radio's repeaters and rooms said them:
/// read as it shows, again as the radio syncs, and followed while a search
/// goes on.
#[derive(Clone, Copy)]
pub struct FoundRegions(RwSignal<Option<MeshcoreRegions>>);

impl FoundRegions {
    pub fn read() -> Self {
        let live = expect_context::<Live>();
        let found = RwSignal::new(None::<MeshcoreRegions>);
        let read = move || {
            let Some(radio) = live.selected.get_untracked() else {
                return;
            };
            if live.selected_kind() != Some(meshctl_api::RadioKind::Meshcore) {
                return;
            }
            spawn_local(async move {
                if let Ok(read) = daemon::meshcore_regions(radio).await
                    && live.selected.get_untracked() == Some(radio)
                {
                    found.try_set(Some(read));
                }
            });
        };
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
        Self(found)
    }

    /// Asks the radio's repeaters and rooms again.
    pub fn search(self) {
        let live = expect_context::<Live>();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let Self(found) = self;
        spawn_local(async move {
            if let Ok(started) = daemon::search_regions(radio).await {
                found.try_set(Some(started));
            }
        });
    }

    pub fn searching(self) -> bool {
        self.0.with(|f| f.as_ref().is_some_and(|f| f.searching))
    }

    /// Every region found, the most served first.
    pub fn names(self) -> Vec<String> {
        self.0.with(|f| {
            f.as_ref()
                .map(|f| f.regions.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    /// How many repeaters serve a region, `*` for none, those heard
    /// straight and in all; none when nothing was found.
    pub fn served(self, name: &str) -> Option<(u32, u32)> {
        self.0.with(|f| {
            let f = f.as_ref().filter(|f| !f.repeaters.is_empty())?;
            Some(if name == "*" {
                (f.unscoped_direct, f.unscoped)
            } else {
                f.regions
                    .iter()
                    .find(|r| r.name == name)
                    .map_or((0, 0), |r| (r.direct, r.repeaters))
            })
        })
    }

    /// Whether what floods in a region, `*` for none, is known to reach no
    /// repeater heard straight: only when any of those said its regions.
    pub fn unserved(self, name: &str) -> bool {
        let direct_known = self.0.with(|f| {
            f.as_ref()
                .is_some_and(|f| f.repeaters.iter().any(|r| r.hops == Some(0)))
        });
        direct_known && self.served(name).is_some_and(|(direct, _)| direct == 0)
    }

    /// The button that searches, or says how many are left to ask.
    pub fn button_words(self, i18n: leptos_i18n::I18nContext<crate::i18n::Locale>) -> String {
        self.0.with(|f| match f {
            Some(f) if f.searching => {
                t_string!(i18n, regions_searching).replace("{n}", &f.left.max(1).to_string())
            }
            _ => t_string!(i18n, regions_search).to_owned(),
        })
    }
}

/// The regions found, to pick one from: `scope` is the region it floods in
/// now, none for none; `pick` chooses another, or none.
#[component]
pub fn RegionFinder(
    scope: Signal<Option<String>>,
    pick: impl Fn(Option<String>) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let regions = FoundRegions::read();
    let FoundRegions(found) = regions;
    let search = move |_| regions.search();
    let searching = move || regions.searching();
    let button = move || regions.button_words(i18n);
    // The region chosen, known to reach no repeater heard straight.
    let warning = move || match scope.get() {
        Some(name) => regions
            .unserved(&name)
            .then(|| t_string!(i18n, regions_unserved).replace("{name}", &name)),
        None => regions
            .unserved("*")
            .then(|| t_string!(i18n, regions_unscoped_unserved).to_owned()),
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
