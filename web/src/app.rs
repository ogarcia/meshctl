// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use crate::daemon::{self, Events, Failure};
use crate::i18n::{I18nContextProvider, Locale, t, t_string, use_i18n};
use gloo_timers::future::sleep;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::events::ServerMessage;
use meshctl_api::{LinkState, Node, Status};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long to wait before trying the daemon again.
const RETRY: Duration = Duration::from_secs(3);

/// How often "seen 5 min ago" is worked out again.
const CLOCK_TICK: Duration = Duration::from_secs(15);

/// How the connection to the daemon is doing, beyond the radio's own link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Daemon {
    Connecting,
    Connected,
    Unreachable,
    NeedsToken,
}

/// Everything the interface shows, kept current by [`follow`].
#[derive(Clone, Copy)]
struct Live {
    daemon: RwSignal<Daemon>,
    status: RwSignal<Option<Status>>,
    nodes: RwSignal<BTreeMap<u32, Node>>,
    /// Seconds since the epoch, ticking.
    now: RwSignal<u32>,
}

impl Live {
    fn new() -> Self {
        Self {
            daemon: RwSignal::new(Daemon::Connecting),
            status: RwSignal::new(None),
            nodes: RwSignal::new(BTreeMap::new()),
            now: RwSignal::new(now()),
        }
    }

    /// Reads the whole state again.
    async fn refresh(self) -> Result<(), Failure> {
        let status = daemon::status().await?;
        let nodes = daemon::nodes().await?;
        self.status.set(Some(status));
        self.nodes
            .set(nodes.into_iter().map(|node| (node.num, node)).collect());
        Ok(())
    }

    /// One connection to the event stream, until it drops.
    async fn session(self) -> Failure {
        let mut events = match Events::open().await {
            Ok(events) => events,
            Err(failure) => return failure,
        };
        // After ready, not before: nothing is lost in between.
        if let Err(failure) = self.refresh().await {
            return failure;
        }
        self.daemon.set(Daemon::Connected);

        while let Some(message) = events.next().await {
            match message {
                ServerMessage::Link { link } => self.status.update(|status| {
                    if let Some(status) = status {
                        status.link = link;
                    }
                }),
                ServerMessage::Node { node } => self.nodes.update(|nodes| {
                    nodes.insert(node.num, node);
                }),
                ServerMessage::Synced | ServerMessage::Lagged => {
                    if let Err(failure) = self.refresh().await {
                        return failure;
                    }
                }
                ServerMessage::Ready | ServerMessage::Error { .. } => {}
            }
        }
        Failure::Unreachable
    }
}

/// Follows the daemon for as long as the page is open.
fn follow(live: Live) {
    spawn_local(async move {
        loop {
            match live.session().await {
                Failure::Unauthorized => {
                    live.daemon.set(Daemon::NeedsToken);
                    // Until a token is given.
                    while live.daemon.get_untracked() == Daemon::NeedsToken {
                        sleep(Duration::from_millis(300)).await;
                    }
                }
                Failure::Unreachable => {
                    live.daemon.set(Daemon::Unreachable);
                    sleep(RETRY).await;
                }
            }
            live.daemon.set(Daemon::Connecting);
        }
    });
    spawn_local(async move {
        loop {
            sleep(CLOCK_TICK).await;
            live.now.set(now());
        }
    });
}

fn now() -> u32 {
    // Milliseconds as an f64, which holds them exactly for a long while yet.
    (js_sys::Date::now() / 1000.0) as u32
}

#[component]
pub fn App() -> impl IntoView {
    view! {
        <I18nContextProvider>
            <Shell />
        </I18nContextProvider>
    }
}

#[component]
fn Shell() -> impl IntoView {
    let live = Live::new();
    follow(live);

    view! {
        <header>
            <h1>"meshctl"</h1>
            <Connection live />
            <LanguageSwitch />
        </header>
        <main>
            <Show
                when=move || live.daemon.get() != Daemon::NeedsToken
                fallback=move || view! { <TokenForm live /> }
            >
                <LocalCard live />
                <NodeList live />
            </Show>
        </main>
    }
}

#[component]
fn LanguageSwitch() -> impl IntoView {
    let i18n = use_i18n();
    let switch = move |_| {
        let next = match i18n.get_locale() {
            Locale::en => Locale::es,
            Locale::es => Locale::en,
        };
        i18n.set_locale(next);
    };
    view! {
        <button class="language" on:click=switch>
            {t!(i18n, other_language)}
        </button>
    }
}

/// The badge that says how things are: the daemon first, the radio after.
#[component]
fn Connection(live: Live) -> impl IntoView {
    let i18n = use_i18n();
    let state = move || {
        let link = live.status.get().map(|s| s.link);
        match (live.daemon.get(), link) {
            (Daemon::Connected, Some(LinkState::Ready)) => "ready",
            (Daemon::Connected, Some(LinkState::Syncing)) => "syncing",
            (Daemon::Connected, Some(LinkState::Connecting)) => "connecting",
            (Daemon::Connected, Some(LinkState::Disconnected) | None) => "radio-down",
            (Daemon::Unreachable, _) => "daemon-down",
            (Daemon::Connecting | Daemon::NeedsToken, _) => "connecting",
        }
    };
    let label = move || match state() {
        "ready" => t!(i18n, link_ready).into_any(),
        "syncing" => t!(i18n, link_syncing).into_any(),
        "radio-down" => t!(i18n, link_radio_down).into_any(),
        "daemon-down" => t!(i18n, link_daemon_down).into_any(),
        _ => t!(i18n, link_connecting).into_any(),
    };
    view! { <span class=move || format!("badge {}", state())>{label}</span> }
}

#[component]
fn TokenForm(live: Live) -> impl IntoView {
    let i18n = use_i18n();
    let value = RwSignal::new(String::new());
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        daemon::set_token(value.get_untracked().trim());
        live.daemon.set(Daemon::Connecting);
    };
    view! {
        <form class="token card" on:submit=submit>
            <p>{t!(i18n, token_needed)}</p>
            <input
                type="password"
                autocomplete="current-password"
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
            <button type="submit">{t!(i18n, token_use)}</button>
        </form>
    }
}

/// The radio meshctl is connected to.
#[component]
fn LocalCard(live: Live) -> impl IntoView {
    let i18n = use_i18n();
    let local = move || live.status.get().and_then(|s| s.local);
    let me = move || {
        let num = local()?.num;
        live.nodes.with(|nodes| nodes.get(&num).cloned())
    };
    let name = move || {
        me().and_then(|n| n.long_name)
            .or_else(|| local().map(|l| l.id))
            .unwrap_or_default()
    };

    view! {
        <Show when=move || local().is_some()>
            <section class="card local">
                <h2>{name}</h2>
                <dl>
                    <dt>{t!(i18n, local_id)}</dt>
                    <dd class="mono">{move || local().map(|l| l.id)}</dd>
                    <dt>{t!(i18n, local_hardware)}</dt>
                    <dd>{move || local().and_then(|l| l.hw_model)}</dd>
                    <dt>{t!(i18n, local_firmware)}</dt>
                    <dd class="mono">{move || local().and_then(|l| l.firmware_version)}</dd>
                    <dt>{t!(i18n, local_role)}</dt>
                    <dd>{move || local().and_then(|l| l.role)}</dd>
                    <dt>{t!(i18n, local_radio)}</dt>
                    <dd>
                        {move || {
                            local()
                                .map(|l| {
                                    [l.region, l.modem_preset]
                                        .into_iter()
                                        .flatten()
                                        .collect::<Vec<_>>()
                                        .join(" · ")
                                })
                        }}
                    </dd>
                </dl>
            </section>
        </Show>
    }
}

#[component]
fn NodeList(live: Live) -> impl IntoView {
    let i18n = use_i18n();
    let filter = RwSignal::new(String::new());

    let shown = move || {
        let needle = filter.get().to_lowercase();
        let mut nodes: Vec<Node> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter(|node| needle.is_empty() || matches(node, &needle))
                .cloned()
                .collect()
        });
        nodes.sort_by(|a, b| b.last_heard.cmp(&a.last_heard).then(a.num.cmp(&b.num)));
        nodes
    };
    let total = move || live.nodes.with(BTreeMap::len);

    view! {
        <section class="card nodes">
            <div class="toolbar">
                <h2>{t!(i18n, nodes_title)} " " <span class="count">{total}</span></h2>
                <input
                    type="search"
                    placeholder=move || t_string!(i18n, search)
                    prop:value=move || filter.get()
                    on:input=move |ev| filter.set(event_target_value(&ev))
                />
            </div>
            <div class="scroll">
                <table>
                    <thead>
                        <tr>
                            <th>{t!(i18n, nodes_name)}</th>
                            <th>{t!(i18n, nodes_id)}</th>
                            <th class="num">{t!(i18n, nodes_heard)}</th>
                            <th class="num">{t!(i18n, nodes_hops)}</th>
                            <th class="num">"SNR"</th>
                            <th class="num">{t!(i18n, nodes_battery)}</th>
                            <th>{t!(i18n, nodes_hardware)}</th>
                        </tr>
                    </thead>
                    <tbody>
                        <For each=shown key=|node| (node.num, node.last_heard) let:node>
                            <NodeRow node now=live.now />
                        </For>
                    </tbody>
                </table>
            </div>
        </section>
    }
}

fn matches(node: &Node, needle: &str) -> bool {
    [
        node.long_name.as_deref(),
        node.short_name.as_deref(),
        Some(&node.id),
    ]
    .into_iter()
    .flatten()
    .any(|text| text.to_lowercase().contains(needle))
}

#[component]
fn NodeRow(node: Node, now: RwSignal<u32>) -> impl IntoView {
    let heard = node.last_heard;
    let hops = if node.via_mqtt {
        "MQTT".to_owned()
    } else {
        node.hops_away.map(|h| h.to_string()).unwrap_or_default()
    };
    let snr = node
        .snr
        .filter(|_| node.hops_away == Some(0) && !node.via_mqtt)
        .map(|s| format!("{s:.1} dB"))
        .unwrap_or_default();
    let battery = match node.battery_level {
        Some(level) if level > 100 => "⚡".to_owned(),
        Some(level) => format!("{level} %"),
        None => String::new(),
    };

    view! {
        <tr>
            <td>
                <span class="short">{node.short_name.clone()}</span>
                " "
                {node.long_name.clone()}
            </td>
            <td class="mono">{node.id.clone()}</td>
            <td class="num">{move || heard.map(|t| ago(now.get(), t)).unwrap_or_default()}</td>
            <td class="num">{hops}</td>
            <td class="num">{snr}</td>
            <td class="num">{battery}</td>
            <td class="dim">{node.hw_model.clone()}</td>
        </tr>
    }
}

/// How long ago, in the largest unit that fits. The units are the same in
/// every language the interface speaks.
fn ago(now: u32, then: u32) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3_600 => format!("{} min", seconds / 60),
        3_600..86_400 => format!("{} h", seconds / 3_600),
        _ => format!("{} d", seconds / 86_400),
    }
}
