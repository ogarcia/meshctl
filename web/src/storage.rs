// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What meshctl keeps, and for how long: how much there is and the room it
//! takes, the times after which each is cleaned away, and what was left
//! behind, to forget as asked.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::radios::Switch;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use meshctl_api::{Cleaned, Gone, Retention, Storage};

/// Which of the times a row sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kept {
    Packets,
    Nodes,
    Messages,
}

impl Kept {
    fn of(self, retention: &Retention) -> Option<u32> {
        match self {
            Self::Packets => retention.packets,
            Self::Nodes => retention.nodes,
            Self::Messages => retention.messages,
        }
    }

    fn set(self, retention: &mut Retention, days: Option<u32>) {
        match self {
            Self::Packets => retention.packets = days,
            Self::Nodes => retention.nodes = days,
            Self::Messages => retention.messages = days,
        }
    }

    /// The time a row takes when switched on, as it is by default, or a
    /// year for what is kept for ever by default.
    fn default_days(self) -> u32 {
        self.of(&Retention::default()).unwrap_or(365)
    }
}

/// A count, its thousands set apart as the language does.
fn count(locale: Locale, n: u64) -> String {
    let separator = match locale {
        Locale::es => '.',
        _ => ',',
    };
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(digit);
    }
    out
}

/// A size in bytes, in the largest unit that reads well, its decimals as
/// the language writes them.
fn size(locale: Locale, bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let (value, unit) = match bytes {
        0..1_024 => return format!("{bytes} B"),
        1_024..1_048_576 => (bytes as f64 / 1_024.0, "KB"),
        1_048_576..1_073_741_824 => (bytes as f64 / 1_048_576.0, "MB"),
        _ => (bytes as f64 / 1_073_741_824.0, "GB"),
    };
    let written = format!("{value:.1}");
    let written = match locale {
        Locale::es => written.replace('.', ","),
        _ => written,
    };
    format!("{written} {unit}")
}

/// What it says of a cleaning.
fn cleaned_words(i18n: I18nContext<Locale>, cleaned: Cleaned) -> String {
    let locale = i18n.get_locale_untracked();
    if cleaned == Cleaned::default() {
        return t_string!(i18n, data_nothing).to_owned();
    }
    t_string!(i18n, data_cleaned)
        .replace("{packets}", &count(locale, cleaned.packets))
        .replace("{nodes}", &count(locale, cleaned.nodes))
        .replace("{messages}", &count(locale, cleaned.messages))
        .replace("{bytes}", &size(locale, cleaned.bytes))
}

#[component]
pub fn DataView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let storage = RwSignal::new(None::<Storage>);
    let retention = RwSignal::new(None::<Retention>);
    let problem = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    let result = RwSignal::new(None::<Cleaned>);
    // Read on opening, and again whenever anything was cleaned or the times
    // changed, here or from another browser.
    Effect::new(move |_| {
        live.storage_turn.track();
        spawn_local(async move {
            if let Ok(read) = daemon::kept().await {
                storage.set(Some(read));
            }
            if let Ok(read) = daemon::retention().await {
                retention.set(Some(read));
            }
        });
    });
    let save = move |changed: Retention| {
        retention.set(Some(changed));
        problem.set(None);
        spawn_local(async move {
            if let Err(failure) = daemon::set_retention(&changed).await {
                problem.set(Some(failure));
            }
        });
    };
    let clean = move |_| {
        busy.set(true);
        result.set(None);
        spawn_local(async move {
            match daemon::clean().await {
                Ok(cleaned) => result.set(Some(cleaned)),
                Err(failure) => problem.set(Some(failure)),
            }
            busy.set(false);
        });
    };

    let row = move |kept: Kept| {
        let (title, hint) = match kept {
            Kept::Packets => (
                t!(i18n, data_packets_title).into_any(),
                t!(i18n, data_packets_hint).into_any(),
            ),
            Kept::Nodes => (
                t!(i18n, data_nodes_title).into_any(),
                t!(i18n, data_nodes_hint).into_any(),
            ),
            Kept::Messages => (
                t!(i18n, data_messages_title).into_any(),
                t!(i18n, data_messages_hint).into_any(),
            ),
        };
        let days = move || retention.with(|r| r.as_ref().and_then(|r| kept.of(r)));
        let on = Signal::derive(move || days().is_some());
        let flip = move |_| {
            let Some(mut changed) = retention.get_untracked() else {
                return;
            };
            let now = if kept.of(&changed).is_some() {
                None
            } else {
                Some(kept.default_days())
            };
            kept.set(&mut changed, now);
            save(changed);
        };
        let typed = move |ev: leptos::ev::Event| {
            let Some(mut changed) = retention.get_untracked() else {
                return;
            };
            let Ok(days) = event_target_value(&ev).trim().parse::<u32>() else {
                // Not a number: back to what it was.
                retention.update(|_| {});
                return;
            };
            kept.set(&mut changed, Some(days.clamp(1, Retention::LONGEST)));
            save(changed);
        };
        view! {
            <div class="pref-row">
                <div class="pref-what">
                    <span class="pref-title">{title}</span>
                    <span class="dim">{hint}</span>
                </div>
                <div class="pref-control retention">
                    <Switch
                        on
                        label=Signal::derive(move || t_string!(i18n, data_clean_after).to_string())
                        flip
                    />
                    <span class:dim=move || !on.get()>{t!(i18n, data_clean_after)}</span>
                    <input
                        type="number"
                        min="1"
                        max=Retention::LONGEST.to_string()
                        inputmode="numeric"
                        disabled=move || !on.get()
                        prop:value=move || days().unwrap_or_else(|| kept.default_days()).to_string()
                        on:change=typed
                    />
                    <span class:dim=move || !on.get()>{t!(i18n, data_days)}</span>
                </div>
            </div>
        }
    };

    // The rows once the times are read: drawn before, their switches would
    // show off and then slide on. Once, not again as each time changes,
    // which would lose the field being written in.
    let loaded = Memo::new(move |_| retention.with(Option::is_some));
    let rows = move || {
        loaded.get().then(|| {
            view! {
                {row(Kept::Packets)}
                {row(Kept::Nodes)}
                {row(Kept::Messages)}
            }
        })
    };

    let summary = move || {
        let read = storage.get()?;
        let locale = i18n.get_locale();
        let since = read.oldest_packet.map(|at| {
            let now = live.now.get();
            crate::mesh::ago(now, u32::try_from(at).unwrap_or(now))
        });
        // As a node's figures are drawn.
        Some(view! {
            <div class="node-figures data-figures">
                <div class="figure">
                    <span class="label">{t!(i18n, data_size)}</span>
                    <span class="value">{size(locale, read.bytes)}</span>
                </div>
                <div class="figure">
                    <span class="label">{t!(i18n, data_packets)}</span>
                    <span class="value">{count(locale, read.packets)}</span>
                    {since.map(|ago| view! {
                        <span class="sub">{t_string!(i18n, data_since).replace("{ago}", &ago)}</span>
                    })}
                </div>
                <div class="figure">
                    <span class="label">{t!(i18n, data_nodes)}</span>
                    <span class="value">{count(locale, read.nodes)}</span>
                </div>
                <div class="figure">
                    <span class="label">{t!(i18n, data_messages)}</span>
                    <span class="value">{count(locale, read.messages)}</span>
                </div>
            </div>
        })
    };

    view! {
        <section class="preferences data">
            {summary}
            <h2>{t!(i18n, data_kept)}</h2>
            {rows}
            <div class="data-clean">
                <span class="dim">{t!(i18n, data_daily)}</span>
                <button type="button" disabled=move || busy.get() on:click=clean>
                    {t!(i18n, data_clean)}
                </button>
            </div>
            {move || result.get().map(|cleaned| view! { <p class="data-result">{cleaned_words(i18n, cleaned)}</p> })}
            {move || problem.get().map(|_| view! { <p class="error">{t!(i18n, error_failed)}</p> })}
            <h2>{t!(i18n, data_left)}</h2>
            <LeftBehind
                title=t!(i18n, data_gone_channels).into_any()
                gone=Signal::derive(move || storage.with(|s| s.as_ref().map(|s| s.gone_channels)))
                channels=true
            />
            <LeftBehind
                title=t!(i18n, data_gone_directs).into_any()
                gone=Signal::derive(move || storage.with(|s| s.as_ref().map(|s| s.gone_directs)))
                channels=false
            />
        </section>
    }
}

/// Conversations left behind: how many, and forgetting them once sure.
#[component]
fn LeftBehind(title: AnyView, gone: Signal<Option<Gone>>, channels: bool) -> impl IntoView {
    let i18n = use_i18n();
    let sure = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let failed = RwSignal::new(false);
    let none = move || gone.get().is_none_or(|g| g.conversations == 0);
    let hint = move || {
        let locale = i18n.get_locale();
        match gone.get() {
            Some(g) if g.conversations > 0 => t_string!(i18n, data_gone_count)
                .replace("{conversations}", &count(locale, g.conversations))
                .replace("{messages}", &count(locale, g.messages)),
            _ => t_string!(i18n, data_gone_none).to_owned(),
        }
    };
    let forget = move |_| {
        busy.set(true);
        failed.set(false);
        spawn_local(async move {
            let done = if channels {
                daemon::forget_gone_channels().await
            } else {
                daemon::forget_gone_directs().await
            };
            // Read again as every client hears of it.
            failed.set(done.is_err());
            busy.set(false);
            sure.set(false);
        });
    };
    view! {
        <div class="pref-row">
            <div class="pref-what">
                <span class="pref-title">{title}</span>
                <span class="dim">{hint}</span>
                {move || failed.get().then(|| view! { <span class="error">{t!(i18n, error_failed)}</span> })}
            </div>
            <div class="data-forget">
                {move || if sure.get() {
                    view! {
                        <button type="button" on:click=move |_| sure.set(false)>
                            {t!(i18n, data_cancel)}
                        </button>
                        <button type="button" class="danger" disabled=move || busy.get() on:click=forget>
                            {t!(i18n, data_sure)}
                        </button>
                    }
                    .into_any()
                } else {
                    view! {
                        <button
                            type="button"
                            class="danger-outline"
                            disabled=none
                            on:click=move |_| sure.set(true)
                        >
                            {t!(i18n, data_forget)}
                        </button>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
}
