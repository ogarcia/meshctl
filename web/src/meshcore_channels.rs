// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A MeshCore radio's channels, in its settings: those it has, by their
//! kind, each edited on its own over the list and saved to the radio
//! straight away; and shared, by its link, which carries its secret.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{ErrorCode, MeshcoreChannel, MeshcoreChannelChange, MeshcoreChannelKind};

const HASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/hash.svg");
const LOCK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/lock.svg");
const LOCK_OPEN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/lock-open.svg");
const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const GRIP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/grip-vertical.svg");
const UP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-up.svg");
const DOWN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");

/// Which channel is being edited: one there is, or a new one, by its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Editing {
    Closed,
    Slot(u32),
    New(u32),
}

/// What becomes of a channel's secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Secret {
    /// The one it has.
    Keep,
    /// A new one, at random.
    New,
    /// One written.
    Own,
}

/// The kind of a channel, in words.
fn kind_label(
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    kind: MeshcoreChannelKind,
) -> &'static str {
    match kind {
        MeshcoreChannelKind::Public => t_string!(i18n, mc_channel_public),
        MeshcoreChannelKind::Hashtag => t_string!(i18n, mc_channel_hashtag),
        MeshcoreChannelKind::Private => t_string!(i18n, mc_channel_private),
    }
}

/// Whether anyone could read it: `Public` all have, a `#name` all who know it.
fn open(kind: MeshcoreChannelKind) -> bool {
    kind != MeshcoreChannelKind::Private
}

/// The secret a channel's link carries, in hex.
fn secret_of(link: &str) -> Option<String> {
    let query = link.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("secret="))
        .map(str::to_owned)
}

#[component]
pub fn MeshcoreChannelsTab() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let loaded = RwSignal::new(None::<meshctl_api::MeshcoreChannels>);
    let editing = RwSignal::new(Editing::Closed);
    let reload = move || {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(read) = daemon::meshcore_channels(radio).await
                && live.selected.get_untracked() == Some(radio)
            {
                loaded.set(Some(read));
            }
        });
    };
    // Read on opening, and each time the radio is read again: after every
    // change above all.
    Effect::new(move |_| {
        live.synced.track();
        live.selected.track();
        reload();
    });
    // Those it has, in their slots' order, each with its place and how many
    // there are.
    let channels = move || {
        loaded.with(|l| {
            let have = l.as_ref().map(|l| l.channels.clone()).unwrap_or_default();
            let count = have.len();
            have.into_iter()
                .enumerate()
                .map(|(at, c)| (at, count, c))
                .collect::<Vec<_>>()
        })
    };
    // The row being dragged, by its place.
    let dragging = RwSignal::new(None::<usize>);
    let problem = RwSignal::new(None::<Failure>);
    // Moves the channel at one place of the list to another: shown so at
    // once, sent to the radio, and read back from it.
    let reorder = move |from: usize, to: usize| {
        let (Some(radio), Some(now)) = (live.selected.get_untracked(), loaded.get_untracked())
        else {
            return;
        };
        let mut order: Vec<u32> = now.channels.iter().map(|c| c.index).collect();
        if from == to || from >= order.len() || to >= order.len() {
            return;
        }
        let moved = order.remove(from);
        order.insert(to, moved);
        loaded.update(|l| {
            if let Some(l) = l {
                let mut channels = l.channels.clone();
                let moved = channels.remove(from);
                channels.insert(to, moved);
                for (slot, channel) in (0..).zip(channels.iter_mut()) {
                    channel.index = slot;
                }
                l.channels = channels;
            }
        });
        problem.set(None);
        spawn_local(async move {
            if let Err(failure) = daemon::reorder_meshcore_channels(radio, &order).await {
                problem.set(Some(failure));
                reload();
            }
        });
    };
    // The first slot free, for a new one.
    let free = move || {
        loaded.with(|l| {
            let l = l.as_ref()?;
            (0..l.slots).find(|slot| l.channels.iter().all(|c| c.index != *slot))
        })
    };

    view! {
        <section class="channel-list">
            <For
                each=channels
                key=|(at, count, c)| format!("{at} {count} {} {} {:?}", c.index, c.name, c.kind)
                let:((at, count, channel))
            >
                <ChannelRow channel at count dragging editing reorder />
            </For>
            {move || problem.get().map(|_| view! { <p class="error">{t!(i18n, mc_reorder_failed)}</p> })}
            <div class="channel-foot">
                <p class="hint dim">{t!(i18n, mc_channels_hint)}</p>
                <crate::links::AddChannelsLink />
                <button
                    type="button"
                    class="primary add"
                    disabled=move || free().is_none()
                    on:click=move |_| {
                        if let Some(slot) = free() {
                            editing.set(Editing::New(slot));
                        }
                    }
                >
                    <span class="icon" inner_html=PLUS></span>
                    {t!(i18n, channel_add)}
                </button>
            </div>
        </section>
        {move || {
            let (index, adding) = match editing.get() {
                Editing::Closed => return ().into_any(),
                Editing::Slot(slot) => (slot, false),
                Editing::New(slot) => (slot, true),
            };
            let start = loaded.with_untracked(|l| {
                l.as_ref()?.channels.iter().find(|c| c.index == index).cloned()
            });
            view! { <ChannelEditor index adding start editing reload /> }.into_any()
        }}
    }
}

#[component]
fn ChannelRow(
    channel: MeshcoreChannel,
    at: usize,
    count: usize,
    dragging: RwSignal<Option<usize>>,
    editing: RwSignal<Editing>,
    reorder: impl Fn(usize, usize) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let slot = channel.index;
    let kind = channel.kind;
    let over = RwSignal::new(false);
    view! {
        <div
            class="radio-row channel-row"
            class:dragging=move || dragging.get() == Some(at)
            class:over=move || over.get()
            draggable="true"
            on:dragstart=move |ev| {
                if let Some(data) = ev.data_transfer() {
                    data.set_effect_allowed("move");
                    // Firefox starts no drag without data.
                    let _ = data.set_data("text/plain", &at.to_string());
                }
                dragging.set(Some(at));
            }
            on:dragover=move |ev| {
                ev.prevent_default();
                over.set(dragging.get_untracked().is_some_and(|from| from != at));
            }
            on:dragleave=move |_| over.set(false)
            on:drop=move |ev| {
                ev.prevent_default();
                over.set(false);
                if let Some(from) = dragging.get_untracked() {
                    reorder(from, at);
                }
                dragging.set(None);
            }
            on:dragend=move |_| dragging.set(None)
        >
            <span class="grip" title=move || t_string!(i18n, channel_drag) inner_html=GRIP></span>
            <span class="moves">
                <button
                    type="button"
                    disabled={at == 0}
                    title=move || t_string!(i18n, radio_up)
                    aria-label=move || t_string!(i18n, radio_up)
                    on:click=move |_| reorder(at, at.saturating_sub(1))
                    inner_html=UP
                ></button>
                <button
                    type="button"
                    disabled={at + 1 >= count}
                    title=move || t_string!(i18n, radio_down)
                    aria-label=move || t_string!(i18n, radio_down)
                    on:click=move |_| reorder(at, at + 1)
                    inner_html=DOWN
                ></button>
            </span>
            <button type="button" class="pick" on:click=move |_| editing.set(Editing::Slot(slot))>
                <span class="channel-mark" inner_html=HASH></span>
                <span class="who">
                    <span class="name"><b>{channel.name}</b></span>
                    <span class="dim mono">{move || t!(i18n, channel_slot, slot = slot)}</span>
                </span>
                <span class="channel-facts">
                    <span class="key" class:open=open(kind)>
                        <span class="icon" inner_html=if open(kind) { LOCK_OPEN } else { LOCK }></span>
                        {move || kind_label(i18n, kind)}
                    </span>
                </span>
            </button>
        </div>
    }
}

#[component]
fn ChannelEditor(
    // `slot` is a word of the view macro's own; called index outside it.
    index: u32,
    adding: bool,
    start: Option<MeshcoreChannel>,
    editing: RwSignal<Editing>,
    reload: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let slot = index;
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let start_name = start.as_ref().map(|c| c.name.clone()).unwrap_or_default();
    let start_kind = start.as_ref().map(|c| c.kind);
    // A #name is kept without its mark, as the apps keep it: written with
    // it, so that it stays one as it is saved.
    let written = match start_kind {
        Some(MeshcoreChannelKind::Hashtag) if !start_name.starts_with('#') => {
            format!("#{start_name}")
        }
        _ => start_name.clone(),
    };
    let name = RwSignal::new(written);
    // Its region as written, `#` and all; empty for the radio's.
    let region = RwSignal::new(
        start
            .as_ref()
            .and_then(|c| c.scope.clone())
            .map_or_else(String::new, |s| if s == "*" { s } else { format!("#{s}") }),
    );
    // The regions found nearby, to pick one, each with how many repeaters
    // serve it.
    let found = crate::meshcore_regions::FoundRegions::read();
    // A region to pick, as written in the field: none for the radio's,
    // `*` for none at all; with how many repeaters serve it, heard straight
    // and in all, dimmed when none heard straight does.
    let region_chip = move |value: Option<String>, text: Signal<String>| {
        let written = match value.as_deref() {
            None => String::new(),
            Some("*") => "*".to_owned(),
            Some(name) => format!("#{name}"),
        };
        // As the regions are found, which may be after it is drawn.
        let served = {
            let value = value.clone();
            Signal::derive(move || value.as_deref().and_then(|name| found.served(name)))
        };
        let far = {
            let value = value.clone();
            Signal::derive(move || value.as_deref().is_some_and(|name| found.unserved(name)))
        };
        let title = move || {
            served.get().map(|(direct, all)| {
                t_string!(i18n, regions_served)
                    .replace("{n}", &all.to_string())
                    .replace("{d}", &direct.to_string())
            })
        };
        let on = written.clone();
        view! {
            <button
                type="button"
                class="choice"
                class:on=move || region.with(|r| r.trim() == on)
                class:far=far
                title=title
                on:click=move |_| region.set(written.clone())
            >
                {text}
                {move || served.get().map(|(direct, all)| view! { <span class="served">{format!("{direct}/{all}")}</span> })}
            </button>
        }
    };
    // The region written, known to reach no repeater heard straight.
    let region_unserved = move || {
        let written = region.with(|r| r.trim().to_owned());
        match written.as_str() {
            "" => None,
            "*" => found
                .unserved("*")
                .then(|| t_string!(i18n, regions_unscoped_unserved).to_owned()),
            name => {
                let name = name.trim_start_matches('#');
                found
                    .unserved(name)
                    .then(|| t_string!(i18n, regions_unserved).replace("{name}", name))
            }
        }
    };
    let secret = RwSignal::new(if adding { Secret::New } else { Secret::Keep });
    let own = RwSignal::new(String::new());
    // Its link, to share it, and the secret it has.
    let link = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    let removing = RwSignal::new(false);
    let name_input = NodeRef::<leptos::html::Input>::new();
    // A `#name` has its secret from it.
    let hashtag = Memo::new(move |_| name.with(|n| n.trim().starts_with('#')));

    if !adding && let Some(radio) = live.selected.get_untracked() {
        spawn_local(async move {
            if let Ok(read) = daemon::meshcore_channel_link(radio, slot).await {
                link.set(Some(read.url));
            }
        });
    }
    let close = move || editing.set(Editing::Closed);
    // Escape closes it, as its ✕ and Cancel do; a click off it does not.
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            close();
        }
    });
    on_cleanup(move || escape.remove());
    if adding {
        Effect::new(move |_| {
            if let Some(input) = name_input.get() {
                let _ = input.focus();
            }
        });
    }

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let scope = region.with_untracked(|r| {
            let r = r.trim();
            (!r.is_empty()).then(|| r.trim_start_matches('#').to_owned())
        });
        let wanted = MeshcoreChannelChange {
            scope,
            name: name.get_untracked().trim().to_owned(),
            secret: if hashtag.get_untracked() {
                None
            } else {
                match secret.get_untracked() {
                    // Kept as it is, renamed or not.
                    Secret::Keep => link.with_untracked(|l| l.as_deref().and_then(secret_of)),
                    Secret::New => None,
                    Secret::Own => Some(own.get_untracked().trim().to_lowercase()),
                }
            },
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match daemon::set_meshcore_channel(radio, slot, &wanted).await {
                Ok(()) => {
                    reload();
                    close();
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };
    let remove = move |_| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match daemon::remove_meshcore_channel(radio, slot).await {
                Ok(()) => {
                    reload();
                    close();
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };

    let message = move || {
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::InvalidSettings) => {
                t!(i18n, mc_channel_invalid).into_any()
            }
            Failure::Rejected(ErrorCode::DuplicateChannel) => {
                t!(i18n, channel_duplicate).into_any()
            }
            Failure::Rejected(ErrorCode::RadioDidNotAnswer) => {
                t!(i18n, settings_no_answer).into_any()
            }
            Failure::Rejected(ErrorCode::RadioNotConnected) => {
                t!(i18n, settings_not_connected).into_any()
            }
            _ => t!(i18n, error_failed).into_any(),
        })
    };
    let title = {
        let shown = start_name.clone();
        move || {
            if adding {
                t!(i18n, channel_new).into_any()
            } else {
                shown.clone().into_any()
            }
        }
    };
    // What the secret is now, for the one there is.
    let kind_now = start_kind.map(|kind| {
        view! {
            <div class="key-now">
                <span class="key" class:open=open(kind)>
                    <span class="icon" inner_html=if open(kind) { LOCK_OPEN } else { LOCK }></span>
                    {move || kind_label(i18n, kind)}
                </span>
            </div>
        }
    });
    let share = move || {
        let url = link.get()?;
        let card = crate::node::link_card(url, move || t!(i18n, mc_channel_share_hint).into_any())?;
        Some(view! {
            <div class="field">
                <span class="dim">{t!(i18n, mc_channel_share)}</span>
                <div class="contact-share">{card}</div>
            </div>
        })
    };

    view! {
        <div class="modal-back">
            <form
                class="card radio-editor modal channel-editor"
                role="dialog"
                aria-modal="true"
                on:submit=submit
            >
                <header class="editor-head">
                    <span class="channel-mark big" inner_html=HASH></span>
                    <span class="who">
                        <h2>{title}</h2>
                        <span class="mono dim">{move || t!(i18n, channel_slot, slot = slot)}</span>
                    </span>
                    <button
                        type="button"
                        class="close"
                        title=move || t_string!(i18n, radio_cancel)
                        aria-label=move || t_string!(i18n, radio_cancel)
                        on:click=move |_| close()
                        inner_html=CLOSE
                    ></button>
                </header>
                <div class="modal-body">
                    <div class="fields">
                        <label>
                            <span class="dim">{t!(i18n, channel_name)}</span>
                            <input
                                node_ref=name_input
                                maxlength="31"
                                required
                                placeholder="#galicia"
                                prop:value=move || name.get()
                                on:input=move |ev| name.set(event_target_value(&ev))
                            />
                            <span class="hint dim">{t!(i18n, mc_channel_name_hint)}</span>
                        </label>
                        <div class="field">
                            <span class="dim">{t!(i18n, channel_key)}</span>
                            {kind_now}
                            <Show
                                when=move || !hashtag.get()
                                fallback=move || view! { <span class="hint dim">{t!(i18n, mc_secret_hashtag)}</span> }
                            >
                                <select
                                    prop:value=move || match secret.get() {
                                        Secret::Keep => "keep",
                                        Secret::New => "new",
                                        Secret::Own => "own",
                                    }
                                    on:change=move |ev| {
                                        secret.set(match event_target_value(&ev).as_str() {
                                            "keep" => Secret::Keep,
                                            "own" => Secret::Own,
                                            _ => Secret::New,
                                        });
                                    }
                                >
                                    {(!adding).then(|| view! { <option value="keep">{t!(i18n, channel_key_keep)}</option> })}
                                    {adding.then(|| view! { <option value="new">{t!(i18n, channel_key_new)}</option> })}
                                    <option value="own">{t!(i18n, channel_key_custom)}</option>
                                </select>
                                <input
                                    class="mono"
                                    class:gone=move || secret.get() != Secret::Own
                                    required=move || secret.get() == Secret::Own && !hashtag.get()
                                    maxlength="32"
                                    pattern="[0-9a-fA-F]{32}"
                                    placeholder="8b3387e9c5cdea6ac9e5edbaa115cd72"
                                    prop:value=move || own.get()
                                    on:input=move |ev| own.set(event_target_value(&ev))
                                />
                                <span class="hint dim" class:gone=move || secret.get() != Secret::Own>
                                    {t!(i18n, mc_secret_own_hint)}
                                </span>
                            </Show>
                        </div>
                        <div class="field">
                            <span class="dim">{t!(i18n, mc_channel_scope)}</span>
                            <input
                                maxlength="31"
                                placeholder=move || t_string!(i18n, mc_channel_scope_default)
                                prop:value=move || region.get()
                                on:input=move |ev| region.set(event_target_value(&ev))
                            />
                            <div class="flag-chips channel-regions">
                                {region_chip(None, Signal::derive(move || t_string!(i18n, mc_channel_scope_default).to_owned()))}
                                {region_chip(Some("*".to_owned()), Signal::derive(move || t_string!(i18n, chat_scope_none).to_owned()))}
                                {move || {
                                    found
                                        .names()
                                        .into_iter()
                                        .map(|name| {
                                            let shown = format!("#{name}");
                                            region_chip(Some(name), Signal::derive(move || shown.clone()))
                                        })
                                        .collect_view()
                                }}
                                <button
                                    type="button"
                                    class="choice region-search"
                                    disabled=move || found.searching()
                                    on:click=move |_| found.search()
                                >
                                    {move || found.button_words(i18n)}
                                </button>
                            </div>
                            {move || {
                                region_unserved().map(|text| {
                                    view! {
                                        <p class="region-warning">
                                            {text}
                                        </p>
                                    }
                                })
                            }}
                            <span class="hint dim">{t!(i18n, mc_channel_scope_hint)}</span>
                        </div>
                        {share}
                    </div>
                </div>
                <p class="error">{message}</p>
                <div class="removing" class:gone=move || !removing.get()>
                    <p>{t!(i18n, mc_channel_remove_question)}</p>
                    <div class="buttons">
                        <button type="button" on:click=move |_| removing.set(false)>
                            {t!(i18n, radio_cancel)}
                        </button>
                        <button type="button" class="danger" disabled=move || busy.get() on:click=remove>
                            {t!(i18n, channel_remove)}
                        </button>
                    </div>
                </div>
                <div class="buttons" class:gone=move || removing.get()>
                    {(!adding).then(|| view! {
                        <button type="button" class="danger" on:click=move |_| removing.set(true)>
                            {t!(i18n, channel_remove)}
                        </button>
                    })}
                    <button type="button" on:click=move |_| close()>
                        {t!(i18n, radio_cancel)}
                    </button>
                    <button type="submit" class="primary" disabled=move || busy.get()>
                        {move || {
                            if adding {
                                t!(i18n, channel_add).into_any()
                            } else {
                                t!(i18n, settings_save).into_any()
                            }
                        }}
                    </button>
                </div>
            </form>
        </div>
    }
}
