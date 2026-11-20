// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios: adding, changing and removing them.

use crate::app::{LinkLabel, Live, link_class};
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::ago;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{ErrorCode, LinkState, Radio, RadioId, RadioKind, RadioSettings};

#[component]
pub fn RadiosView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // The radio being changed, or none when the form adds one.
    let editing = RwSignal::new(None::<Radio>);
    let radios = move || live.radios.get().unwrap_or_default();

    view! {
        <section class="card">
            <h2>{t!(i18n, radios_title)}</h2>
            <div class="scroll">
                <table>
                    <thead>
                        <tr>
                            <th>{t!(i18n, radio_name)}</th>
                            <th>{t!(i18n, radio_kind)}</th>
                            <th>{t!(i18n, radio_address)}</th>
                            <th>{t!(i18n, radio_state)}</th>
                            <th></th>
                        </tr>
                    </thead>
                    <tbody>
                        <For each=radios key=|radio| radio.clone() let:radio>
                            <RadioRow radio editing />
                        </For>
                    </tbody>
                </table>
            </div>
        </section>
        <section class="card">
            <h2>
                {move || match editing.get() {
                    Some(_) => t!(i18n, radio_edit_title).into_any(),
                    None => t!(i18n, radio_add_title).into_any(),
                }}
            </h2>
            <RadioForm editing />
        </section>
    }
}

#[component]
fn RadioRow(radio: Radio, editing: RwSignal<Option<Radio>>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let id = radio.id;
    let link = radio.link;
    let enabled = radio.enabled;
    let edit = {
        let radio = radio.clone();
        move |_| editing.set(Some(radio.clone()))
    };
    // One click, for a radio that comes and goes: the one in the car, say.
    let toggle = {
        let settings = RadioSettings {
            name: radio.name.clone(),
            kind: radio.kind,
            address: radio.address.clone(),
            enabled: !radio.enabled,
        };
        move |_| {
            let settings = settings.clone();
            spawn_local(async move {
                if daemon::save_radio(Some(id), &settings).await.is_ok() {
                    let _ = live.refresh().await;
                }
            });
        }
    };
    let remove = move |_| {
        let sure = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(t_string!(i18n, radio_remove_confirm))
                    .ok()
            })
            .unwrap_or(false);
        if sure {
            spawn_local(async move {
                if daemon::remove_radio(id).await.is_ok() {
                    if editing.get_untracked().is_some_and(|r| r.id == id) {
                        editing.set(None);
                    }
                    let _ = live.refresh_radios().await;
                }
            });
        }
    };

    view! {
        <tr>
            <td>
                <a href="#" on:click=move |ev| {
                    ev.prevent_default();
                    live.select(id);
                }>{radio.name.clone()}</a>
            </td>
            <td>{kind_name(radio.kind)}</td>
            <td class="mono">{radio.address.clone()}</td>
            <td>
                <span class=format!("badge {}", link_class(link))>
                    <LinkLabel link=Signal::stored(link) />
                </span>
                {(link != LinkState::Ready)
                    .then_some(radio.last_connected)
                    .flatten()
                    .map(|at| {
                        let at = u32::try_from(at).unwrap_or_default();
                        view! {
                            <span class="dim">
                                " · " {move || ago(live.now.get(), at)}
                            </span>
                        }
                    })}
            </td>
            <td class="actions">
                <button on:click=toggle>
                    {if enabled {
                        t!(i18n, radio_disable).into_any()
                    } else {
                        t!(i18n, radio_enable).into_any()
                    }}
                </button>
                <button on:click=edit>{t!(i18n, radio_edit)}</button>
                <button class="danger" on:click=remove>{t!(i18n, radio_remove)}</button>
            </td>
        </tr>
    }
}

fn kind_name(kind: RadioKind) -> &'static str {
    match kind {
        RadioKind::Meshtastic => "Meshtastic",
        RadioKind::Meshcore => "MeshCore",
    }
}

/// Adds a radio, or changes the one in `editing`.
#[component]
pub fn RadioForm(editing: RwSignal<Option<Radio>>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let name = RwSignal::new(String::new());
    let address = RwSignal::new(String::new());
    let kind = RwSignal::new(RadioKind::Meshtastic);
    let enabled = RwSignal::new(true);
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);

    // The form shows the radio being changed, or empties for a new one.
    Effect::new(move |_| {
        let radio = editing.get();
        name.set(radio.as_ref().map(|r| r.name.clone()).unwrap_or_default());
        address.set(
            radio
                .as_ref()
                .map(|r| r.address.clone())
                .unwrap_or_default(),
        );
        enabled.set(radio.as_ref().is_none_or(|r| r.enabled));
        kind.set(radio.as_ref().map_or(RadioKind::Meshtastic, |r| r.kind));
        error.set(None);
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let settings = RadioSettings {
            name: name.get_untracked(),
            kind: kind.get_untracked(),
            address: address.get_untracked(),
            enabled: enabled.get_untracked(),
        };
        let id: Option<RadioId> = editing.get_untracked().map(|r| r.id);
        busy.set(true);
        spawn_local(async move {
            match daemon::save_radio(id, &settings).await {
                Ok(radio) => {
                    editing.set(None);
                    let _ = live.refresh_radios().await;
                    if id.is_none() {
                        live.select(radio.id);
                    }
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };

    let message = move || {
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::InvalidName) => t!(i18n, error_invalid_name).into_any(),
            Failure::Rejected(ErrorCode::InvalidAddress) => {
                t!(i18n, error_invalid_address).into_any()
            }
            Failure::Rejected(ErrorCode::UnsupportedKind) => {
                t!(i18n, error_unsupported_kind).into_any()
            }
            _ => t!(i18n, error_failed).into_any(),
        })
    };

    view! {
        <form class="radio-form" on:submit=submit>
            <label>
                <span>{t!(i18n, radio_name)}</span>
                <input
                    required
                    maxlength="64"
                    placeholder=move || t_string!(i18n, radio_name_hint)
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                />
            </label>
            <label>
                <span>{t!(i18n, radio_kind)}</span>
                <select
                    prop:value=move || match kind.get() {
                        RadioKind::Meshtastic => "meshtastic",
                        RadioKind::Meshcore => "meshcore",
                    }
                    on:change=move |ev| {
                        kind.set(match event_target_value(&ev).as_str() {
                            "meshcore" => RadioKind::Meshcore,
                            _ => RadioKind::Meshtastic,
                        });
                    }
                >
                    <option value="meshtastic">"Meshtastic"</option>
                    <option value="meshcore">"MeshCore"</option>
                </select>
            </label>
            <label>
                <span>{t!(i18n, radio_address)}</span>
                <input
                    required
                    class="mono"
                    placeholder="192.168.1.10:4403 · /dev/ttyACM0"
                    prop:value=move || address.get()
                    on:input=move |ev| address.set(event_target_value(&ev))
                />
            </label>
            <label class="check">
                <input
                    type="checkbox"
                    prop:checked=move || enabled.get()
                    on:change=move |ev| enabled.set(event_target_checked(&ev))
                />
                <span>{t!(i18n, radio_enabled)}</span>
            </label>
            <p class="hint">{t!(i18n, radio_address_hint)}</p>
            <p class="error">{message}</p>
            <div class="buttons">
                <button type="submit" class="primary" disabled=move || busy.get()>
                    {move || match editing.get() {
                        Some(_) => t!(i18n, radio_save).into_any(),
                        None => t!(i18n, radio_add).into_any(),
                    }}
                </button>
                <Show when=move || editing.get().is_some()>
                    <button type="button" on:click=move |_| editing.set(None)>
                        {t!(i18n, radio_cancel)}
                    </button>
                </Show>
            </div>
        </form>
    }
}
