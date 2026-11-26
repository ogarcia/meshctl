// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's channels, in its settings: those it has, each edited on its
//! own over the list and saved to the radio straight away. A channel's key
//! is asked for only to be shown, and forgotten with its editor.

use crate::app::Live;
use crate::chat::preset_name;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::{precision_radius, radius};
use crate::radios::Switch;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    ChannelChange, ChannelHistory, ChannelKey, ChannelSecret, ChannelSettings, ErrorCode,
    KeyChange, NodeSettings, NodeSettingsChange, PUBLIC_POSITION_PRECISION,
};
use wasm_bindgen::JsCast;

const HASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/hash.svg");
const LOCK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/lock.svg");
const LOCK_OPEN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/lock-open.svg");
const COPY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/copy.svg");
const CHECK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/check.svg");
const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");
const EYE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye.svg");
const EYE_OFF: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye-off.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const GRIP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/grip-vertical.svg");
const UP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-up.svg");
const DOWN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
const PIN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/map-pin.svg");
const MUTED: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/bell-off.svg");

/// The precision a channel starts sharing the position with, as the apps
/// start it: some 2.9 km off, which a public channel takes too.
const FIRST_PRECISION: u32 = 13;

/// The default key's first fifteen bytes in base64: a key of one's own that
/// starts so is the default one, as the radio tells it.
const DEFAULT_KEY_START: &str = "1PG7OiApB1nwvP+rz05p";

/// How precisely a channel shares the position, in words: exactly, or
/// within so many metres; None when it does not.
fn precision_label(
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    bits: u32,
) -> Option<String> {
    match bits {
        0 => None,
        32 => Some(t_string!(i18n, channel_precision_exact).to_owned()),
        bits => Some(precision_radius(bits).map_or_else(String::new, radius)),
    }
}

/// What a save or a removal came to, as the settings page tells it.
pub type Notice = RwSignal<Option<Result<bool, Failure>>>;

/// Which channel is being edited: one there is, or a new one, by its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Editing {
    Closed,
    Slot(u32),
    New(u32),
}

/// What the radio names an unnamed channel, in any slot: its modem
/// preset, or Custom without one.
fn preset_of(settings: &NodeSettings) -> String {
    if settings.lora.use_preset {
        preset_name(&settings.lora.modem_preset)
    } else {
        "Custom".to_owned()
    }
}

/// A channel's name, or, for an unnamed one, the one the radio gives it.
pub(crate) fn shown_name(settings: &NodeSettings, channel: &ChannelSettings) -> String {
    if channel.name.is_empty() {
        preset_of(settings)
    } else {
        channel.name.clone()
    }
}

/// Whether a key keeps what is sent private: the default one is known to all.
fn private(key: ChannelKey) -> bool {
    matches!(key, ChannelKey::Aes128 | ChannelKey::Aes256)
}

#[component]
pub fn ChannelsTab(loaded: RwSignal<Option<NodeSettings>>, notice: Notice) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let editing = RwSignal::new(Editing::Closed);
    // Those it has, in their slots' order, with how many there are.
    let channels = move || {
        loaded.with(|l| {
            l.as_ref()
                .map(|l| {
                    let have: Vec<_> = l.channels.iter().filter(|c| c.role != "DISABLED").collect();
                    let count = have.len();
                    have.into_iter()
                        .enumerate()
                        .map(|(at, c)| (at, count, shown_name(l, c), c.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    };
    // The rows as one is dragged to another place.
    let sortable = crate::sortable::Sortable::new();
    // Moves the channel at one place of the list to another: shown so at
    // once, sent to the radio, and read back from it.
    let reorder = move |from: usize, to: usize| {
        let (Some(radio), Some(settings)) = (live.selected.get_untracked(), loaded.get_untracked())
        else {
            return;
        };
        let mut order: Vec<u32> = settings
            .channels
            .iter()
            .filter(|c| c.role != "DISABLED")
            .map(|c| c.index)
            .collect();
        if from == to || from >= order.len() || to >= order.len() {
            return;
        }
        let moved = order.remove(from);
        order.insert(to, moved);
        // Another primary: what that means, before.
        if order[0] != 0 {
            let name = shown_name(&settings, &settings.channels[order[0] as usize]);
            let sure = web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(&format!(
                        "«{name}» {}",
                        t_string!(i18n, channel_primary_confirm)
                    ))
                    .ok()
                })
                .unwrap_or(false);
            if !sure {
                return;
            }
        }
        loaded.update(|l| {
            if let Some(l) = l {
                let before = l.channels.clone();
                for (at, channel) in l.channels.iter_mut().enumerate() {
                    *channel = match order.get(at) {
                        Some(&from) => ChannelSettings {
                            index: at as u32,
                            role: if at == 0 { "PRIMARY" } else { "SECONDARY" }.to_owned(),
                            ..before[from as usize].clone()
                        },
                        None => ChannelSettings {
                            index: at as u32,
                            role: "DISABLED".to_owned(),
                            name: String::new(),
                            key: ChannelKey::None,
                            uplink: false,
                            downlink: false,
                            position_precision: 0,
                            public: true,
                            muted: false,
                        },
                    };
                }
            }
        });
        spawn_local(async move {
            let applied = daemon::reorder_channels(radio, &order).await;
            notice.set(Some(applied.map(|a| a.reboots)));
        });
    };
    // The first slot free, for a new one.
    let free = move || {
        loaded.with(|l| {
            l.as_ref()?
                .channels
                .iter()
                .find(|c| c.index > 0 && c.role == "DISABLED")
                .map(|c| c.index)
        })
    };

    view! {
        <section
            class="channel-list"
            class:sorting=move || sortable.sorting()
            on:dragover=move |ev| sortable.over(&ev)
            on:drop=move |ev| sortable.drop(&ev, reorder)
        >
            <For
                each=channels
                key=|(at, count, name, c)| format!("{at} {count} {name} {c:?}")
                let:((at, count, name, channel))
            >
                <ChannelRow name channel at count sortable editing reorder />
            </For>
            <div class="channel-foot">
                <p class="hint dim">{t!(i18n, channels_hint)}</p>
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
        {move || match editing.get() {
            Editing::Closed => ().into_any(),
            Editing::Slot(slot) => {
                view! { <ChannelEditor index=slot adding=false loaded editing notice /> }.into_any()
            }
            Editing::New(slot) => {
                view! { <ChannelEditor index=slot adding=true loaded editing notice /> }.into_any()
            }
        }}
    }
}

/// What kind of key a channel has, in words.
/// A channel's key as its list says it: its padlock, shut for a key that
/// keeps it private, open for none or the default, and its word.
pub(crate) fn key_view(
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    key: ChannelKey,
) -> impl IntoView {
    view! {
        <span class="key" class:open=!private(key)>
            <span class="icon" inner_html=if private(key) { LOCK } else { LOCK_OPEN }></span>
            {move || key_label(i18n, key)}
        </span>
    }
}

fn key_label(i18n: leptos_i18n::I18nContext<crate::i18n::Locale>, key: ChannelKey) -> String {
    match key {
        ChannelKey::None => t_string!(i18n, key_none).to_owned(),
        ChannelKey::Default => t_string!(i18n, key_default).to_owned(),
        ChannelKey::Simple => t_string!(i18n, key_simple).to_owned(),
        ChannelKey::Aes128 => "AES-128".to_owned(),
        ChannelKey::Aes256 => "AES-256".to_owned(),
    }
}

#[component]
fn ChannelRow(
    name: String,
    channel: ChannelSettings,
    at: usize,
    count: usize,
    sortable: crate::sortable::Sortable,
    editing: RwSignal<Editing>,
    reorder: impl Fn(usize, usize) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let slot = channel.index;
    let key = channel.key;
    view! {
        <div
            class="radio-row channel-row"
            class:dragging=move || sortable.dragged(at)
            style=move || sortable.style(at)
            draggable="true"
            on:dragstart=move |ev| sortable.start(&ev, at)
            on:dragend=move |_| sortable.end()
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
                <span class="name">
                    <b>{name}</b>
                    {(slot == 0).then(|| view! { <span class="tag">{t!(i18n, channel_primary)}</span> })}
                </span>
                <span class="dim mono">{move || t!(i18n, channel_slot, slot = slot)}</span>
            </span>
            <span class="channel-facts">
                {channel.uplink.then(|| view! { <span class="tag">"↑ MQTT"</span> })}
                {channel.downlink.then(|| view! { <span class="tag">"↓ MQTT"</span> })}
                {channel
                    .muted
                    .then(|| {
                        view! {
                            <span class="tag position" title=move || t_string!(i18n, channel_muted)>
                                <span class="icon" inner_html=MUTED></span>
                            </span>
                        }
                    })}
                {
                    let bits = channel.position_precision;
                    (bits > 0)
                        .then(|| {
                            view! {
                                <span
                                    class="tag position"
                                    title=move || t_string!(i18n, channel_position)
                                >
                                    <span class="icon" inner_html=PIN></span>
                                    {move || precision_label(i18n, bits)}
                                </span>
                            }
                        })
                }
                {key_view(i18n, key)}
            </span>
            </button>
        </div>
    }
}

/// What to do with the key, as its select says it.
fn key_code(key: KeyChange) -> &'static str {
    match key {
        KeyChange::Keep => "keep",
        KeyChange::None => "none",
        KeyChange::Default => "default",
        KeyChange::NewAes128 => "new_aes128",
        KeyChange::NewAes256 => "new_aes256",
        KeyChange::Custom => "custom",
    }
}

fn key_of(code: &str) -> KeyChange {
    match code {
        "none" => KeyChange::None,
        "default" => KeyChange::Default,
        "new_aes128" => KeyChange::NewAes128,
        "new_aes256" => KeyChange::NewAes256,
        "custom" => KeyChange::Custom,
        _ => KeyChange::Keep,
    }
}

/// Puts a text on the clipboard: by the clipboard's own API where the page
/// may use it, which is only over HTTPS or on this machine; otherwise by
/// copying what the input holds selected.
fn copy(text: &str, input: Option<web_sys::HtmlInputElement>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    if window.is_secure_context() {
        let _ = window.navigator().clipboard().write_text(text);
        return;
    }
    if let Some(input) = input {
        input.select();
        if let Some(document) = window
            .document()
            .and_then(|d| d.dyn_into::<web_sys::HtmlDocument>().ok())
        {
            let _ = document.exec_command("copy");
        }
    }
}

#[component]
fn ChannelEditor(
    // `slot` is a word of the view macro's own; called index outside it.
    index: u32,
    adding: bool,
    loaded: RwSignal<Option<NodeSettings>>,
    editing: RwSignal<Editing>,
    notice: Notice,
) -> impl IntoView {
    let slot = index;
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let primary = slot == 0;
    let (start, preset) = loaded.with_untracked(|l| {
        let l = l.as_ref();
        (
            l.and_then(|l| l.channels.get(slot as usize).cloned()),
            l.map(preset_of).unwrap_or_default(),
        )
    });
    let start = start.unwrap_or(ChannelSettings {
        index: slot,
        role: "DISABLED".to_owned(),
        name: String::new(),
        key: ChannelKey::None,
        uplink: false,
        downlink: false,
        position_precision: 0,
        public: true,
        muted: false,
    });
    let current_key = start.key;
    let name = RwSignal::new(if adding {
        String::new()
    } else {
        start.name.clone()
    });
    let uplink = RwSignal::new(!adding && start.uplink);
    let downlink = RwSignal::new(!adding && start.downlink);
    let muted = RwSignal::new(!adding && start.muted);
    // A new one does not share the position until told to.
    let precision = RwSignal::new(if adding { 0 } else { start.position_precision });
    let start_precision = start.position_precision;
    let start_public = start.public;
    // A new one gets a key of its own, as the apps give it.
    let key = RwSignal::new(if adding {
        KeyChange::NewAes256
    } else {
        KeyChange::Keep
    });
    let custom = RwSignal::new(String::new());
    // Whether anyone could read it, with the key it is to have: as the radio
    // tells it, which for one of one's own is a guess the daemon checks.
    let public = Memo::new(move |_| match key.get() {
        KeyChange::Keep => start_public,
        KeyChange::None | KeyChange::Default => true,
        KeyChange::NewAes128 | KeyChange::NewAes256 => false,
        KeyChange::Custom => custom.with(|text| {
            let text = text.trim();
            text.len() <= 4 || text.starts_with(DEFAULT_KEY_START)
        }),
    });
    // On a public one the radio sends the position only roughly: what it
    // would cut down is not offered, and is brought down as it goes public.
    Effect::new(move |_| {
        if public.get() && precision.get_untracked() > PUBLIC_POSITION_PRECISION {
            precision.set(PUBLIC_POSITION_PRECISION);
        }
    });
    // The precisions offered, as the apps offer them, and the one it has if
    // that is another.
    // From the roughest to the most precise, along the slider.
    let precisions = Memo::new(move |_| {
        let most = if public.get() {
            PUBLIC_POSITION_PRECISION
        } else {
            19
        };
        let exact = (!public.get()).then_some(32);
        let mut offered: Vec<u32> = (10..=most).chain(exact).collect();
        if start_precision != 0
            && !offered.contains(&start_precision)
            && (start_precision <= most || !public.get())
        {
            offered.push(start_precision);
            offered.sort_unstable();
        }
        offered
    });
    // Where the precision is along it: the nearest offered, at or under it.
    let step = move || {
        let bits = precision.get();
        precisions.with(|offered| offered.iter().rposition(|&p| p <= bits).unwrap_or(0))
    }; // The key itself, while shown: nowhere else, and gone with the editor.
    let secret = RwSignal::new(None::<ChannelSecret>);
    let copied = RwSignal::new(false);
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    let key_input = NodeRef::<leptos::html::Input>::new();
    let name_input = NodeRef::<leptos::html::Input>::new();

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

    let reveal = move |_| {
        if secret.with_untracked(Option::is_some) {
            secret.set(None);
            copied.set(false);
            return;
        }
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match daemon::channel_secret(radio, slot).await {
                Ok(read) => secret.set(Some(read)),
                Err(failure) => error.set(Some(failure)),
            }
        });
    };
    let copy_key = move |_| {
        if let Some(shown) = secret.get_untracked() {
            copy(&shown.key, key_input.get_untracked());
            copied.set(true);
        }
    };

    // Sends one channel as it is to be, and closes once the radio has it.
    let send = move |channel: ChannelChange| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let change = NodeSettingsChange {
                channels: vec![channel],
                ..Default::default()
            };
            match daemon::change_settings(radio, &change).await {
                Ok(applied) => {
                    notice.set(Some(Ok(applied.reboots)));
                    editing.set(Editing::Closed);
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let key = key.get_untracked();
        send(ChannelChange {
            index: slot,
            role: if primary { "PRIMARY" } else { "SECONDARY" }.to_owned(),
            name: name.get_untracked().trim().to_owned(),
            key,
            custom_key: (key == KeyChange::Custom).then(|| custom.get_untracked()),
            uplink: uplink.get_untracked(),
            downlink: downlink.get_untracked(),
            position_precision: Some(precision.get_untracked()),
            muted: Some(muted.get_untracked()),
        });
    };
    // Taken off the radio, its key with it, and those after it moved up;
    // its messages kept aside or forgotten, as chosen.
    let removing = RwSignal::new(false);
    let remove = move |history: ChannelHistory| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match daemon::remove_channel(radio, slot, history).await {
                Ok(applied) => {
                    notice.set(Some(Ok(applied.reboots)));
                    editing.set(Editing::Closed);
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };

    let message = move || {
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::InvalidSettings) => t!(i18n, channel_invalid).into_any(),
            Failure::Rejected(ErrorCode::PublicChannelPrecision) => {
                t!(i18n, channel_public_precision).into_any()
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
        let shown = if start.name.is_empty() {
            preset.clone()
        } else {
            start.name.clone()
        };
        move || {
            if adding {
                t!(i18n, channel_new).into_any()
            } else {
                shown.clone().into_any()
            }
        }
    };
    let detail = move || {
        if primary {
            t!(i18n, channel_slot_primary).into_any()
        } else {
            t!(i18n, channel_slot, slot = slot).into_any()
        }
    };

    view! {
        <div class="modal-back">
            <form
                class="card radio-editor modal channel-editor"
                role="dialog"
                aria-modal="true"
                on:submit=submit
            >
                <div class="dialog-top">
                <header class="editor-head">
                    <span class="channel-mark big" inner_html=HASH></span>
                    <span class="who">
                        <h2>{title}</h2>
                        <span class="mono dim">{detail}</span>
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
                                maxlength="11"
                                placeholder=move || {
                                    format!("{}: {preset}", t_string!(i18n, channel_name_preset))
                                }
                                prop:value=move || name.get()
                                on:input=move |ev| name.set(event_target_value(&ev))
                            />
                        </label>
                        <div class="field">
                            <span class="dim">{t!(i18n, channel_key)}</span>
                            {(!adding)
                                .then(|| {
                                    view! {
                                        <div class="key-now">
                                            <span class="key" class:open=!private(current_key)>
                                                <span
                                                    class="icon"
                                                    inner_html=if private(current_key) { LOCK } else { LOCK_OPEN }
                                                ></span>
                                                {move || key_label(i18n, current_key)}
                                            </span>
                                            {(current_key != ChannelKey::None)
                                                .then(|| {
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class="icon-button"
                                                            class:on=move || secret.with(Option::is_some)
                                                            title=move || {
                                                                if secret.with(Option::is_some) {
                                                                    t_string!(i18n, channel_key_hide)
                                                                } else {
                                                                    t_string!(i18n, channel_key_show)
                                                                }
                                                            }
                                                            aria-label=move || t_string!(i18n, channel_key_show)
                                                            on:click=reveal
                                                            inner_html=move || {
                                                                if secret.with(Option::is_some) { EYE_OFF } else { EYE }
                                                            }
                                                        ></button>
                                                    }
                                                })}
                                        </div>
                                            {move || {
                                                secret
                                                    .get()
                                                    .map(|shown| {
                                                        view! {
                                                            <div class="key-shown">
                                                            <input
                                                                node_ref=key_input
                                                                class="mono key-text"
                                                                readonly
                                                                prop:value=shown.key
                                                                on:focus=move |ev| {
                                                                    let input: web_sys::HtmlInputElement = event_target(&ev);
                                                                    input.select();
                                                                }
                                                            />
                                                            <button
                                                                type="button"
                                                                class="icon-button"
                                                                title=move || t_string!(i18n, channel_key_copy)
                                                                aria-label=move || t_string!(i18n, channel_key_copy)
                                                                on:click=copy_key
                                                                inner_html=move || if copied.get() { CHECK } else { COPY }
                                                            ></button>
                                                            </div>
                                                        }
                                                    })
                                            }}
                                    }
                                })}
                            <select
                                prop:value=move || key_code(key.get())
                                on:change=move |ev| key.set(key_of(&event_target_value(&ev)))
                            >
                                {(!adding)
                                    .then(|| {
                                        view! { <option value="keep">{t!(i18n, channel_key_keep)}</option> }
                                    })}
                                <option value="new_aes256">{t!(i18n, channel_key_new)} " · AES-256"</option>
                                <option value="new_aes128">{t!(i18n, channel_key_new)} " · AES-128"</option>
                                <option value="custom">{t!(i18n, channel_key_custom)}</option>
                                <option value="default">{t!(i18n, channel_key_default)}</option>
                                <option value="none">{t!(i18n, key_none)}</option>
                            </select>
                            <input
                                class="mono"
                                class:gone=move || key.get() != KeyChange::Custom
                                required=move || key.get() == KeyChange::Custom
                                placeholder="1PG7OiApB1nwvP+rz05pAQ=="
                                prop:value=move || custom.get()
                                on:input=move |ev| custom.set(event_target_value(&ev))
                            />
                            <span class="hint dim">
                                {move || match key.get() {
                                    KeyChange::Custom => t!(i18n, channel_key_custom_hint).into_any(),
                                    KeyChange::Default => t!(i18n, channel_key_default_hint).into_any(),
                                    KeyChange::None => t!(i18n, channel_key_none_hint).into_any(),
                                    _ => ().into_any(),
                                }}
                            </span>
                        </div>
                        <div class="toggle">
                            <Switch
                                on=Signal::derive(move || uplink.get())
                                label=Signal::derive(move || t_string!(i18n, channel_uplink).to_owned())
                                flip=move |_| uplink.update(|on| *on = !*on)
                            />
                            <span>
                                {t!(i18n, channel_uplink)}
                                <span class="dim">{t!(i18n, channel_uplink_hint)}</span>
                            </span>
                        </div>
                        <div class="toggle">
                            <Switch
                                on=Signal::derive(move || downlink.get())
                                label=Signal::derive(move || t_string!(i18n, channel_downlink).to_owned())
                                flip=move |_| downlink.update(|on| *on = !*on)
                            />
                            <span>
                                {t!(i18n, channel_downlink)}
                                <span class="dim">{t!(i18n, channel_downlink_hint)}</span>
                            </span>
                        </div>
                        <div class="toggle">
                            <Switch
                                on=Signal::derive(move || precision.get() > 0)
                                label=Signal::derive(move || t_string!(i18n, channel_position).to_owned())
                                flip=move |_| precision.update(|p| *p = if *p == 0 { FIRST_PRECISION } else { 0 })
                            />
                            <span>
                                {t!(i18n, channel_position)}
                                <span class="dim">{t!(i18n, channel_position_hint)}</span>
                            </span>
                        </div>
                        <label class="precision" class:gone=move || precision.get() == 0>
                            <span class="dim">
                                {t!(i18n, channel_precision)}
                                <b>{move || precision_label(i18n, precision.get())}</b>
                            </span>
                            <input
                                type="range"
                                min="0"
                                max=move || precisions.with(|o| o.len().saturating_sub(1)).to_string()
                                step="1"
                                prop:value=move || step().to_string()
                                on:input=move |ev| {
                                    if let Ok(at) = event_target_value(&ev).parse::<usize>()
                                        && let Some(bits) = precisions.with_untracked(|o| o.get(at).copied())
                                    {
                                        precision.set(bits);
                                    }
                                }
                            />
                            <span class="ends dim">
                                <span>{move || precisions.with(|o| o.first().and_then(|&b| precision_label(i18n, b)))}</span>
                                <span>{move || precisions.with(|o| o.last().and_then(|&b| precision_label(i18n, b)))}</span>
                            </span>
                            <span class="hint dim" class:gone=move || !public.get()>
                                {t!(i18n, channel_public_hint)}
                            </span>
                        </label>
                        <div class="toggle">
                            <Switch
                                on=Signal::derive(move || muted.get())
                                label=Signal::derive(move || t_string!(i18n, channel_muted).to_owned())
                                flip=move |_| muted.update(|m| *m = !*m)
                            />
                            <span>
                                {t!(i18n, channel_muted)}
                                <span class="dim">{t!(i18n, channel_muted_hint)}</span>
                            </span>
                        </div>
                    </div>
                </div>
                <p class="error">{message}</p>
                </div>
                // Taking it off asks first what becomes of its messages.
                <div class="removing" class:gone=move || !removing.get()>
                    <p>{t!(i18n, channel_remove_question)}</p>
                    <div class="buttons">
                        <button type="button" on:click=move |_| removing.set(false)>
                            {t!(i18n, radio_cancel)}
                        </button>
                        <button
                            type="button"
                            class="danger"
                            disabled=move || busy.get()
                            on:click=move |_| remove(ChannelHistory::Forget)
                        >
                            {t!(i18n, channel_remove_forget)}
                        </button>
                        <button
                            type="button"
                            class="primary"
                            disabled=move || busy.get()
                            on:click=move |_| remove(ChannelHistory::Keep)
                        >
                            {t!(i18n, channel_remove_keep)}
                        </button>
                    </div>
                </div>
                <div class="buttons" class:gone=move || removing.get()>
                    {(!adding && !primary)
                        .then(|| {
                            view! {
                                <button type="button" class="danger" on:click=move |_| removing.set(true)>
                                    {t!(i18n, channel_remove)}
                                </button>
                            }
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
