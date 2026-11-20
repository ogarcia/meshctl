// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The interface's frame, and what keeps it current: the connection to the
//! daemon, the radios, and what the selected one knows.

use crate::daemon::{self, Events, Failure};
use crate::i18n::{I18nContextProvider, Locale, t, use_i18n};
use crate::mesh::MeshView;
use crate::radios::{RadioForm, RadiosView};
use gloo_timers::future::sleep;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::events::ServerMessage;
use meshctl_api::{LinkState, Node, Radio, RadioId, Status};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long to wait before trying the daemon again.
const RETRY: Duration = Duration::from_secs(3);

/// How often "heard 5 min ago" is worked out again.
const CLOCK_TICK: Duration = Duration::from_secs(15);

/// How the connection to the daemon is doing, beyond the radios' own links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Daemon {
    Connecting,
    Connected,
    Unreachable,
    NeedsToken,
}

/// Which page is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Mesh,
    Radios,
}

/// Everything the interface shows, kept current by [`follow`].
#[derive(Clone, Copy)]
pub struct Live {
    pub daemon: RwSignal<Daemon>,
    pub page: RwSignal<Page>,
    /// `None` until the daemon has said which radios there are.
    pub radios: RwSignal<Option<Vec<Radio>>>,
    pub selected: RwSignal<Option<RadioId>>,
    /// Of the selected radio.
    pub status: RwSignal<Option<Status>>,
    /// Of the selected radio.
    pub nodes: RwSignal<BTreeMap<u32, Node>>,
    /// Seconds since the epoch, ticking.
    pub now: RwSignal<u32>,
}

impl Live {
    fn new() -> Self {
        Self {
            daemon: RwSignal::new(Daemon::Connecting),
            page: RwSignal::new(Page::Mesh),
            radios: RwSignal::new(None),
            selected: RwSignal::new(daemon::last_radio()),
            status: RwSignal::new(None),
            nodes: RwSignal::new(BTreeMap::new()),
            now: RwSignal::new(now()),
        }
    }

    pub fn select(self, radio: RadioId) {
        daemon::set_last_radio(radio);
        self.selected.set(Some(radio));
        self.page.set(Page::Mesh);
        spawn_local(async move {
            let _ = self.refresh_selected().await;
        });
    }

    /// Reads the radios again, and keeps a selection that still exists.
    pub async fn refresh_radios(self) -> Result<(), Failure> {
        let radios = daemon::radios().await?;
        let selected = self.selected.get_untracked();
        if !selected.is_some_and(|id| radios.iter().any(|r| r.id == id)) {
            let first = radios.first().map(|r| r.id);
            self.selected.set(first);
        }
        self.radios.set(Some(radios));
        Ok(())
    }

    /// Reads what the selected radio knows again.
    pub async fn refresh_selected(self) -> Result<(), Failure> {
        let Some(radio) = self.selected.get_untracked() else {
            self.status.set(None);
            self.nodes.set(BTreeMap::new());
            return Ok(());
        };
        let status = daemon::status(radio).await?;
        let nodes = daemon::nodes(radio).await?;
        // The selection may have moved on while we were asking.
        if self.selected.get_untracked() == Some(radio) {
            self.status.set(Some(status));
            self.nodes
                .set(nodes.into_iter().map(|node| (node.num, node)).collect());
        }
        Ok(())
    }

    pub async fn refresh(self) -> Result<(), Failure> {
        self.refresh_radios().await?;
        self.refresh_selected().await
    }

    fn is_selected(self, radio: RadioId) -> bool {
        self.selected.get_untracked() == Some(radio)
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
            let refreshed = match message {
                ServerMessage::Link { radio, link } => {
                    self.radios.update(|radios| {
                        if let Some(r) = radios.iter_mut().flatten().find(|r| r.id == radio) {
                            r.link = link;
                        }
                    });
                    if self.is_selected(radio) {
                        self.status.update(|status| {
                            if let Some(status) = status {
                                status.link = link;
                            }
                        });
                    }
                    Ok(())
                }
                ServerMessage::Node { radio, node } => {
                    if self.is_selected(radio) {
                        self.nodes.update(|nodes| {
                            nodes.insert(node.num, node);
                        });
                    }
                    Ok(())
                }
                ServerMessage::Synced { radio } if self.is_selected(radio) => {
                    self.refresh_selected().await
                }
                ServerMessage::Radios | ServerMessage::Lagged => self.refresh().await,
                _ => Ok(()),
            };
            if let Err(failure) = refreshed {
                return failure;
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
                Failure::Unreachable | Failure::Rejected(_) => {
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
    let i18n = use_i18n();
    let live = Live::new();
    provide_context(live);
    follow(live);

    let has_radios = move || {
        live.radios
            .with(|r| r.as_ref().is_some_and(|r| !r.is_empty()))
    };
    let toggle_page = move |_| {
        live.page.update(|page| {
            *page = match page {
                Page::Mesh => Page::Radios,
                Page::Radios => Page::Mesh,
            };
        });
    };

    view! {
        <header>
            <h1>"meshctl"</h1>
            <Show when=has_radios>
                <RadioPicker />
                <Connection />
                <button on:click=toggle_page>
                    {move || match live.page.get() {
                        Page::Mesh => t!(i18n, radios_manage).into_any(),
                        Page::Radios => t!(i18n, radios_back).into_any(),
                    }}
                </button>
            </Show>
            <LanguageSwitch />
        </header>
        <main>
            {move || match (live.daemon.get(), live.radios.with(|r| r.as_ref().map(Vec::len))) {
                (Daemon::NeedsToken, _) => view! { <TokenForm /> }.into_any(),
                (Daemon::Unreachable, None) => view! {
                    <p class="card dim">{t!(i18n, link_daemon_down)}</p>
                }.into_any(),
                (_, None) => ().into_any(),
                (_, Some(0)) => view! { <Welcome /> }.into_any(),
                (_, Some(_)) => match live.page.get() {
                    Page::Mesh => view! { <MeshView /> }.into_any(),
                    Page::Radios => view! { <RadiosView /> }.into_any(),
                },
            }}
        </main>
    }
}

/// What a daemon with no radios yet shows.
#[component]
fn Welcome() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <section class="card welcome">
            <h2>{t!(i18n, welcome_title)}</h2>
            <p>{t!(i18n, welcome_text)}</p>
            <RadioForm editing=RwSignal::new(None) />
        </section>
    }
}

#[component]
fn RadioPicker() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let options = move || live.radios.get().unwrap_or_default();
    let change = move |ev| {
        if let Ok(id) = event_target_value(&ev).parse() {
            live.select(id);
        }
    };
    view! {
        <select class="picker" on:change=change>
            <For
                each=options
                key=|radio| (radio.id, radio.name.clone(), radio.enabled)
                let:radio
            >
                <option
                    value=radio.id.to_string()
                    selected=move || live.selected.get() == Some(radio.id)
                >
                    {radio.name.clone()}
                    {(!radio.enabled).then(|| view! { " (" {t!(i18n, link_disabled)} ")" })}
                </option>
            </For>
        </select>
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

/// The badge class for a radio's link.
pub fn link_class(link: LinkState) -> &'static str {
    match link {
        LinkState::Ready => "ready",
        LinkState::Syncing => "syncing",
        LinkState::Connecting => "connecting",
        LinkState::Disconnected => "radio-down",
        LinkState::Disabled => "disabled",
    }
}

/// A radio's link, in words.
#[component]
pub fn LinkLabel(#[prop(into)] link: Signal<LinkState>) -> impl IntoView {
    let i18n = use_i18n();
    move || match link.get() {
        LinkState::Ready => t!(i18n, link_ready).into_any(),
        LinkState::Syncing => t!(i18n, link_syncing).into_any(),
        LinkState::Connecting => t!(i18n, link_connecting).into_any(),
        LinkState::Disconnected => t!(i18n, link_radio_down).into_any(),
        LinkState::Disabled => t!(i18n, link_disabled).into_any(),
    }
}

/// The badge that says how things are: the daemon first, the selected radio
/// after.
#[component]
fn Connection() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let link = Signal::derive(move || {
        live.status
            .get()
            .map_or(LinkState::Connecting, |status| status.link)
    });
    view! {
        {move || match live.daemon.get() {
            Daemon::Connected => view! {
                <span class=move || format!("badge {}", link_class(link.get()))>
                    <LinkLabel link />
                </span>
            }.into_any(),
            Daemon::Unreachable => view! {
                <span class="badge daemon-down">{t!(i18n, link_daemon_down)}</span>
            }.into_any(),
            _ => view! {
                <span class="badge connecting">{t!(i18n, link_connecting)}</span>
            }.into_any(),
        }}
    }
}

#[component]
fn TokenForm() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
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
