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
use meshctl_api::MeshcoreRegions;
use std::time::Duration;

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
