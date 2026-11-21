// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's channels, in its settings: those it has, each edited on its
//! own over the list and saved to the radio straight away. A channel's key
//! is asked for only to be shown, and forgotten with its editor.

use crate::app::Live;
use crate::chat::preset_name;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::radios::Switch;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    ChannelChange, ChannelKey, ChannelSecret, ChannelSettings, ErrorCode, KeyChange, NodeSettings,
    NodeSettingsChange,
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

/// What a save or a removal came to, as the settings page tells it.
pub type Notice = RwSignal<Option<Result<bool, Failure>>>;

/// Which channel is being edited: one there is, or a new one, by its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Editing {
    Closed,
    Slot(u32),
    New(u32),
}

/// A channel's name, or, for an unnamed primary, its modem preset's.
fn shown_name(settings: &NodeSettings, channel: &ChannelSettings) -> String {
    if channel.name.is_empty() && channel.index == 0 {
        preset_name(&settings.lora.modem_preset)
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
    let editing = RwSignal::new(Editing::Closed);
    // Those it has, in their slots' order.
    let channels = move || {
        loaded.with(|l| {
            l.as_ref()
                .map(|l| {
                    l.channels
                        .iter()
                        .filter(|c| c.role != "DISABLED")
                        .map(|c| (shown_name(l, c), c.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
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
        <section class="channel-list">
            <For each=channels key=|(name, c)| format!("{name} {c:?}") let:((name, channel))>
                <ChannelRow name channel editing />
            </For>
            <div class="channel-foot">
                <p class="hint dim">{t!(i18n, channels_hint)}</p>
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
fn ChannelRow(name: String, channel: ChannelSettings, editing: RwSignal<Editing>) -> impl IntoView {
    let i18n = use_i18n();
    let slot = channel.index;
    let key = channel.key;
    view! {
        <button type="button" class="channel-row" on:click=move |_| editing.set(Editing::Slot(slot))>
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
                <span class="key" class:open=!private(key)>
                    <span class="icon" inner_html=if private(key) { LOCK } else { LOCK_OPEN }></span>
                    {move || key_label(i18n, key)}
                </span>
            </span>
        </button>
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
            l.map(|l| preset_name(&l.lora.modem_preset))
                .unwrap_or_default(),
        )
    });
    let start = start.unwrap_or(ChannelSettings {
        index: slot,
        role: "DISABLED".to_owned(),
        name: String::new(),
        key: ChannelKey::None,
        uplink: false,
        downlink: false,
    });
    let current_key = start.key;
    let name = RwSignal::new(if adding {
        String::new()
    } else {
        start.name.clone()
    });
    let uplink = RwSignal::new(!adding && start.uplink);
    let downlink = RwSignal::new(!adding && start.downlink);
    // A new one gets a key of its own, as the apps give it.
    let key = RwSignal::new(if adding {
        KeyChange::NewAes256
    } else {
        KeyChange::Keep
    });
    let custom = RwSignal::new(String::new());
    // The key itself, while shown: nowhere else, and gone with the editor.
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
        });
    };
    // Taken off the radio: its slot freed, and its key with it.
    let remove = move |_| {
        let sure = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(t_string!(i18n, channel_remove_confirm))
                    .ok()
            })
            .unwrap_or(false);
        if sure {
            send(ChannelChange {
                index: slot,
                role: "DISABLED".to_owned(),
                name: String::new(),
                key: KeyChange::None,
                custom_key: None,
                uplink: false,
                downlink: false,
            });
        }
    };

    let message = move || {
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::InvalidSettings) => t!(i18n, channel_invalid).into_any(),
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
        let shown = if start.name.is_empty() && primary {
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
                <div class="fields">
                    <label>
                        <span class="dim">{t!(i18n, channel_name)}</span>
                        <input
                            node_ref=name_input
                            maxlength="11"
                            required=!primary
                            placeholder=move || {
                                if primary {
                                    format!("{}: {preset}", t_string!(i18n, channel_name_preset))
                                } else {
                                    t_string!(i18n, channel_name_hint).to_owned()
                                }
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
                </div>
                <p class="error">{message}</p>
                <div class="buttons">
                    {(!adding && !primary)
                        .then(|| {
                            view! {
                                <button type="button" class="danger" on:click=remove>
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
