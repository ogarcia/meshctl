// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's admin keys: the public keys of the radios it takes admin
//! requests from, over the mesh, which let them read and change its
//! settings. Up to three, added from the nodes known or written, and taken
//! away; each change saved at once, as the radio reboots for it. The rest
//! of its security, its own keys and managed mode, is left as it is.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{AdminKeys, ErrorCode};

/// How many keys the firmware takes.
const MAX_KEYS: usize = 3;

/// The section with the radio's admin keys.
#[component]
pub fn AdminKeysPanel() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let node = crate::settings::node_of();
    let keys = RwSignal::new(None::<AdminKeys>);
    let problem = RwSignal::new(None::<Failure>);
    let applied = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let picking = RwSignal::new(false);
    let load = move || {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match daemon::admin_keys(radio, node).await {
                Ok(read) => {
                    let _ = keys.try_set(Some(read));
                }
                Err(failure) => {
                    let _ = problem.try_set(Some(failure));
                }
            }
        });
    };
    load();
    // Read again as the radio syncs anew, after rebooting for a change;
    // the note that it reboots, gone then.
    Effect::new(move |was: Option<u32>| {
        let synced = live.synced.get();
        if was.is_some_and(|was| was != synced) {
            applied.set(false);
            load();
        }
        synced
    });
    // All of them as they are to be, sent at once, once confirmed: the
    // radio reboots.
    let save = move |next: Vec<String>| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let sure = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(t_string!(i18n, settings_reboot_confirm))
                    .ok()
            })
            .unwrap_or(false);
        if !sure {
            return;
        }
        busy.set(true);
        problem.set(None);
        let managed = keys.with_untracked(|k| k.as_ref().is_some_and(|k| k.managed));
        spawn_local(async move {
            let change = AdminKeys {
                keys: next.clone(),
                managed,
            };
            // As said by then, the page perhaps gone.
            match daemon::change_admin_keys(radio, node, &change).await {
                Ok(_) => {
                    let _ = keys.try_set(Some(change));
                    let _ = applied.try_set(true);
                }
                Err(failure) => {
                    let _ = problem.try_set(Some(failure));
                }
            }
            let _ = busy.try_set(false);
        });
    };
    let current = move || keys.with(|k| k.as_ref().map(|k| k.keys.clone()).unwrap_or_default());
    let remove = move |key: String| {
        let next: Vec<String> = current().into_iter().filter(|k| *k != key).collect();
        save(next);
    };
    let add = move |key: String| {
        picking.set(false);
        let mut next = current();
        if !next.contains(&key) && next.len() < MAX_KEYS {
            next.push(key);
            save(next);
        }
    };
    view! {
        <div class="section-head">
            <h2>{move || crate::names::section(i18n.get_locale(), "security")}</h2>
        </div>
        <div class="admin-keys">
            <h3>{t!(i18n, admin_keys_title)}</h3>
            <p class="dim">{t!(i18n, admin_keys_hint)}</p>
            {node.map(|_| view! { <p class="dim">{t!(i18n, admin_keys_remote_hint)}</p> })}
            {move || {
                keys.get()
                    .map(|read| {
                        let managed = read.managed;
                        let rows = read
                            .keys
                            .iter()
                            .cloned()
                            .map(|key| {
                                let shown = key.clone();
                                let gone = key.clone();
                                // The last one of a managed radio stays.
                                let last = managed && read.keys.len() == 1;
                                view! {
                                    <li>
                                        <span class="admin-key-who">
                                            <b>{move || owner_of(i18n, live, &key)}</b>
                                            <span class="mono public-key" title=shown.clone()>
                                                {shown.clone()}
                                            </span>
                                        </span>
                                        <button
                                            type="button"
                                            class="icon-button"
                                            disabled=move || busy.get() || last
                                            title=move || t_string!(i18n, admin_keys_remove)
                                            aria-label=move || t_string!(i18n, admin_keys_remove)
                                            inner_html=TRASH
                                            on:click=move |_| remove(gone.clone())
                                        ></button>
                                    </li>
                                }
                            })
                            .collect_view();
                        let full = read.keys.len() >= MAX_KEYS;
                        view! {
                            {read.keys.is_empty().then(|| view! {
                                <p class="admin-keys-none">{t!(i18n, admin_keys_none)}</p>
                            })}
                            <ul class="admin-key-list">{rows}</ul>
                            {managed.then(|| view! {
                                <p class="dim">{t!(i18n, admin_keys_managed)}</p>
                            })}
                            <button
                                type="button"
                                class="with-icon"
                                disabled=move || busy.get() || full
                                on:click=move |_| picking.set(true)
                            >
                                <span class="icon" inner_html=PLUS></span>
                                <span class="label">{t!(i18n, admin_keys_add)}</span>
                            </button>
                        }
                    })
            }}
            {move || {
                applied.get().then(|| view! { <p class="note">{t!(i18n, settings_applied_reboot)}</p> })
            }}
            {move || {
                problem
                    .get()
                    .map(|failure| {
                        let words = match failure {
                            Failure::Rejected(ErrorCode::InvalidSettings) => {
                                t_string!(i18n, admin_keys_invalid)
                            }
                            Failure::Rejected(ErrorCode::RadioNotConnected) => {
                                t_string!(i18n, chat_not_connected)
                            }
                            _ => t_string!(i18n, error_failed),
                        };
                        view! { <p class="note bad">{words}</p> }
                    })
            }}
        </div>
        <Show when=move || picking.get()>
            <KeyPicker
                taken=Signal::derive(current)
                pick=Callback::new(add)
                close=Callback::new(move |()| picking.set(false))
            />
        </Show>
    }
}

/// Whose a key is, among the nodes known: its name, or that nobody known
/// has it.
fn owner_of(i18n: leptos_i18n::I18nContext<crate::i18n::Locale>, live: Live, key: &str) -> String {
    live.nodes
        .with(|nodes| {
            nodes
                .values()
                .find(|n| n.public_key.as_deref() == Some(key))
                .map(|n| crate::chat::title(live, meshctl_api::Peer::Node { node: n.num }))
        })
        .unwrap_or_else(|| t_string!(i18n, admin_keys_unknown).to_owned())
}

/// The nodes known with a key, to take one of theirs, but the radio's own
/// and those taken already; or a key written.
#[component]
fn KeyPicker(
    taken: Signal<Vec<String>>,
    pick: Callback<String>,
    close: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let filter = RwSignal::new(String::new());
    let written = RwSignal::new(String::new());
    let own = live
        .status
        .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
    let nodes = move || {
        let needle = filter.get().to_lowercase();
        let taken = taken.get();
        let mut shown: Vec<(String, String)> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter(|n| Some(n.num) != own)
                .filter_map(|n| {
                    let key = n.public_key.clone()?;
                    let name = crate::chat::title(live, meshctl_api::Peer::Node { node: n.num });
                    (!taken.contains(&key)
                        && (needle.is_empty() || name.to_lowercase().contains(&needle)))
                    .then_some((name, key))
                })
                .collect()
        });
        shown.sort_by_key(|(name, _)| name.to_lowercase());
        shown
    };
    // A key as written: base64 of 32 bytes.
    let readable = move || {
        let key = written.get();
        let key = key.trim();
        window()
            .atob(key)
            .ok()
            .filter(|bytes| bytes.chars().count() == 32)
            .map(|_| key.to_owned())
    };
    let title = Signal::derive(move || t_string!(i18n, admin_keys_add).to_owned());
    view! {
        <crate::shared::Picker title close>
            <div class="key-written">
                <input
                    type="text"
                    class="mono"
                    spellcheck="false"
                    autocomplete="off"
                    placeholder=move || t_string!(i18n, admin_keys_write)
                    prop:value=move || written.get()
                    on:input=move |ev| written.set(event_target_value(&ev))
                />
                <button
                    type="button"
                    class="primary"
                    disabled=move || readable().is_none()
                    on:click=move |_| {
                        if let Some(key) = readable() {
                            pick.run(key);
                        }
                    }
                >
                    {t!(i18n, admin_keys_take)}
                </button>
            </div>
            <input
                class="picker-search"
                type="search"
                placeholder=move || t_string!(i18n, search)
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))
            />
            <ul class="picker-list">
                {move || {
                    nodes()
                        .into_iter()
                        .map(|(name, key)| {
                            let short = format!("{}…", key.chars().take(12).collect::<String>());
                            view! {
                                <li>
                                    <button type="button" on:click=move |_| pick.run(key.clone())>
                                        <span class="shared-mark avatar">{crate::badge::initials(&name)}</span>
                                        <span class="shared-what">
                                            <b>{name.clone()}</b>
                                            <span class="dim mono">{short}</span>
                                        </span>
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()
                }}
            </ul>
        </crate::shared::Picker>
    }
}

const TRASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/trash.svg");
const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");
