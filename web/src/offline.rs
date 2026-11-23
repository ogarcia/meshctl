// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio out of reach, said the same way on every page: switched off,
//! not connected, or meshctl itself not answering. A strip over what is
//! known of it, or, where nothing is, the page's empty state.

use crate::app::{Daemon, LinkLabel, Live};
use crate::empty::{self, EmptyState};
use crate::i18n::{t, use_i18n};
use crate::mesh::ago;
use leptos::prelude::*;
use meshctl_api::{LinkState, Radio};

/// Why the selected radio is out of reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offline {
    /// meshctl itself does not answer.
    Daemon,
    /// Switched off: meshctl does not try.
    Disabled,
    /// meshctl tries, and does not get to it.
    Unreachable,
}

/// The selected radio, and why it is out of reach, if it is.
pub fn offline(live: Live) -> Option<(Offline, Radio)> {
    let id = live.selected.get()?;
    let radio = live
        .radios
        .with(|radios| radios.as_ref()?.iter().find(|r| r.id == id).cloned())?;
    let why = if live.daemon.get() != Daemon::Connected {
        Offline::Daemon
    } else {
        match radio.link {
            LinkState::Ready | LinkState::Syncing => return None,
            LinkState::Disabled => Offline::Disabled,
            LinkState::Connecting | LinkState::Disconnected => Offline::Unreachable,
        }
    };
    Some((why, radio))
}

fn icon(why: Offline) -> &'static str {
    match why {
        Offline::Daemon => empty::SERVER_OFF,
        Offline::Disabled => empty::POWER,
        Offline::Unreachable => empty::PLUG_OFF,
    }
}

fn title(why: Offline, name: String) -> AnyView {
    let i18n = use_i18n();
    match why {
        Offline::Daemon => t!(i18n, empty_daemon_title).into_any(),
        Offline::Disabled => t!(i18n, empty_disabled_title, name = name).into_any(),
        Offline::Unreachable => t!(i18n, empty_offline_title, name = name).into_any(),
    }
}

/// What can be done: switch it on, or see how its link is doing.
fn action(why: Offline, radio: Radio) -> Option<AnyView> {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    match why {
        Offline::Daemon => None,
        Offline::Disabled => Some(
            view! {
                <button
                    type="button"
                    class="primary"
                    on:click=move |_| crate::radios::quick(live, &radio, |s| s.enabled = true)
                >
                    {t!(i18n, empty_enable)}
                </button>
            }
            .into_any(),
        ),
        Offline::Unreachable => {
            let link = Signal::stored(radio.link);
            Some(
                view! {
                    <span class=move || format!("badge {}", live.link_class(link.get()))>
                        <LinkLabel link />
                    </span>
                }
                .into_any(),
            )
        }
    }
}

/// A strip over what is known of a radio out of reach: why, how old what
/// shows is, and what can be done. `node` adds that a node's page can be
/// read but nothing asked of it.
#[component]
pub fn OfflineBanner(#[prop(optional)] node: bool) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || {
        let (why, radio) = offline(live)?;
        let since = radio.last_connected.map(|at| {
            let at = u32::try_from(at).unwrap_or_default();
            t!(
                i18n,
                radio_last_connected,
                ago = move || ago(live.now.get(), at)
            )
            .into_any()
        });
        let detail = match (why, since) {
            (Offline::Daemon, _) => t!(i18n, offline_daemon_short).into_any(),
            (_, Some(since)) => since,
            (Offline::Disabled, None) => t!(i18n, offline_disabled_short).into_any(),
            (Offline::Unreachable, None) => t!(i18n, offline_unreachable_short).into_any(),
        };
        let asking = node.then(|| view! { " " {t!(i18n, offline_node)} });
        Some(view! {
            <div class="offline-banner" class:disabled=why == Offline::Disabled>
                <span class="offline-icon" inner_html=icon(why)></span>
                <div class="offline-words">
                    <strong>{title(why, radio.name.clone())}</strong>
                    <span>{detail} {asking}</span>
                </div>
                {action(why, radio)}
            </div>
        })
    }
}

/// A page with nothing of a radio out of reach to show: why, in the middle,
/// and what can be done.
#[component]
pub fn OfflineEmpty(why: Offline, radio: Radio) -> impl IntoView {
    let i18n = use_i18n();
    let text = match why {
        Offline::Daemon => t!(i18n, empty_daemon_text).into_any(),
        Offline::Disabled => t!(i18n, offline_disabled_text).into_any(),
        Offline::Unreachable => t!(i18n, offline_unreachable_text).into_any(),
    };
    let title = title(why, radio.name.clone());
    match action(why, radio) {
        Some(action) => view! {
            <EmptyState icon=icon(why) title text>
                {action}
            </EmptyState>
        }
        .into_any(),
        None => view! { <EmptyState icon=icon(why) title text /> }.into_any(),
    }
}
