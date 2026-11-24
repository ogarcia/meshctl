// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A phone's way around: a bar of tabs at the foot in place of the two bars
//! aside, the selected radio in the heading to switch it from a sheet, and
//! the pages those bars stand for on a wider screen. All of it shows only on
//! a narrow screen, as the style sheet decides.

use crate::app::{Conversations, LinkLabel, Live, LocalFoot, Page};
use crate::badge::{self, Face};
use crate::i18n::{t, t_string, use_i18n};
use crate::radios::Editing;
use leptos::prelude::*;

const MAP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/map.svg");
const ANTENNA: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/antenna.svg");
const MESSAGE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/message.svg");
const ACTIVITY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/activity.svg");
const DOTS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/dots.svg");

/// The tab a page is under.
fn tab_of(page: Page) -> Page {
    match page {
        Page::Map => Page::Map,
        Page::Nodes | Page::Node => Page::Nodes,
        Page::Chats | Page::Chat => Page::Chats,
        Page::Packets => Page::Packets,
        Page::More | Page::RadioSettings | Page::Preferences | Page::Radios | Page::Data => {
            Page::More
        }
    }
}

/// The tab the page shown is in: the radio's own page, in More, as its
/// card there opens it, not in the nodes, which leave it out.
fn current_tab(live: Live) -> Page {
    let page = live.page.get();
    let own = live
        .status
        .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
    if page == Page::Node && own.is_some() && live.node.get() == own {
        Page::More
    } else {
        tab_of(page)
    }
}

/// Whether a page is a tab's own, which has the radio in its heading, or one
/// within it, which has the way back.
pub fn is_tab(page: Page) -> bool {
    tab_of(page) == page
}

/// Where a page within a tab goes back to, on a phone; a node's page has its
/// own way back.
pub fn back_of(page: Page) -> Option<Page> {
    match page {
        Page::Chat => Some(Page::Chats),
        Page::RadioSettings | Page::Preferences | Page::Radios | Page::Data => Some(Page::More),
        _ => None,
    }
}

/// The bar of tabs at the foot.
#[component]
pub fn TabBar() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let tab = move |page: Page, icon: &'static str| {
        let label = move || match page {
            Page::Map => t_string!(i18n, page_map),
            Page::Nodes => t_string!(i18n, nodes_title),
            Page::Chats => t_string!(i18n, page_chats),
            Page::Packets => t_string!(i18n, page_packets),
            _ => t_string!(i18n, page_more),
        };
        let on = move || current_tab(live) == page;
        view! {
            <button
                type="button"
                class:on=on
                aria-current=move || on().then_some("page")
                on:click=move |_| live.go(page)
            >
                <span class="tab-icon" inner_html=icon></span>
                <span class="label">{label}</span>
                {(page == Page::Chats).then_some(move || {
                    // All that was not read yet, in every conversation.
                    let unread: u32 = live.conversations.with(|c| c.iter().map(|c| c.unread).sum());
                    (unread > 0).then(|| view! { <span class="unread">{crate::app::unread_count(unread)}</span> })
                })}
            </button>
        }
    };
    view! {
        <nav class="tabbar" aria-label=move || t_string!(i18n, nav_menu)>
            {tab(Page::Map, MAP)}
            {tab(Page::Nodes, ANTENNA)}
            {tab(Page::Chats, MESSAGE)}
            {tab(Page::Packets, ACTIVITY)}
            {tab(Page::More, DOTS)}
        </nav>
    }
}

/// The selected radio, in a tab's heading: its button, its name and how it
/// is; touching it opens the sheet to switch it.
#[component]
pub fn RadioChip() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || {
        live.selected_radio().map(|radio| {
            let link = radio.link;
            view! {
                <button
                    type="button"
                    class="radio-chip"
                    title=move || t_string!(i18n, nav_switch_radio)
                    on:click=move |_| live.radio_sheet.set(true)
                >
                    <span class="radio-face" class:colored=radio.badge.color.is_some() style=badge::style(&radio.badge)>
                        <Face name=radio.name.clone() badge=radio.badge.clone() short=radio.short_name.clone() />
                        <span class=format!("dot {}", live.link_class(link))></span>
                    </span>
                    <span class="name">{radio.name.clone()}</span>
                    <svg class="caret" viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M6 9l6 6l6 -6"></path>
                    </svg>
                </button>
            }
        })
    }
}

/// The way back from a page within a tab, on a phone.
#[component]
pub fn PhoneBack() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // On a phone, to the tab's own page; on a wider screen, from the
    // settings, to the last page out of them, where one came from.
    let button = move |class: &'static str, to: Page| {
        view! {
            <button
                class=class
                title=move || t_string!(i18n, back)
                aria-label=move || t_string!(i18n, back)
                on:click=move |_| live.go_back_to(to)
            >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M5 12h14M5 12l6 6M5 12l6 -6"></path>
                </svg>
            </button>
        }
    };
    move || {
        back_of(live.page.get()).map(|to| {
            let wide =
                (to == Page::More).then(|| button("icon-button wide-back", live.came_from.get()));
            view! {
                {button("icon-button phone-back", to)}
                {wide}
            }
        })
    }
}

/// The radios, to switch the selected one: a sheet from the foot.
#[component]
pub fn RadioSheet() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = live.radio_sheet;
    let radios = move || {
        live.radios
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.shown)
            .collect::<Vec<_>>()
    };
    let title = Signal::derive(move || t_string!(i18n, radios_title).to_owned());
    let foot = ViewFn::from(move || {
        view! {
            <button
                class="link"
                on:click=move |_| {
                    open.set(false);
                    live.radio_editing.set(Editing::Closed);
                    live.go(Page::Radios);
                }
            >
                {t!(i18n, nav_manage_radios)}
            </button>
        }
    });
    view! {
        <crate::node::Drawer open title foot>
            <ul class="radio-list">
                <For
                    each=radios
                    key=|radio| (radio.id, radio.name.clone(), radio.badge.clone(), radio.link)
                    let:radio
                >
                    {
                        let id = radio.id;
                        let link = Signal::derive(move || radio.link);
                        let selected = move || live.selected.get() == Some(id);
                        view! {
                            <li>
                                <button
                                    class:on=selected
                                    on:click=move |_| {
                                        live.select(id);
                                        open.set(false);
                                    }
                                >
                                    <span
                                        class="radio-face"
                                        class:colored=radio.badge.color.is_some()
                                        style=badge::style(&radio.badge)
                                    >
                                        <Face
                                            name=radio.name.clone()
                                            badge=radio.badge.clone()
                                            short=radio.short_name.clone()
                                        />
                                        <span class=move || format!("dot {}", live.link_class(link.get()))></span>
                                    </span>
                                    <span class="who">
                                        <b>{radio.name.clone()}</b>
                                        <span class="dim">
                                            <LinkLabel link />
                                        </span>
                                    </span>
                                    {move || {
                                        let unread = live.unread(id);
                                        (unread > 0).then(|| view! {
                                            <span class="unread">{crate::app::unread_count(unread)}</span>
                                        })
                                    }}
                                    <Show when=selected>
                                        <svg class="check" viewBox="0 0 24 24" aria-hidden="true">
                                            <path d="M5 12l5 5l10 -10"></path>
                                        </svg>
                                    </Show>
                                </button>
                            </li>
                        }
                    }
                </For>
            </ul>
        </crate::node::Drawer>
    }
}

/// The selected radio's conversations, in their sections, as a page.
#[component]
pub fn ChatsView() -> impl IntoView {
    view! {
        <div class="phone-list">
            <Conversations />
        </div>
    }
}

/// The rest: the radio's own node and its settings, the radios, and how the
/// interface looks.
#[component]
pub fn MoreView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let item = move |page: Page| {
        let label = move || match page {
            Page::RadioSettings => t!(i18n, nav_radio_settings).into_any(),
            Page::Radios => t!(i18n, radios_title).into_any(),
            Page::Data => t!(i18n, data_title).into_any(),
            _ => t!(i18n, prefs_title).into_any(),
        };
        view! {
            <button
                class="item"
                on:click=move |_| {
                    if page == Page::Radios {
                        live.radio_editing.set(Editing::Closed);
                    }
                    live.go(page);
                }
            >
                <span class="label">{label}</span>
                <svg class="chevron" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M9 6l6 6l-6 6"></path>
                </svg>
            </button>
        }
    };
    view! {
        <div class="phone-list more">
            <Show when=move || live.selected.get().is_some()>
                <LocalFoot />
                {item(Page::RadioSettings)}
            </Show>
            <h2>"meshctl"</h2>
            {item(Page::Radios)}
            {item(Page::Data)}
            {item(Page::Preferences)}
        </div>
    }
}
