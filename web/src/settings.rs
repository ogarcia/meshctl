// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's own settings, in tabs as the phone apps have them: who it is,
//! how it transmits, its channels, the rest of its configuration and its
//! modules. Read from it while it is connected, changed a tab at a time, and
//! read back once it has applied them.

use crate::app::Live;
use crate::channel_settings::ChannelsTab;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::preferences::pills;
use crate::radios::Switch;
use crate::sections::{SaveBar, SectionEdits, SectionsTab, clear_when_synced};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    ErrorCode, LinkState, NodeSettings, NodeSettingsChange, RadioId, RadioKind, SettingsSection,
};
use std::collections::BTreeMap;

/// What changed between the settings read and the ones in the form. The
/// channels are not in it: each is saved on its own, from its editor.
fn changes(original: &NodeSettings, draft: &NodeSettings) -> NodeSettingsChange {
    NodeSettingsChange {
        owner: (draft.owner != original.owner).then(|| draft.owner.clone()),
        device: (draft.device != original.device).then(|| draft.device.clone()),
        lora: (draft.lora != original.lora).then(|| draft.lora.clone()),
        channels: Vec::new(),
    }
}

fn is_empty(change: &NodeSettingsChange) -> bool {
    change.owner.is_none()
        && change.device.is_none()
        && change.lora.is_none()
        && change.channels.is_empty()
}

/// The page's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// Its names, and what it does in the mesh.
    User,
    Lora,
    Channels,
    /// The rest of its configuration.
    Device,
    Modules,
}

const TABS: [Tab; 5] = [
    Tab::User,
    Tab::Lora,
    Tab::Channels,
    Tab::Device,
    Tab::Modules,
];

/// What of a change a tab holds: a tab saves only its own.
fn part_of(tab: Tab, change: NodeSettingsChange) -> NodeSettingsChange {
    match tab {
        Tab::User => NodeSettingsChange {
            owner: change.owner,
            device: change.device,
            ..Default::default()
        },
        Tab::Lora => NodeSettingsChange {
            lora: change.lora,
            ..Default::default()
        },
        Tab::Channels | Tab::Device | Tab::Modules => NodeSettingsChange::default(),
    }
}

/// A tab's part of `from`, put in `to`: to undo it, or to take it as saved.
fn copy_part(tab: Tab, from: &NodeSettings, to: &mut NodeSettings) {
    match tab {
        Tab::User => {
            to.owner = from.owner.clone();
            to.device = from.device.clone();
        }
        Tab::Lora => to.lora = from.lora.clone(),
        Tab::Channels | Tab::Device | Tab::Modules => {}
    }
}

/// The settings read again, keeping what is being edited of them: saving a
/// tab, which reads them all back, does not undo another.
fn merge(before: &NodeSettings, draft: &NodeSettings, read: &NodeSettings) -> NodeSettings {
    let mut merged = read.clone();
    if draft.owner != before.owner {
        merged.owner = draft.owner.clone();
    }
    if draft.device != before.device {
        merged.device = draft.device.clone();
    }
    if draft.lora != before.lora {
        merged.lora = draft.lora.clone();
    }
    merged
}

#[component]
pub fn SettingsView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let loaded = RwSignal::new(None::<NodeSettings>);
    let sections = RwSignal::new(Vec::<SettingsSection>::new());
    let draft = RwSignal::new(None::<NodeSettings>);
    let section_edits: SectionEdits = RwSignal::new(BTreeMap::new());
    let notice = RwSignal::new(None::<Result<bool, Failure>>);
    clear_when_synced(live, notice);
    let busy = RwSignal::new(false);
    let tab = RwSignal::new(Tab::User);

    let ready = move || {
        live.status
            .with(|s| s.as_ref().is_some_and(|s| s.link == LinkState::Ready))
    };
    // Still shown while the radio only syncs again, as after every change:
    // what is being edited elsewhere on the page must not vanish under it.
    let shown = move || {
        live.selected_kind() != Some(RadioKind::Meshcore)
            && live.status.with(|s| {
                s.as_ref()
                    .is_some_and(|s| matches!(s.link, LinkState::Ready | LinkState::Syncing))
            })
    };

    // Read them whenever the radio is (again) in sync: on opening the page,
    // and once a change has been applied and read back. What another radio
    // was being given goes.
    Effect::new(move |last: Option<Option<RadioId>>| {
        let radio = live.selected.get();
        if last.is_some_and(|last| last != radio) {
            loaded.set(None);
            draft.set(None);
            sections.set(Vec::new());
            section_edits.set(BTreeMap::new());
            notice.set(None);
        }
        if !ready() {
            return radio;
        }
        live.synced.track();
        let Some(id) = radio else { return radio };
        spawn_local(async move {
            if let Ok(read) = daemon::settings(id).await
                && live.selected.get_untracked() == Some(id)
            {
                let before = loaded.get_untracked();
                let now = draft.get_untracked();
                let merged = match (before, now) {
                    (Some(before), Some(now)) => merge(&before, &now, &read),
                    _ => read.clone(),
                };
                draft.set(Some(merged));
                loaded.set(Some(read));
            }
            if let Ok(read) = daemon::sections(id).await
                && live.selected.get_untracked() == Some(id)
            {
                sections.set(read);
            }
        });
        radio
    });

    // What a tab would change, if anything.
    let pending = move |tab: Tab| {
        let (Some(original), Some(now)) = (loaded.get(), draft.get()) else {
            return None;
        };
        let change = part_of(tab, changes(&original, &now));
        (!is_empty(&change)).then_some(change)
    };
    let edited = move |tab: Tab| match tab {
        Tab::Device | Tab::Modules => {
            let prefix = if tab == Tab::Modules {
                "module."
            } else {
                "config."
            };
            section_edits.with(|e| e.keys().any(|id| id.starts_with(prefix)))
        }
        _ => pending(tab).is_some(),
    };

    let save = move |tab: Tab| {
        let (Some(radio), Some(change)) = (live.selected.get_untracked(), pending(tab)) else {
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
            if applied.is_ok() {
                // Taken as the radio's until it is read back.
                if let Some(now) = draft.get_untracked() {
                    loaded.update(|l| {
                        if let Some(l) = l {
                            copy_part(tab, &now, l);
                        }
                    });
                }
            }
            notice.set(Some(applied.map(|a| a.reboots)));
            busy.set(false);
        });
    };
    let undo = move |tab: Tab| {
        if let Some(original) = loaded.get_untracked() {
            draft.update(|d| {
                if let Some(d) = d {
                    copy_part(tab, &original, d);
                }
            });
        }
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

    let tabs = TABS
        .into_iter()
        .map(|this| {
            let label = move || match this {
                Tab::User => t!(i18n, settings_tab_user).into_any(),
                Tab::Lora => t!(i18n, settings_tab_lora).into_any(),
                Tab::Channels => t!(i18n, settings_tab_channels).into_any(),
                Tab::Device => t!(i18n, settings_tab_device).into_any(),
                Tab::Modules => t!(i18n, settings_tab_modules).into_any(),
            };
            view! {
                <button
                    type="button"
                    role="tab"
                    class:on=move || tab.get() == this
                    class:edited=move || edited(this)
                    aria-selected=move || (tab.get() == this).to_string()
                    on:click=move |_| {
                        tab.set(this);
                        notice.set(None);
                    }
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    let at = move || TABS.iter().position(|t| *t == tab.get()).unwrap_or(0);

    // Built again only as the tab changes, not as anything in it is typed.
    let has_draft = Memo::new(move |_| draft.with(Option::is_some));
    let body = move || {
        let this = tab.get();
        if !has_draft.get() {
            return ().into_any();
        }
        let dirty = Signal::derive(move || pending(this).is_some());
        let bar = move || {
            view! {
                <SaveBar
                    dirty
                    busy=Signal::derive(move || busy.get())
                    save=move || save(this)
                    undo=move || undo(this)
                />
            }
        };
        match this {
            Tab::User => view! { <UserTab draft /> {bar()} }.into_any(),
            Tab::Lora => view! { <LoraTab draft /> {bar()} }.into_any(),
            Tab::Channels => view! { <ChannelsTab loaded notice /> }.into_any(),
            Tab::Device => {
                view! { <SectionsTab sections module=false edits=section_edits /> }.into_any()
            }
            Tab::Modules => {
                view! { <SectionsTab sections module=true edits=section_edits /> }.into_any()
            }
        }
    };

    view! {
        <Show
            when=shown
            fallback=move || {
                if live.selected_kind() == Some(RadioKind::Meshcore) {
                    view! { <p class="card dim">{t!(i18n, meshcore_not_yet)}</p> }.into_any()
                } else {
                    view! { <p class="card dim">{t!(i18n, settings_not_connected)}</p> }.into_any()
                }
            }
        >
            <div class="node-settings">
                <div
                    class="segmented settings-tabs"
                    role="tablist"
                    style=move || format!("--n: {}; --at: {}", TABS.len(), at())
                >
                    {tabs.clone()}
                </div>
                {notice_view}
                <div class="settings">{body}</div>
            </div>
        </Show>
    }
}

type Draft = RwSignal<Option<NodeSettings>>;

/// A field of the draft, read and written.
fn field<T: Clone + Default + Send + Sync + 'static>(
    draft: Draft,
    get: impl Fn(&NodeSettings) -> T + Copy + Send + Sync + 'static,
    set: impl Fn(&mut NodeSettings, T) + Copy + Send + Sync + 'static,
) -> (Signal<T>, impl Fn(T) + Copy) {
    // Nothing while there is no draft, as for a moment on leaving the radio.
    let read = Signal::derive(move || draft.with(|d| d.as_ref().map(get).unwrap_or_default()));
    let write = move |value: T| {
        draft.update(|d| {
            if let Some(d) = d {
                set(d, value);
            }
        });
    };
    (read, write)
}

/// A setting in a row: what it is and a word on it, and its control.
#[component]
fn Row(title: AnyView, hint: AnyView, children: Children) -> impl IntoView {
    view! {
        <div class="pref-row">
            <div class="pref-what">
                <span class="pref-title">{title}</span>
                <span class="dim">{hint}</span>
            </div>
            {children()}
        </div>
    }
}

#[component]
fn UserTab(draft: Draft) -> impl IntoView {
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
        <div class="preferences">
            <Row
                title=t!(i18n, settings_long_name).into_any()
                hint=t!(i18n, settings_long_name_hint).into_any()
            >
                <input
                    class="pref-control"
                    maxlength="39"
                    prop:value=long_name
                    on:input=move |ev| set_long(event_target_value(&ev))
                />
            </Row>
            <Row
                title=t!(i18n, settings_short_name).into_any()
                hint=t!(i18n, settings_short_name_hint).into_any()
            >
                <input
                    class="pref-control mono"
                    prop:value=short_name
                    on:input=move |ev| set_short(event_target_value(&ev))
                />
            </Row>
            <Row
                title=t!(i18n, settings_role).into_any()
                hint=t!(i18n, settings_role_hint).into_any()
            >
                <Choice class="pref-control" options=roles value=role set=set_role />
            </Row>
        </div>
    }
}

/// A bandwidth as the radio keeps it, in kHz: a few codes stand for a
/// fraction.
fn bandwidth_label(code: u32) -> String {
    let khz = match code {
        8 => "7.8".to_owned(),
        10 => "10.4".to_owned(),
        16 => "15.6".to_owned(),
        21 => "20.8".to_owned(),
        31 => "31.25".to_owned(),
        42 => "41.7".to_owned(),
        62 => "62.5".to_owned(),
        200 => "203.125".to_owned(),
        400 => "406.25".to_owned(),
        800 => "812.5".to_owned(),
        1600 => "1625".to_owned(),
        other => other.to_string(),
    };
    format!("{khz} kHz")
}

/// What a preset stands for, in the radio's region: on 2.4 GHz the standard
/// ones are wider.
fn preset_params(settings: &NodeSettings, preset: &str) -> Option<(u32, u32, u32)> {
    let params = settings.choices.preset_params.get(preset)?;
    let bandwidth = match (settings.lora.region == "LORA_24", params.bandwidth) {
        (true, 500) => 1600,
        (true, 250) => 800,
        (true, 125) => 400,
        (_, bandwidth) => bandwidth,
    };
    Some((bandwidth, params.spread_factor, params.coding_rate))
}

/// The bandwidths that can be set by hand in the radio's region.
fn bandwidths(settings: &NodeSettings) -> Vec<u32> {
    if settings.lora.region == "LORA_24" {
        settings.choices.wide_bandwidths.clone()
    } else {
        settings.choices.bandwidths.clone()
    }
}

/// Without a preset, it starts from the one it had, if what it was set to by
/// hand is nothing the region takes.
fn settle_by_hand(settings: &mut NodeSettings) {
    let fits = bandwidths(settings).contains(&settings.lora.bandwidth)
        && (5..=12).contains(&settings.lora.spread_factor)
        && (5..=8).contains(&settings.lora.coding_rate);
    if !settings.lora.use_preset && !fits {
        let preset = settings.lora.modem_preset.clone();
        if let Some((bandwidth, spread_factor, coding_rate)) = preset_params(settings, &preset) {
            settings.lora.bandwidth = bandwidth;
            settings.lora.spread_factor = spread_factor;
            settings.lora.coding_rate = coding_rate;
        }
    }
}

#[component]
fn LoraTab(draft: Draft) -> impl IntoView {
    let i18n = use_i18n();
    let (region, _) = field(draft, |d| d.lora.region.clone(), |d, v| d.lora.region = v);
    // Another region takes its own presets: the one there was, if it takes
    // it, or its default; and, by hand, its own bandwidths.
    let set_region = move |value: String| {
        draft.update(|d| {
            let Some(d) = d else { return };
            d.lora.region = value;
            let allowed = d
                .choices
                .presets_by_region
                .get(&d.lora.region)
                .cloned()
                .unwrap_or_default();
            if !allowed.contains(&d.lora.modem_preset)
                && let Some(first) = allowed.first()
            {
                d.lora.modem_preset = first.clone();
            }
            settle_by_hand(d);
        });
    };
    let (preset, set_preset) = field(
        draft,
        |d| d.lora.modem_preset.clone(),
        |d, v| d.lora.modem_preset = v,
    );
    let (use_preset, _) = field(draft, |d| d.lora.use_preset, |d, v| d.lora.use_preset = v);
    let flip_preset = move |_| {
        draft.update(|d| {
            if let Some(d) = d {
                d.lora.use_preset = !d.lora.use_preset;
                settle_by_hand(d);
            }
        });
    };
    let (hop_limit, set_hop_limit) =
        field(draft, |d| d.lora.hop_limit, |d, v| d.lora.hop_limit = v);
    let (tx, set_tx) = field(draft, |d| d.lora.tx_enabled, |d, v| d.lora.tx_enabled = v);
    let (bandwidth, set_bandwidth) =
        field(draft, |d| d.lora.bandwidth, |d, v| d.lora.bandwidth = v);
    let (spread_factor, set_spread_factor) = field(
        draft,
        |d| d.lora.spread_factor,
        |d, v| d.lora.spread_factor = v,
    );
    let (coding_rate, set_coding_rate) =
        field(draft, |d| d.lora.coding_rate, |d, v| d.lora.coding_rate = v);
    let regions = move || {
        draft.with(|d| {
            d.as_ref()
                .map(|d| d.choices.regions.clone())
                .unwrap_or_default()
        })
    };
    // Those its region takes.
    let presets = move || {
        draft.with(|d| {
            d.as_ref()
                .and_then(|d| d.choices.presets_by_region.get(&d.lora.region).cloned())
                .unwrap_or_default()
        })
    };
    let bandwidth_codes = move || draft.with(|d| d.as_ref().map(bandwidths).unwrap_or_default());

    let spread_factors = (5..=12u32)
        .map(|sf| {
            (
                Signal::derive(move || spread_factor.get() == sf),
                Box::new(move || set_spread_factor(sf)) as Box<dyn Fn() + Send + Sync>,
                sf.to_string().into_any(),
            )
        })
        .collect::<Vec<_>>();
    let coding_rates = (5..=8u32)
        .map(|cr| {
            (
                Signal::derive(move || coding_rate.get() == cr),
                Box::new(move || set_coding_rate(cr)) as Box<dyn Fn() + Send + Sync>,
                format!("4/{cr}").into_any(),
            )
        })
        .collect::<Vec<_>>();

    view! {
        <p class="dim tab-hint">{t!(i18n, settings_reboot_hint)}</p>
        <div class="preferences">
            <Row
                title=t!(i18n, settings_region).into_any()
                hint=t!(i18n, settings_region_hint).into_any()
            >
                <Choice class="pref-control" options=regions value=region set=set_region />
            </Row>
            <Row
                title=t!(i18n, settings_use_preset).into_any()
                hint=t!(i18n, settings_use_preset_hint).into_any()
            >
                <div class="pref-control switch-control">
                    <Switch
                        on=use_preset
                        label=Signal::derive(move || t_string!(i18n, settings_use_preset).to_owned())
                        flip=flip_preset
                    />
                </div>
            </Row>
            <div class:gone=move || !use_preset.get()>
                <Row
                    title=t!(i18n, settings_preset).into_any()
                    hint=t!(i18n, settings_preset_hint).into_any()
                >
                    {move || {
                        // Built again as the region changes what it offers.
                        let options = presets();
                        view! { <Choice class="pref-control" options=move || options.clone() value=preset set=set_preset /> }
                    }}
                </Row>
            </div>
            <div class:gone=move || use_preset.get()>
                <Row
                    title=t!(i18n, settings_bandwidth).into_any()
                    hint=t!(i18n, settings_bandwidth_hint).into_any()
                >
                    <select
                        class="pref-control"
                        prop:value=move || bandwidth.get().to_string()
                        on:change=move |ev| {
                            if let Ok(code) = event_target_value(&ev).parse() {
                                set_bandwidth(code);
                            }
                        }
                    >
                        {move || {
                            bandwidth_codes()
                                .into_iter()
                                .map(|code| {
                                    let selected = bandwidth.get_untracked() == code;
                                    view! {
                                        <option value=code.to_string() selected=selected>
                                            {bandwidth_label(code)}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                </Row>
                <Row
                    title=t!(i18n, settings_spread_factor).into_any()
                    hint=t!(i18n, settings_spread_factor_hint).into_any()
                >
                    {pills(spread_factors)}
                </Row>
                <Row
                    title=t!(i18n, settings_coding_rate).into_any()
                    hint=t!(i18n, settings_coding_rate_hint).into_any()
                >
                    {pills(coding_rates)}
                </Row>
            </div>
            <Row
                title=t!(i18n, settings_hop_limit).into_any()
                hint=t!(i18n, settings_hop_limit_hint).into_any()
            >
                <input
                    class="pref-control"
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
            </Row>
            <Row title=t!(i18n, settings_tx).into_any() hint=t!(i18n, settings_tx_hint).into_any()>
                <div class="pref-control switch-control">
                    <Switch
                        on=tx
                        label=Signal::derive(move || t_string!(i18n, settings_tx).to_owned())
                        flip=move |_| set_tx(!tx.get_untracked())
                    />
                </div>
            </Row>
        </div>
    }
}

/// A select over a list of codes, which shows them as they are: the radio's
/// own names for its roles, regions and presets.
#[component]
fn Choice(
    #[prop(optional)] class: &'static str,
    options: impl Fn() -> Vec<String> + Send + Sync + 'static,
    value: Signal<String>,
    set: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <select class=class prop:value=value on:change=move |ev| set(event_target_value(&ev))>
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
