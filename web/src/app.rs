// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The interface's frame, and what keeps it current: the connection to the
//! daemon, the radios, and what the selected one knows.
//!
//! The frame is two bars on the left and the page on the right. The first bar
//! holds the radios, one button each, and meshctl's own settings at its foot.
//! The second holds what there is to see of the selected radio: its pages, its
//! channels and its direct conversations, and the radio itself at its foot.
//! Over the page, a bar says what is showing and what can be done with it.

use crate::chat::{ChatView, peer_of, short_name, title};
use crate::daemon::{self, Events, Failure};
use crate::i18n::{I18nContextProvider, Locale, t, t_string, use_i18n};
use crate::map::{MapView, placed};
use crate::mesh::{MeshView, ago, model};
use crate::node::NodeView;
use crate::packets::{KEPT, PacketsView};
use crate::preferences::PreferencesView;
use crate::radios::{RadioForm, RadiosView};
use crate::settings::SettingsView;
use gloo_timers::future::sleep;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::events::ServerMessage;
use meshctl_api::{
    Accent, Channel, Conversation, Language, LinkState, MapTheme, Message, NewRequest, Node,
    Packet, Peer, Preferences, Radio, RadioId, RadioKind, Request, RequestKind, Status, Theme,
    Traceroute,
};
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
    /// The selected radio's nodes where they are: the front page.
    Map,
    /// The selected radio's nodes.
    Nodes,
    /// What the selected radio hears, as it hears it.
    Packets,
    /// The open conversation.
    Chat,
    /// The open node.
    Node,
    /// The selected radio's own settings.
    RadioSettings,
    /// meshctl's: how the interface looks.
    Preferences,
    /// meshctl's: the radios it connects to.
    Radios,
}

impl Page {
    /// Whether it is one of meshctl's own settings, not a radio's.
    fn is_meshctl(self) -> bool {
        matches!(self, Self::Preferences | Self::Radios)
    }
}

/// Everything the interface shows, kept current by [`follow`].
#[derive(Clone, Copy)]
pub struct Live {
    pub daemon: RwSignal<Daemon>,
    pub page: RwSignal<Page>,
    /// Whether the bars are open over the page, on a narrow screen.
    pub menu: RwSignal<bool>,
    pub preferences: RwSignal<Preferences>,
    /// `None` until the daemon has said which radios there are.
    pub radios: RwSignal<Option<Vec<Radio>>>,
    pub selected: RwSignal<Option<RadioId>>,
    /// Of the selected radio.
    pub status: RwSignal<Option<Status>>,
    /// Of the selected radio.
    pub nodes: RwSignal<BTreeMap<u32, Node>>,
    /// Of the selected radio.
    pub channels: RwSignal<Vec<Channel>>,
    /// Of the selected radio.
    pub conversations: RwSignal<Vec<Conversation>>,
    /// The conversation open in the chat, if any.
    pub open: RwSignal<Option<Peer>>,
    /// The messages of the open conversation, oldest first.
    pub thread: RwSignal<Vec<Message>>,
    /// Of the selected radio, the newest first.
    pub traceroutes: RwSignal<Vec<Traceroute>>,
    /// What the selected radio heard, the newest first.
    pub packets: RwSignal<Vec<Packet>>,
    /// The node whose page is open, if any.
    pub node: RwSignal<Option<u32>>,
    /// The node picked on the map, if any.
    pub map_pick: RwSignal<Option<u32>>,
    /// The node whose last traced route the map shows, if any.
    pub map_route: RwSignal<Option<u32>>,
    /// What the open node was asked for, the newest first.
    pub requests: RwSignal<Vec<Request>>,
    /// Why the last request or traceroute was not sent.
    pub ask_error: RwSignal<Option<Failure>>,
    /// Bumped each time the selected radio syncs, for what is read only then.
    pub synced: RwSignal<u32>,
    /// Seconds since the epoch, ticking.
    pub now: RwSignal<u32>,
}

impl Live {
    fn new() -> Self {
        Self {
            daemon: RwSignal::new(Daemon::Connecting),
            page: RwSignal::new(Page::Map),
            menu: RwSignal::new(false),
            preferences: RwSignal::new(Preferences::default()),
            radios: RwSignal::new(None),
            selected: RwSignal::new(daemon::last_radio()),
            status: RwSignal::new(None),
            nodes: RwSignal::new(BTreeMap::new()),
            channels: RwSignal::new(Vec::new()),
            conversations: RwSignal::new(Vec::new()),
            open: RwSignal::new(None),
            thread: RwSignal::new(Vec::new()),
            traceroutes: RwSignal::new(Vec::new()),
            packets: RwSignal::new(Vec::new()),
            node: RwSignal::new(None),
            map_pick: RwSignal::new(None),
            map_route: RwSignal::new(None),
            requests: RwSignal::new(Vec::new()),
            ask_error: RwSignal::new(None),
            synced: RwSignal::new(0),
            now: RwSignal::new(now()),
        }
    }

    /// Shows a page, and closes the bars over it on a narrow screen.
    pub fn go(self, page: Page) {
        self.page.set(page);
        self.menu.set(false);
    }

    pub fn select(self, radio: RadioId) {
        daemon::set_last_radio(radio);
        self.selected.set(Some(radio));
        self.open.set(None);
        self.thread.set(Vec::new());
        self.node.set(None);
        self.requests.set(Vec::new());
        self.map_pick.set(None);
        self.map_route.set(None);
        if matches!(self.page.get_untracked(), Page::Chat | Page::Node)
            || self.page.get_untracked().is_meshctl()
        {
            self.page.set(Page::Map);
        }
        self.menu.set(false);
        spawn_local(async move {
            let _ = self.refresh_selected().await;
        });
    }

    /// Opens a conversation in the chat.
    pub fn open_chat(self, peer: Peer) {
        self.open.set(Some(peer));
        self.thread.set(Vec::new());
        self.go(Page::Chat);
        spawn_local(async move {
            let _ = self.refresh_thread().await;
        });
    }

    /// Changes the preferences here at once, and in the daemon for every
    /// other browser.
    pub fn prefer(self, preferences: Preferences) {
        self.preferences.set(preferences);
        spawn_local(async move {
            if let Ok(kept) = daemon::save_preferences(&preferences).await {
                self.preferences.set(kept);
            }
        });
    }

    /// Reads the open conversation's messages again.
    pub async fn refresh_thread(self) -> Result<(), Failure> {
        let (Some(radio), Some(peer)) = (self.selected.get_untracked(), self.open.get_untracked())
        else {
            return Ok(());
        };
        let messages = daemon::messages(radio, peer).await?;
        if self.selected.get_untracked() == Some(radio) && self.open.get_untracked() == Some(peer) {
            self.thread.set(messages);
        }
        Ok(())
    }

    async fn refresh_conversations(self) -> Result<(), Failure> {
        let Some(radio) = self.selected.get_untracked() else {
            return Ok(());
        };
        let conversations = daemon::conversations(radio).await?;
        if self.selected.get_untracked() == Some(radio) {
            self.conversations.set(conversations);
        }
        Ok(())
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
            self.channels.set(Vec::new());
            self.conversations.set(Vec::new());
            return Ok(());
        };
        let status = daemon::status(radio).await?;
        let nodes = daemon::nodes(radio).await?;
        let channels = daemon::channels(radio).await?;
        let conversations = daemon::conversations(radio).await?;
        let traceroutes = daemon::traceroutes(radio).await?;
        let packets = daemon::packets(radio, None).await?;
        // The selection may have moved on while we were asking.
        if self.selected.get_untracked() == Some(radio) {
            self.status.set(Some(status));
            self.nodes
                .set(nodes.into_iter().map(|node| (node.num, node)).collect());
            self.channels.set(channels);
            self.conversations.set(conversations);
            self.traceroutes.set(traceroutes);
            self.packets.set(packets);
        }
        self.refresh_thread().await
    }

    /// Opens a node's page.
    pub fn open_node(self, node: u32) {
        self.node.set(Some(node));
        self.requests.set(Vec::new());
        self.ask_error.set(None);
        self.go(Page::Node);
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(requests) = daemon::requests(radio, node).await
                && self.node.get_untracked() == Some(node)
                && self.selected.get_untracked() == Some(radio)
            {
                self.requests.set(requests);
            }
        });
    }

    /// Asks a node for something, on a channel of the selected radio.
    pub fn ask(self, node: u32, kind: RequestKind, channel: u32) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        self.ask_error.set(None);
        spawn_local(async move {
            let new = NewRequest {
                node,
                kind,
                channel,
            };
            match daemon::ask(radio, &new).await {
                Ok(request) => self.keep_request(request),
                Err(failure) => self.ask_error.set(Some(failure)),
            }
        });
    }

    /// Puts a request of the open node in its list, or where it was.
    fn keep_request(self, request: Request) {
        if self.node.get_untracked() != Some(request.node) {
            return;
        }
        self.requests.update(
            |requests| match requests.iter_mut().find(|r| r.id == request.id) {
                Some(kept) => *kept = request,
                None => requests.insert(0, request),
            },
        );
    }

    /// Traces the route to a node from the selected radio, on a channel.
    pub fn trace(self, node: u32, channel: u32) {
        let Some(radio) = self.selected.get_untracked() else {
            return;
        };
        self.ask_error.set(None);
        spawn_local(async move {
            match daemon::trace(radio, node, channel).await {
                Ok(traceroute) => self.keep_traceroute(traceroute),
                Err(failure) => self.ask_error.set(Some(failure)),
            }
        });
    }

    /// Puts a traceroute in the list, or where it was, as it is now.
    fn keep_traceroute(self, traceroute: Traceroute) {
        self.traceroutes.update(|traceroutes| {
            match traceroutes.iter_mut().find(|t| t.id == traceroute.id) {
                Some(kept) => *kept = traceroute,
                None => traceroutes.insert(0, traceroute),
            }
        });
    }

    pub async fn refresh(self) -> Result<(), Failure> {
        self.preferences.set(daemon::preferences().await?);
        self.refresh_radios().await?;
        self.refresh_selected().await
    }

    /// The selected radio, as the daemon last said.
    pub fn selected_radio(self) -> Option<Radio> {
        let id = self.selected.get()?;
        self.radios
            .with(|radios| radios.as_ref()?.iter().find(|r| r.id == id).cloned())
    }

    /// Which mesh the selected radio speaks.
    pub fn selected_kind(self) -> Option<RadioKind> {
        let id = self.selected.get()?;
        self.radios
            .with(|radios| radios.as_ref()?.iter().find(|r| r.id == id).map(|r| r.kind))
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
                    self.synced.update(|n| *n += 1);
                    self.refresh_selected().await
                }
                ServerMessage::Message { radio, message } if self.is_selected(radio) => {
                    if self.open.get_untracked() == Some(peer_of(&message)) {
                        self.thread.update(|thread| {
                            if !thread.iter().any(|m| m.id == message.id) {
                                thread.push(message);
                            }
                        });
                    }
                    self.refresh_conversations().await
                }
                ServerMessage::MessageStatus {
                    radio,
                    id,
                    status,
                    error,
                } if self.is_selected(radio) => {
                    self.thread.update(|thread| {
                        if let Some(message) = thread.iter_mut().find(|m| m.id == id) {
                            message.status = status;
                            message.error = error;
                        }
                    });
                    Ok(())
                }
                ServerMessage::Traceroute { radio, traceroute } if self.is_selected(radio) => {
                    self.keep_traceroute(traceroute);
                    Ok(())
                }
                ServerMessage::Request { radio, request } if self.is_selected(radio) => {
                    self.keep_request(request);
                    Ok(())
                }
                ServerMessage::Packet { radio, packet } if self.is_selected(radio) => {
                    self.packets.update(|packets| {
                        packets.insert(0, packet);
                        packets.truncate(KEPT);
                    });
                    Ok(())
                }
                ServerMessage::Preferences { preferences } => {
                    self.preferences.set(preferences);
                    Ok(())
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

/// Puts the preferences on `<html>`, where the style sheet reads them, as the
/// daemon already did when it served the page.
fn show(preferences: Preferences) {
    let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    let set = |name: &str, value: Option<&str>| {
        let _ = match value {
            Some(value) => root.set_attribute(name, value),
            None => root.remove_attribute(name),
        };
    };
    set(
        "data-theme",
        match preferences.theme {
            Theme::System => None,
            Theme::Light => Some("light"),
            Theme::Dark => Some("dark"),
        },
    );
    set(
        "data-accent",
        Some(match preferences.accent {
            Accent::Amber => "amber",
        }),
    );
    set(
        "data-map-theme",
        match preferences.map_theme {
            MapTheme::Page => None,
            MapTheme::Light => Some("light"),
            MapTheme::Dark => Some("dark"),
        },
    );
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

    Effect::new(move |_| {
        let preferences = live.preferences.get();
        show(preferences);
        match preferences.language {
            Some(Language::En) => i18n.set_locale(Locale::en),
            Some(Language::Es) => i18n.set_locale(Locale::es),
            None => {}
        }
    });

    let class = move || {
        if live.menu.get() {
            "app menu-open"
        } else {
            "app"
        }
    };
    view! {
        // On a wide screen the map takes the whole page, without the bar over
        // it: what that would say, its nodes' cards say. The style sheet
        // decides, by the width.
        <div class=class class:on-map=move || live.page.get() == Page::Map>
            <Rail />
            <Sidebar />
            <div class="page">
                <Topbar />
                <main>
                    <Content />
                </main>
            </div>
        </div>
    }
}

/// What the page shows.
#[component]
fn Content() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || match (
        live.daemon.get(),
        live.radios.with(|r| r.as_ref().map(Vec::len)),
        live.page.get(),
    ) {
        (Daemon::NeedsToken, _, _) => view! { <TokenForm /> }.into_any(),
        (Daemon::Unreachable, None, _) => {
            view! { <p class="card dim">{t!(i18n, link_daemon_down)}</p> }.into_any()
        }
        (_, None, _) => ().into_any(),
        (_, _, Page::Preferences) => view! { <PreferencesView /> }.into_any(),
        (_, _, Page::Radios) => view! { <RadiosView /> }.into_any(),
        (_, Some(0), _) => view! { <Welcome /> }.into_any(),
        (_, Some(_), Page::Map) => view! { <MapView /> }.into_any(),
        (_, Some(_), Page::Nodes) => view! { <MeshView /> }.into_any(),
        (_, Some(_), Page::Chat) => view! { <ChatView /> }.into_any(),
        (_, Some(_), Page::Node) => view! { <NodeView /> }.into_any(),
        (_, Some(_), Page::Packets) => view! { <PacketsView /> }.into_any(),
        (_, Some(_), Page::RadioSettings) => view! { <SettingsView /> }.into_any(),
    }
}

/// The first bar: the radios, and meshctl's own settings.
#[component]
fn Rail() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let radios = move || live.radios.get().unwrap_or_default();
    let settings_class = move || {
        if live.page.get().is_meshctl() {
            "rail-button active"
        } else {
            "rail-button"
        }
    };
    view! {
        <nav class="rail" aria-label=move || t_string!(i18n, nav_radios)>
            <svg class="logo" viewBox="0 0 24 24" role="img" aria-label="meshctl">
                <circle cx="5" cy="12" r="2.5"></circle>
                <circle cx="19" cy="5" r="2.5"></circle>
                <circle cx="19" cy="19" r="2.5"></circle>
                <path d="M7.3 11 16.7 6M7.3 13l9.4 5"></path>
            </svg>
            <span class="rail-rule"></span>
            <For each=radios key=|radio| (radio.id, radio.name.clone()) let:radio>
                <RadioButton id=radio.id name=radio.name.clone() />
            </For>
            <button
                class="rail-button add"
                title=move || t_string!(i18n, radio_add_title)
                aria-label=move || t_string!(i18n, radio_add_title)
                on:click=move |_| live.go(Page::Radios)
            >
                "+"
            </button>
            <span class="rail-space"></span>
            <button
                class=settings_class
                title=move || t_string!(i18n, nav_meshctl_settings)
                aria-label=move || t_string!(i18n, nav_meshctl_settings)
                on:click=move |_| live.go(Page::Preferences)
            >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <circle cx="12" cy="12" r="3"></circle>
                    <path d="M12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2"></path>
                </svg>
            </button>
        </nav>
    }
}

/// Two letters for a radio: its words' first, or its first two.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let letters: String = match words.as_slice() {
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
        [only] => only.chars().take(2).collect(),
        [] => "?".into(),
    };
    let mut chars = letters.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[component]
fn RadioButton(id: RadioId, name: String) -> impl IntoView {
    let live = expect_context::<Live>();
    let link = move || {
        live.radios.with(|radios| {
            radios
                .iter()
                .flatten()
                .find(|r| r.id == id)
                .map_or(LinkState::Disconnected, |r| r.link)
        })
    };
    let class = move || {
        let active = live.selected.get() == Some(id) && !live.page.get().is_meshctl();
        if active {
            "rail-button radio active"
        } else {
            "rail-button radio"
        }
    };
    let short = initials(&name);
    view! {
        <button class=class title=name.clone() aria-label=name on:click=move |_| live.select(id)>
            {short}
            <span class=move || format!("dot {}", link_class(link()))></span>
        </button>
    }
}

/// The second bar: what there is of the selected radio, or of meshctl's own
/// settings.
#[component]
fn Sidebar() -> impl IntoView {
    let live = expect_context::<Live>();
    view! {
        <aside class="sidebar">
            {move || {
                if live.page.get().is_meshctl() {
                    view! { <MeshctlSidebar /> }.into_any()
                } else {
                    view! { <RadioSidebar /> }.into_any()
                }
            }}
        </aside>
    }
}

#[component]
fn MeshctlSidebar() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let item = move |page: Page| {
        let class = move || {
            if live.page.get() == page {
                "item active"
            } else {
                "item"
            }
        };
        let label = move || match page {
            Page::Preferences => t!(i18n, prefs_title).into_any(),
            _ => t!(i18n, radios_title).into_any(),
        };
        view! { <button class=class on:click=move |_| live.go(page)>{label}</button> }
    };
    view! {
        <header class="sidebar-head">
            <h1>"meshctl"</h1>
            <span class="dim">{t!(i18n, nav_meshctl_settings)}</span>
        </header>
        <div class="sidebar-body">{item(Page::Preferences)} {item(Page::Radios)}</div>
    }
}

#[component]
fn RadioSidebar() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let page_item = move |page: Page| {
        let class = move || {
            if live.page.get() == page {
                "item active"
            } else {
                "item"
            }
        };
        let (label, count) = match page {
            Page::Map => (
                t!(i18n, page_map).into_any(),
                Some(Signal::derive(move || placed(live))),
            ),
            Page::Nodes => (
                t!(i18n, nodes_title).into_any(),
                Some(Signal::derive(move || live.nodes.with(BTreeMap::len))),
            ),
            _ => (t!(i18n, page_packets).into_any(), None),
        };
        view! {
            <button class=class on:click=move |_| live.go(page)>
                {label}
                {count.map(|count| view! { <span class="count">{count}</span> })}
            </button>
        }
    };
    let channels = move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| Peer::Channel { channel: c.index })
                .collect::<Vec<_>>()
        })
    };
    let directs = move || {
        live.conversations.with(|conversations| {
            conversations
                .iter()
                .map(|c| c.peer)
                .filter(|peer| matches!(peer, Peer::Node { .. }))
                .collect::<Vec<_>>()
        })
    };

    view! {
        <Show when=move || live.selected.get().is_some()>
            <RadioHead />
            <div class="sidebar-body">
                {page_item(Page::Map)}
                {page_item(Page::Nodes)}
                {page_item(Page::Packets)}
                <h2>{t!(i18n, nav_channels)}</h2>
                <For each=channels key=|peer| *peer let:peer>
                    <PeerItem peer />
                </For>
                <h2>{t!(i18n, nav_directs)}</h2>
                <For each=directs key=|peer| *peer let:peer>
                    <PeerItem peer />
                </For>
                <Show when=move || directs().is_empty()>
                    <p class="empty dim">{t!(i18n, nav_no_directs)}</p>
                </Show>
            </div>
            <LocalFoot />
        </Show>
    }
}

/// The selected radio's name, and how it is.
#[component]
fn RadioHead() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let radio = move || live.selected_radio();
    let link = Signal::derive(move || radio().map_or(LinkState::Connecting, |r| r.link));
    let kind = move || match radio().map(|r| r.kind) {
        Some(RadioKind::Meshtastic) => "Meshtastic",
        Some(RadioKind::Meshcore) => "MeshCore",
        None => "",
    };
    view! {
        <header class="sidebar-head">
            <h1>{move || radio().map(|r| r.name)}</h1>
            <span class="dim">
                {kind} " · " <LinkLabel link /> " · "
                {move || t!(i18n, nav_node_count, n = move || live.nodes.with(BTreeMap::len))}
            </span>
        </header>
    }
}

/// A channel or a node to talk to.
#[component]
fn PeerItem(peer: Peer) -> impl IntoView {
    let live = expect_context::<Live>();
    let class = move || {
        if live.page.get() == Page::Chat && live.open.get() == Some(peer) {
            "item active"
        } else {
            "item"
        }
    };
    let marker = match peer {
        Peer::Channel { .. } => view! { <span class="mark-channel">"#"</span> }.into_any(),
        Peer::Node { node } => {
            let hops = move || live.nodes.with(|n| n.get(&node).and_then(|n| n.hops_away));
            view! {
                <span class=move || match hops() {
                    Some(0) => "mark-node direct",
                    Some(1..=2) => "mark-node near",
                    Some(3..=4) => "mark-node mid",
                    _ => "mark-node far",
                }></span>
            }
            .into_any()
        }
    };
    let label = move || {
        let title = title(live, peer);
        // The sidebar marks channels already.
        title
            .strip_prefix("# ")
            .map_or_else(|| title.clone(), str::to_owned)
    };
    view! {
        <button class=class on:click=move |_| live.open_chat(peer)>
            {marker}
            <span class="label">{label}</span>
        </button>
    }
}

/// The selected radio's own node, and the way to its settings.
#[component]
fn LocalFoot() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let local = move || live.status.get().and_then(|s| s.local);
    let num = move || local().map(|l| l.num);
    let name = move || {
        let num = num()?;
        live.nodes.with(|nodes| nodes.get(&num)?.long_name.clone())
    };
    let short = move || num().map(|num| short_name(live, num)).unwrap_or_default();
    let detail = move || {
        local().map(|l| match l.hw_model.as_deref().map(model) {
            Some(hw) => format!("{} · {hw}", l.id),
            None => l.id,
        })
    };
    let class = move || {
        if live.page.get() == Page::RadioSettings {
            "settings active"
        } else {
            "settings"
        }
    };
    view! {
        <footer class="local-foot">
            <span class="avatar">{short}</span>
            <span class="who">
                <b>{move || name().unwrap_or_else(|| t_string!(i18n, nav_my_node).to_owned())}</b>
                <span class="mono dim">{detail}</span>
            </span>
            <button
                class=class
                title=move || t_string!(i18n, nav_radio_settings)
                aria-label=move || t_string!(i18n, nav_radio_settings)
                on:click=move |_| live.go(Page::RadioSettings)
            >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12"></path>
                    <circle cx="16" cy="6" r="2"></circle>
                    <circle cx="10" cy="12" r="2"></circle>
                    <circle cx="18" cy="18" r="2"></circle>
                </svg>
            </button>
        </footer>
    }
}

/// Over the page: what is showing, and what can be done with it.
#[component]
fn Topbar() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let heading = move || match live.page.get() {
        Page::Nodes => view! { <h1>{t!(i18n, nodes_title)}</h1> }.into_any(),
        Page::Map => match live.map_pick.get() {
            Some(node) => view! { <PeerHeading peer=Peer::Node { node } /> }.into_any(),
            None => view! { <h1>{t!(i18n, page_map)}</h1> }.into_any(),
        },
        Page::Packets => view! { <h1>{t!(i18n, page_packets)}</h1> }.into_any(),
        Page::Node => match live.node.get() {
            Some(node) => view! { <PeerHeading peer=Peer::Node { node } /> }.into_any(),
            None => ().into_any(),
        },
        Page::RadioSettings => view! {
            <h1>{t!(i18n, nav_radio_settings)}</h1>
        }
        .into_any(),
        Page::Preferences => view! { <h1>{t!(i18n, prefs_title)}</h1> }.into_any(),
        Page::Radios => view! { <h1>{t!(i18n, radios_title)}</h1> }.into_any(),
        Page::Chat => match live.open.get() {
            Some(peer) => view! { <PeerHeading peer /> }.into_any(),
            None => view! { <h1>{t!(i18n, page_chat)}</h1> }.into_any(),
        },
    };
    view! {
        <header class="topbar">
            <button
                class="menu-toggle"
                aria-label=move || t_string!(i18n, nav_menu)
                on:click=move |_| live.menu.update(|open| *open = !*open)
            >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M4 6h16M4 12h16M4 18h16"></path>
                </svg>
            </button>
            {heading}
        </header>
    }
}

/// The open conversation's heading, and what can be done with whom it is with.
#[component]
fn PeerHeading(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    match peer {
        Peer::Channel { channel } => view! {
            <div class="heading">
                <h1>{move || title(live, peer)}</h1>
                <span class="dim">{t!(i18n, nav_channel_slot, index = channel)}</span>
            </div>
        }
        .into_any(),
        Peer::Node { node } => {
            let known = move || live.nodes.with(|nodes| nodes.get(&node).cloned());
            let detail = move || {
                let node = known()?;
                let mut parts = vec![node.id.clone()];
                match node.hops_away {
                    _ if node.via_mqtt => parts.push("MQTT".into()),
                    Some(0) => {
                        if let Some(snr) = node.snr {
                            parts.push(format!("SNR {snr:.1}"));
                        }
                    }
                    Some(hops) => parts.push(format!("{hops} ⇢")),
                    None => {}
                }
                if let Some(heard) = node.last_heard {
                    parts.push(ago(live.now.get(), heard));
                }
                Some(parts.join(" · "))
            };
            // Where else there is to go with the node: from the chat, its page;
            // from its page, the chat. On the map its card has both already.
            let action = move || {
                let page = live.page.get();
                let chat = (page == Page::Node).then(|| {
                    view! {
                        <button class="primary" on:click=move |_| live.open_chat(peer)>
                            {t!(i18n, node_message)}
                        </button>
                    }
                });
                let profile = (page == Page::Chat).then(|| {
                    view! {
                        <button on:click=move |_| live.open_node(node)>{t!(i18n, node_page)}</button>
                    }
                });
                view! { {profile} {chat} }
            };
            view! {
                <span class="avatar">{move || short_name(live, node)}</span>
                <div class="heading">
                    <h1>{move || title(live, peer)}</h1>
                    <span class="mono dim">{detail}</span>
                </div>
                <div class="actions">{action}</div>
            }
            .into_any()
        }
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

/// The class for a radio's link.
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

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn radios_go_by_two_letters() {
        assert_eq!(initials("casa"), "Ca");
        assert_eq!(initials("Casa de campo"), "Cd");
        assert_eq!(initials("Ñu"), "Ñu");
        assert_eq!(initials(""), "?");
    }
}
