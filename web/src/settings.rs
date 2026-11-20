// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's own settings: read from it while it is connected, changed all
//! at once, and read back once it has applied them.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::sections::Sections;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    ChannelChange, ChannelKey, ErrorCode, KeyChange, LinkState, NodeSettings, NodeSettingsChange,
    SettingsSection,
};

/// What changed between the settings read and the ones in the form.
fn changes(
    original: &NodeSettings,
    draft: &NodeSettings,
    keys: &[KeyChange],
) -> NodeSettingsChange {
    NodeSettingsChange {
        owner: (draft.owner != original.owner).then(|| draft.owner.clone()),
        device: (draft.device != original.device).then(|| draft.device.clone()),
        lora: (draft.lora != original.lora).then(|| draft.lora.clone()),
        channels: draft
            .channels
            .iter()
            .zip(&original.channels)
            .zip(keys)
            .filter(|((now, before), key)| now != before || **key != KeyChange::Keep)
            .map(|((now, _), key)| ChannelChange {
                index: now.index,
                role: now.role.clone(),
                name: now.name.clone(),
                key: *key,
                uplink: now.uplink,
                downlink: now.downlink,
            })
            .collect(),
    }
}

fn is_empty(change: &NodeSettingsChange) -> bool {
    change.owner.is_none()
        && change.device.is_none()
        && change.lora.is_none()
        && change.channels.is_empty()
}

#[component]
pub fn SettingsView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let loaded = RwSignal::new(None::<NodeSettings>);
    let sections = RwSignal::new(Vec::<SettingsSection>::new());
    let draft = RwSignal::new(None::<NodeSettings>);
    let keys = RwSignal::new(vec![KeyChange::Keep; 8]);
    let notice = RwSignal::new(None::<Result<bool, Failure>>);
    let busy = RwSignal::new(false);

    let ready = move || {
        live.status
            .with(|s| s.as_ref().is_some_and(|s| s.link == LinkState::Ready))
    };
    // Still shown while the radio only syncs again, as after every change:
    // what is being edited elsewhere on the page must not vanish under it.
    let shown = move || {
        live.selected_kind() != Some(meshctl_api::RadioKind::Meshcore)
            && live.status.with(|s| {
                s.as_ref()
                    .is_some_and(|s| matches!(s.link, LinkState::Ready | LinkState::Syncing))
            })
    };

    // Read them whenever the radio is (again) in sync: on opening the page,
    // and once a change has been applied and read back.
    Effect::new(move |_| {
        let radio = live.selected.get();
        if !ready() {
            return;
        }
        live.synced.track();
        let Some(radio) = radio else { return };
        spawn_local(async move {
            if let Ok(settings) = daemon::settings(radio).await {
                keys.set(vec![KeyChange::Keep; settings.channels.len()]);
                draft.set(Some(settings.clone()));
                loaded.set(Some(settings));
            }
            if let Ok(read) = daemon::sections(radio).await {
                sections.set(read);
            }
        });
    });

    let pending = move || {
        let (Some(original), Some(now)) = (loaded.get(), draft.get()) else {
            return None;
        };
        let change = changes(&original, &now, &keys.get());
        (!is_empty(&change)).then_some(change)
    };

    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let (Some(radio), Some(change)) = (live.selected.get_untracked(), pending()) else {
            return;
        };
        // Role and LoRa changes reboot the radio: say so before, not after.
        if change.device.is_some() || change.lora.is_some() {
            let sure = web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(t_string!(i18n, settings_reboot_confirm))
                        .ok()
                })
                .unwrap_or(false);
            if !sure {
                return;
            }
        }
        busy.set(true);
        notice.set(None);
        spawn_local(async move {
            let applied = daemon::change_settings(radio, &change).await;
            notice.set(Some(applied.map(|a| a.reboots)));
            busy.set(false);
        });
    };

    let notice_view = move || {
        notice.get().map(|outcome| match outcome {
            Ok(true) => {
                view! { <p class="card note">{t!(i18n, settings_applied_reboot)}</p> }.into_any()
            }
            Ok(false) => view! { <p class="card note">{t!(i18n, settings_applied)}</p> }.into_any(),
            Err(Failure::Rejected(ErrorCode::InvalidSettings)) => view! {
                <p class="card note bad">{t!(i18n, settings_invalid)}</p>
            }
            .into_any(),
            Err(Failure::Rejected(ErrorCode::RadioDidNotAnswer)) => view! {
                <p class="card note bad">{t!(i18n, settings_no_answer)}</p>
            }
            .into_any(),
            Err(_) => view! { <p class="card note bad">{t!(i18n, error_failed)}</p> }.into_any(),
        })
    };

    view! {
        <Show
            when=shown
            fallback=move || {
                if live.selected_kind() == Some(meshctl_api::RadioKind::Meshcore) {
                    view! { <p class="card dim">{t!(i18n, meshcore_not_yet)}</p> }.into_any()
                } else {
                    view! { <p class="card dim">{t!(i18n, settings_not_connected)}</p> }.into_any()
                }
            }
        >
            {notice_view}
            {move || {
                draft
                    .get()
                    .is_some()
                    .then(|| {
                        view! {
                            <form class="settings" on:submit=save>
                                <Identity draft />
                                <Radio draft />
                                <Channels draft keys loaded />
                                <div class="card save">
                                    <button
                                        type="submit"
                                        class="primary"
                                        disabled=move || busy.get() || pending().is_none()
                                    >
                                        {t!(i18n, settings_save)}
                                    </button>
                                    <button
                                        type="button"
                                        disabled=move || pending().is_none()
                                        on:click=move |_| {
                                            draft.set(loaded.get_untracked());
                                            keys.update(|k| k.fill(KeyChange::Keep));
                                        }
                                    >
                                        {t!(i18n, settings_undo)}
                                    </button>
                                </div>
                            </form>
                        }
                    })
            }}
            <Sections sections />
        </Show>
    }
}

type Draft = RwSignal<Option<NodeSettings>>;

/// A field of the draft, read and written.
fn field<T: Clone + Send + Sync + 'static>(
    draft: Draft,
    get: impl Fn(&NodeSettings) -> T + Copy + Send + Sync + 'static,
    set: impl Fn(&mut NodeSettings, T) + Copy + Send + Sync + 'static,
) -> (Signal<T>, impl Fn(T) + Copy) {
    let read = Signal::derive(move || draft.with(|d| get(d.as_ref().expect("loaded"))));
    let write = move |value: T| {
        draft.update(|d| {
            if let Some(d) = d {
                set(d, value);
            }
        });
    };
    (read, write)
}

#[component]
fn Identity(draft: Draft) -> impl IntoView {
    let i18n = use_i18n();
    let (long_name, set_long) = field(
        draft,
        |d| d.owner.long_name.clone(),
        |d, v| d.owner.long_name = v,
    );
    let (short_name, set_short) = field(
        draft,
        |d| d.owner.short_name.clone(),
        |d, v| d.owner.short_name = v,
    );
    let (role, set_role) = field(draft, |d| d.device.role.clone(), |d, v| d.device.role = v);
    let roles = move || {
        draft.with(|d| {
            d.as_ref()
                .map(|d| d.choices.roles.clone())
                .unwrap_or_default()
        })
    };

    view! {
        <section class="card">
            <h2>{t!(i18n, settings_identity)}</h2>
            <div class="fields">
                <label>
                    <span>{t!(i18n, settings_long_name)}</span>
                    <input
                        maxlength="39"
                        prop:value=long_name
                        on:input=move |ev| set_long(event_target_value(&ev))
                    />
                </label>
                <label>
                    <span>{t!(i18n, settings_short_name)}</span>
                    <input
                        class="mono"
                        prop:value=short_name
                        on:input=move |ev| set_short(event_target_value(&ev))
                    />
                </label>
                <label>
                    <span>{t!(i18n, settings_role)}</span>
                    <Choice options=roles value=role set=set_role />
                </label>
            </div>
        </section>
    }
}

#[component]
fn Radio(draft: Draft) -> impl IntoView {
    let i18n = use_i18n();
    let (region, set_region) = field(draft, |d| d.lora.region.clone(), |d, v| d.lora.region = v);
    let (preset, set_preset) = field(
        draft,
        |d| d.lora.modem_preset.clone(),
        |d, v| d.lora.modem_preset = v,
    );
    let (use_preset, set_use_preset) =
        field(draft, |d| d.lora.use_preset, |d, v| d.lora.use_preset = v);
    let (hop_limit, set_hop_limit) =
        field(draft, |d| d.lora.hop_limit, |d, v| d.lora.hop_limit = v);
    let (tx, set_tx) = field(draft, |d| d.lora.tx_enabled, |d, v| d.lora.tx_enabled = v);
    let regions = move || {
        draft.with(|d| {
            d.as_ref()
                .map(|d| d.choices.regions.clone())
                .unwrap_or_default()
        })
    };
    let presets = move || {
        draft.with(|d| {
            d.as_ref()
                .map(|d| d.choices.presets.clone())
                .unwrap_or_default()
        })
    };

    view! {
        <section class="card">
            <h2>{t!(i18n, settings_radio)}</h2>
            <div class="fields">
                <label>
                    <span>{t!(i18n, settings_region)}</span>
                    <Choice options=regions value=region set=set_region />
                </label>
                <label>
                    <span>{t!(i18n, settings_preset)}</span>
                    <Choice options=presets value=preset set=set_preset />
                </label>
                <label>
                    <span>{t!(i18n, settings_hop_limit)}</span>
                    <input
                        type="number"
                        min="1"
                        max="7"
                        prop:value=move || hop_limit.get().to_string()
                        on:input=move |ev| {
                            if let Ok(hops) = event_target_value(&ev).parse() {
                                set_hop_limit(hops);
                            }
                        }
                    />
                </label>
                <label class="check">
                    <input
                        type="checkbox"
                        prop:checked=use_preset
                        on:change=move |ev| set_use_preset(event_target_checked(&ev))
                    />
                    <span>{t!(i18n, settings_use_preset)}</span>
                </label>
                <label class="check">
                    <input
                        type="checkbox"
                        prop:checked=tx
                        on:change=move |ev| set_tx(event_target_checked(&ev))
                    />
                    <span>{t!(i18n, settings_tx)}</span>
                </label>
            </div>
            <p class="hint dim">{t!(i18n, settings_reboot_hint)}</p>
        </section>
    }
}

#[component]
fn Channels(
    draft: Draft,
    keys: RwSignal<Vec<KeyChange>>,
    loaded: RwSignal<Option<NodeSettings>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <section class="card">
            <h2>{t!(i18n, settings_channels)}</h2>
            <div class="scroll">
                <table class="channels">
                    <thead>
                        <tr>
                            <th>"#"</th>
                            <th>{t!(i18n, settings_channel_role)}</th>
                            <th>{t!(i18n, settings_channel_name)}</th>
                            <th>{t!(i18n, settings_channel_key)}</th>
                            <th>"↑ MQTT"</th>
                            <th>"↓ MQTT"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {(0..8usize)
                            .map(|index| view! { <ChannelRow index draft keys loaded /> })
                            .collect_view()}
                    </tbody>
                </table>
            </div>
        </section>
    }
}

#[component]
fn ChannelRow(
    index: usize,
    draft: Draft,
    keys: RwSignal<Vec<KeyChange>>,
    loaded: RwSignal<Option<NodeSettings>>,
) -> impl IntoView {
    // `slot` is a word of the view macro's own; called index outside it.
    let slot = index;
    let i18n = use_i18n();
    let (role, set_role) = field(
        draft,
        move |d| d.channels[slot].role.clone(),
        move |d, v| d.channels[slot].role = v,
    );
    let (name, set_name) = field(
        draft,
        move |d| d.channels[slot].name.clone(),
        move |d, v| d.channels[slot].name = v,
    );
    let (uplink, set_uplink) = field(
        draft,
        move |d| d.channels[slot].uplink,
        move |d, v| d.channels[slot].uplink = v,
    );
    let (downlink, set_downlink) = field(
        draft,
        move |d| d.channels[slot].downlink,
        move |d, v| d.channels[slot].downlink = v,
    );
    let primary = slot == 0;
    let roles = move || {
        if primary {
            vec!["PRIMARY".to_owned()]
        } else {
            vec!["SECONDARY".to_owned(), "DISABLED".to_owned()]
        }
    };
    // What the key is now, as the radio has it.
    let current = move || {
        loaded.with(|l| {
            l.as_ref()
                .map_or(ChannelKey::None, |l| l.channels[slot].key)
        })
    };
    let current_label = move || match current() {
        ChannelKey::None => t_string!(i18n, key_none),
        ChannelKey::Default => t_string!(i18n, key_default),
        ChannelKey::Simple => t_string!(i18n, key_simple),
        ChannelKey::Aes128 => "AES-128",
        ChannelKey::Aes256 => "AES-256",
    };
    let key_value = move || match keys.with(|k| k[slot]) {
        KeyChange::Keep => "keep",
        KeyChange::None => "none",
        KeyChange::Default => "default",
        KeyChange::NewAes128 => "new_aes128",
        KeyChange::NewAes256 => "new_aes256",
    };
    let set_key = move |value: String| {
        let change = match value.as_str() {
            "none" => KeyChange::None,
            "default" => KeyChange::Default,
            "new_aes128" => KeyChange::NewAes128,
            "new_aes256" => KeyChange::NewAes256,
            _ => KeyChange::Keep,
        };
        keys.update(|k| k[slot] = change);
    };

    view! {
        <tr>
            <td class="num">{slot}</td>
            <td><Choice options=roles value=role set=set_role /></td>
            <td>
                <input
                    maxlength="11"
                    prop:value=name
                    on:input=move |ev| set_name(event_target_value(&ev))
                />
            </td>
            <td>
                <select
                    prop:value=key_value
                    on:change=move |ev| set_key(event_target_value(&ev))
                >
                    <option value="keep">
                        {t!(i18n, key_keep)} " (" {current_label} ")"
                    </option>
                    <option value="none">{t!(i18n, key_none)}</option>
                    <option value="default">{t!(i18n, key_default)}</option>
                    <option value="new_aes128">{t!(i18n, key_new)} " AES-128"</option>
                    <option value="new_aes256">{t!(i18n, key_new)} " AES-256"</option>
                </select>
            </td>
            <td>
                <input
                    type="checkbox"
                    prop:checked=uplink
                    on:change=move |ev| set_uplink(event_target_checked(&ev))
                />
            </td>
            <td>
                <input
                    type="checkbox"
                    prop:checked=downlink
                    on:change=move |ev| set_downlink(event_target_checked(&ev))
                />
            </td>
        </tr>
    }
}

/// A select over a list of codes, which shows them as they are: the radio's
/// own names for its roles, regions and presets.
#[component]
fn Choice(
    options: impl Fn() -> Vec<String> + Send + Sync + 'static,
    value: Signal<String>,
    set: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <select prop:value=value on:change=move |ev| set(event_target_value(&ev))>
            {move || {
                options()
                    .into_iter()
                    .map(|code| {
                        let selected = value.get_untracked() == code;
                        view! {
                            <option value=code.clone() selected=selected>
                                {code.clone()}
                            </option>
                        }
                    })
                    .collect_view()
            }}
        </select>
    }
}
