// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radio's own settings, in tabs as the phone apps have them: who it is,
//! how it transmits, its channels, the rest of its configuration and its
//! modules. Read from it while it is connected, changed a tab at a time, and
//! read back once it has applied them.

use crate::app::{LinkLabel, Live};
use crate::channel_settings::ChannelsTab;
use crate::daemon::{self, Failure};
use crate::empty::{self, EmptyState};
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::names;
use crate::preferences::pills;
use crate::radios::Switch;
use crate::sections::{SaveBar, SectionEdits, SectionsTab, clear_when_synced};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    ErrorCode, LinkState, LoraAdvanced, NodeSettings, NodeSettingsChange, RadioId, RadioKind,
    RegionLimits, SettingsSection,
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
pub enum Tab {
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
    // Kept in the page's state, so that it opens where it was left, or where
    // a shortcut to a tab asks.
    let tab = live.settings_tab;

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
        // Not of a radio that has none of these, as MeshCore's yet.
        if !ready() || live.selected_kind() == Some(RadioKind::Meshcore) {
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
        // Role and LoRa changes reboot the radio, and so do being licensed
        // and the status message: say so before, not after.
        let owner_reboots = change.owner.as_ref().is_some_and(|owner| {
            loaded.with_untracked(|l| {
                l.as_ref().is_some_and(|l| {
                    owner.licensed != l.owner.licensed || owner.status != l.owner.status
                })
            })
        });
        if change.device.is_some() || change.lora.is_some() || owner_reboots {
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
                Tab::User => t_string!(i18n, settings_tab_user).to_string(),
                Tab::Lora => t_string!(i18n, settings_tab_lora).to_string(),
                Tab::Channels => t_string!(i18n, settings_tab_channels).to_string(),
                Tab::Device => t_string!(i18n, settings_tab_device).to_string(),
                Tab::Modules => t_string!(i18n, settings_tab_modules).to_string(),
            };
            view! {
                // Its label again, in bold and unseen, so that it is as wide
                // picked as not.
                <button
                    type="button"
                    role="tab"
                    data-label=label
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
    let at = Memo::new(move |_| TABS.iter().position(|t| *t == tab.get()).unwrap_or(0));

    // Each tab as wide as its word, the room left shared out: the pill goes
    // where the one picked is and as wide, measured once it is laid out, as
    // the tab, the language or the window change.
    let strip = NodeRef::<leptos::html::Div>::new();
    let pill = crate::preferences::slide(strip, at);

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
            // Its settings, from a link too.
            Tab::Lora => view! {
                <LoraTab draft />
                <SaveBar
                    dirty
                    busy=Signal::derive(move || busy.get())
                    save=move || save(this)
                    undo=move || undo(this)
                >
                    <crate::links::LoraLink />
                </SaveBar>
            }
            .into_any(),
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
                let radio = live.selected_radio();
                let name = radio.as_ref().map(|r| r.name.clone()).unwrap_or_default();
                match radio {
                    // A MeshCore radio's settings are its own, while it is
                    // connected.
                    Some(radio)
                        if radio.kind == RadioKind::Meshcore
                            && matches!(radio.link, LinkState::Ready | LinkState::Syncing) =>
                    {
                        view! { <crate::meshcore_settings::MeshcoreSettingsView /> }.into_any()
                    }
                    // Switched off: it can be switched on from here.
                    Some(radio) if radio.link == LinkState::Disabled => view! {
                        <EmptyState
                            icon=empty::POWER
                            title=t!(i18n, empty_disabled_title, name = name.clone()).into_any()
                            text=t!(i18n, empty_disabled_text).into_any()
                        >
                            <button
                                type="button"
                                class="primary"
                                on:click=move |_| crate::radios::quick(live, &radio, |s| s.enabled = true)
                            >
                                {t!(i18n, empty_enable)}
                            </button>
                        </EmptyState>
                    }
                    .into_any(),
                    Some(radio) => {
                        let link = Signal::stored(radio.link);
                        view! {
                            <EmptyState
                                icon=empty::PLUG_OFF
                                title=t!(i18n, empty_offline_title, name = name.clone()).into_any()
                                text=t!(i18n, empty_offline_text).into_any()
                            >
                                <span class=move || format!("badge {}", live.link_class(link.get()))>
                                    <LinkLabel link />
                                </span>
                            </EmptyState>
                        }
                        .into_any()
                    }
                    None => ().into_any(),
                }
            }
        >
            <div class="node-settings">
                <div
                    class="segmented fitted settings-tabs"
                    role="tablist"
                    node_ref=strip
                    style=move || format!("--n: {}; --at: {}; {}", TABS.len(), at.get(), pill.get())
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
    let has_status =
        Memo::new(move |_| draft.with(|d| d.as_ref().is_some_and(|d| d.owner.status.is_some())));
    let (status, set_status) = field(
        draft,
        |d| d.owner.status.clone().unwrap_or_default(),
        |d, v| {
            if d.owner.status.is_some() {
                d.owner.status = Some(v);
            }
        },
    );
    let (unmessagable, set_unmessagable) = field(
        draft,
        |d| d.owner.unmessagable.unwrap_or(false),
        |d, v| d.owner.unmessagable = Some(v),
    );
    let (licensed, set_licensed) = field(
        draft,
        |d| d.owner.licensed.unwrap_or(false),
        |d, v| d.owner.licensed = Some(v),
    );
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
                <Choice
                    class="pref-control"
                    options=roles
                    value=role
                    set=set_role
                    label=names::role
                />
            </Row>
            // Only where the firmware has them.
            <div class:gone=move || !has_status.get()>
                <Row
                    title=t!(i18n, settings_status).into_any()
                    hint=t!(i18n, settings_status_hint).into_any()
                >
                    <input
                        class="pref-control"
                        maxlength="79"
                        prop:value=move || status.get()
                        on:input=move |ev| set_status(event_target_value(&ev))
                    />
                </Row>
            </div>
            <Row
                title=t!(i18n, settings_unmessagable).into_any()
                hint=t!(i18n, settings_unmessagable_hint).into_any()
            >
                <div class="pref-control switch-control">
                    <Switch
                        on=unmessagable
                        label=Signal::derive(move || t_string!(i18n, settings_unmessagable).to_owned())
                        flip=move |_| set_unmessagable(!unmessagable.get_untracked())
                    />
                </div>
            </Row>
            <Row
                title=t!(i18n, settings_licensed).into_any()
                hint=t!(i18n, settings_licensed_hint).into_any()
            >
                <div class="pref-control switch-control">
                    <Switch
                        on=licensed
                        label=Signal::derive(move || t_string!(i18n, settings_licensed).to_owned())
                        flip=move |_| set_licensed(!licensed.get_untracked())
                    />
                </div>
            </Row>
        </div>
    }
}

/// A bandwidth as the radio keeps it, in kHz: a few codes stand for a
/// fraction.
pub fn bandwidth_label(code: u32) -> String {
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
    // In the firmware's order, the most used first.
    let regions = move || {
        let codes = draft.with(|d| {
            d.as_ref()
                .map(|d| d.choices.regions.clone())
                .unwrap_or_default()
        });
        names::placed(codes, "UNSET", &region.get())
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
                Signal::stored(sf.to_string()),
            )
        })
        .collect::<Vec<_>>();
    let coding_rates = (5..=8u32)
        .map(|cr| {
            (
                Signal::derive(move || coding_rate.get() == cr),
                Box::new(move || set_coding_rate(cr)) as Box<dyn Fn() + Send + Sync>,
                Signal::stored(format!("4/{cr}")),
            )
        })
        .collect::<Vec<_>>();

    // A link's, once taken: in the form, to be looked over and saved, what
    // few need to touch as it is.
    let live = expect_context::<Live>();
    Effect::new(move |_| {
        if let Some(theirs) = live.lora_from_link.get() {
            live.lora_from_link.set(None);
            draft.update(|d| {
                if let Some(d) = d {
                    d.lora = meshctl_api::LoraSettings {
                        advanced: d.lora.advanced.clone(),
                        ..theirs
                    };
                }
            });
        }
    });

    view! {
        <p class="dim tab-hint">{t!(i18n, settings_reboot_hint)}</p>
        <div class="preferences">
            <Row
                title=t!(i18n, settings_region).into_any()
                hint=t!(i18n, settings_region_hint).into_any()
            >
                <Choice
                    class="pref-control"
                    options=regions
                    value=region
                    set=set_region
                    label=names::region
                />
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
                        view! {
                            <Choice
                                class="pref-control"
                                options=move || options.clone()
                                value=preset
                                set=set_preset
                                label=names::preset
                            />
                        }
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
        <LoraAdvancedRows draft />
    }
}

const CHEVRON: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");

/// A bandwidth's code in kHz.
fn khz(code: u32) -> f64 {
    match code {
        8 => 7.8,
        10 => 10.4,
        16 => 15.6,
        21 => 20.8,
        31 => 31.25,
        42 => 41.7,
        62 => 62.5,
        200 => 203.125,
        400 => 406.25,
        800 => 812.5,
        1600 => 1625.0,
        other => f64::from(other),
    }
}

/// What the radio's region and bandwidth make of its frequency: how many
/// slots there are, and the frequency of each, in MHz, from slot 1.
struct Band {
    limits: RegionLimits,
    width: f64,
    half: f64,
    slots: u32,
    /// The slot it uses when none is set, from 1.
    auto_slot: u32,
}

/// The firmware's hash for picking a frequency slot by a channel's name.
fn djb2(text: &str) -> u32 {
    text.bytes().fold(5381_u32, |hash, byte| {
        hash.wrapping_shl(5)
            .wrapping_add(hash)
            .wrapping_add(u32::from(byte))
    })
}

impl Band {
    fn of(settings: &NodeSettings) -> Option<Self> {
        let limits = *settings.choices.region_limits.get(&settings.lora.region)?;
        let bandwidth = if settings.lora.use_preset {
            preset_params(settings, &settings.lora.modem_preset)?.0
        } else {
            settings.lora.bandwidth
        };
        let width = limits.spacing + limits.padding * 2.0 + khz(bandwidth) / 1000.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let slots = if settings.lora.region == "UNSET" {
            0
        } else {
            ((limits.frequency_end - limits.frequency_start + limits.spacing) / width).round()
                as u32
        };
        // As the firmware picks it: its region's own, or by the primary
        // channel's name, which for an unnamed one is its preset's.
        let auto_slot = if limits.default_slot > 0 && limits.default_slot <= slots {
            limits.default_slot
        } else if slots > 0 {
            let primary = settings
                .channels
                .first()
                .map(|c| c.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| {
                    if settings.lora.use_preset {
                        crate::chat::preset_name(&settings.lora.modem_preset)
                    } else {
                        "Custom".to_owned()
                    }
                });
            djb2(&primary) % slots + 1
        } else {
            1
        };
        Some(Self {
            limits,
            width,
            half: khz(bandwidth) / 2000.0,
            slots,
            auto_slot,
        })
    }

    fn frequency(&self, slot: u32) -> f64 {
        self.limits.frequency_start
            + self.half
            + self.limits.padding
            + f64::from(slot.saturating_sub(1)) * self.width
    }
}

/// A field of the advanced part of the draft's LoRa, read and written.
fn advanced<T: Clone + Default + Send + Sync + 'static>(
    draft: Draft,
    get: impl Fn(&LoraAdvanced) -> T + Copy + Send + Sync + 'static,
    set: impl Fn(&mut LoraAdvanced, T) + Copy + Send + Sync + 'static,
) -> (Signal<T>, impl Fn(T) + Copy) {
    field(
        draft,
        move |d| d.lora.advanced.as_ref().map(get).unwrap_or_default(),
        move |d, v| {
            if let Some(advanced) = d.lora.advanced.as_mut() {
                set(advanced, v);
            }
        },
    )
}

/// What few need to touch of how the radio transmits: folded until asked
/// for, held to what its region allows.
#[component]
fn LoraAdvancedRows(draft: Draft) -> impl IntoView {
    let i18n = use_i18n();
    let open = RwSignal::new(false);
    let licensed =
        move || draft.with(|d| d.as_ref().is_some_and(|d| d.owner.licensed == Some(true)));
    let band = Memo::new(move |_| {
        draft.with(|d| d.as_ref().and_then(Band::of).map(|b| (b.limits, b.slots)))
    });
    let (power, set_power) = advanced(draft, |a| a.tx_power, |a, v| a.tx_power = v);
    let (slot, set_slot) = advanced(draft, |a| a.frequency_slot, |a, v| a.frequency_slot = v);
    let (frequency, set_frequency) = advanced(
        draft,
        |a| a.override_frequency,
        |a, v| a.override_frequency = v,
    );
    let (offset, set_offset) =
        advanced(draft, |a| a.frequency_offset, |a, v| a.frequency_offset = v);
    let (duty, set_duty) = advanced(
        draft,
        |a| a.override_duty_cycle,
        |a, v| a.override_duty_cycle = v,
    );
    let (boosted, set_boosted) =
        advanced(draft, |a| a.rx_boosted_gain, |a, v| a.rx_boosted_gain = v);
    let (ok_mqtt, set_ok_mqtt) = advanced(draft, |a| a.ok_to_mqtt, |a, v| a.ok_to_mqtt = v);
    let (ignore_mqtt, set_ignore_mqtt) =
        advanced(draft, |a| a.ignore_mqtt, |a, v| a.ignore_mqtt = v);
    let (fan_off, set_fan_off) =
        advanced(draft, |a| a.pa_fan_disabled, |a, v| a.pa_fan_disabled = v);

    // Another region or bandwidth may have fewer slots, or allow less power:
    // what it would not take is brought within.
    Effect::new(move |_| {
        let Some((limits, slots)) = band.get() else {
            return;
        };
        if slot.get_untracked() > slots {
            set_slot(0);
        }
        if !licensed() && power.get_untracked() > limits.power_limit as i32 {
            set_power(limits.power_limit as i32);
        }
    });

    let power_limit = move || band.get().map_or(0, |(l, _)| l.power_limit);
    // A licensed owner's radio is not held to its region's power: as far as
    // its hardware goes, which the radio itself keeps to.
    let power_most = move || if licensed() { 30 } else { power_limit() };
    let slots = move || band.get().map_or(0, |(_, s)| s);
    let slot_hint = move || format!("{} {}.", t_string!(i18n, settings_slot_hint), slots());
    // What the radio will use when left to it: the slot it picks, and that
    // slot's frequency, or the one set's.
    let auto_slot = move || {
        draft
            .with(|d| d.as_ref().and_then(Band::of))
            .map_or(1, |b| b.auto_slot)
    };
    let auto_frequency = move || {
        let at = slot.get();
        draft
            .with(|d| d.as_ref().and_then(Band::of))
            .map(|band| {
                let at = if at > 0 { at } else { band.auto_slot };
                format!("{:.3}", band.frequency(at))
            })
            .unwrap_or_default()
    };
    let band_hint = move || {
        band.get().map_or_else(String::new, |(l, _)| {
            format!(
                "{} {:.3}–{:.3} MHz.",
                t_string!(i18n, settings_frequency_hint),
                l.frequency_start,
                l.frequency_end
            )
        })
    };
    let limited = move || band.get().is_some_and(|(l, _)| l.duty_cycle < 100.0);
    let duty_hint = move || {
        let duty = band.get().map_or(100.0, |(l, _)| l.duty_cycle);
        format!("{} {duty} %.", t_string!(i18n, settings_duty_hint))
    };
    let number = |text: String| text.trim().replace(',', ".");
    let switch_row = move |title: AnyView,
                           hint: AnyView,
                           on: Signal<bool>,
                           set: Box<dyn Fn(bool) + Send + Sync>,
                           label: Signal<String>| {
        let set = std::sync::Arc::new(set);
        view! {
            <Row title hint>
                <div class="pref-control switch-control">
                    <Switch
                        on
                        label
                        flip=move |_| set(!on.get_untracked())
                    />
                </div>
            </Row>
        }
    };

    view! {
        <button
            type="button"
            class="advanced-toggle"
            class:open=move || open.get()
            aria-expanded=move || open.get().to_string()
            on:click=move |_| open.update(|o| *o = !*o)
        >
            <span class="pref-what">
                <span class="pref-title">{t!(i18n, settings_advanced)}</span>
                <span class="dim">{t!(i18n, settings_advanced_hint)}</span>
            </span>
            <span class="chevron" inner_html=CHEVRON></span>
        </button>
        <div class="preferences" class:gone=move || !open.get()>
            <Row
                title=t!(i18n, settings_tx_power).into_any()
                hint=view! { {t!(i18n, settings_tx_power_hint)} " " {move || power_limit()} " dBm." }.into_any()
            >
                <input
                    class="pref-control"
                    type="number"
                    min="0"
                    max=move || power_limit().to_string()
                    prop:value=move || power.get().to_string()
                    on:input=move |ev| {
                        // Not past what the region allows, so as not to send
                        // what the radio would cut down.
                        if let Ok(dbm) = number(event_target_value(&ev)).parse::<i32>() {
                            set_power(dbm.clamp(0, power_most() as i32));
                        }
                    }
                />
            </Row>
            <Row title=t!(i18n, settings_slot).into_any() hint=view! { {slot_hint} }.into_any()>
                // Left empty, the radio's own, which it shows greyed, as "auto".
                <div class="pref-control auto-field" class:auto=move || slot.get() == 0>
                    <input
                        type="number"
                        min="1"
                        max=move || slots().to_string()
                        disabled=move || frequency.get() != 0.0
                        placeholder=move || auto_slot().to_string()
                        prop:value=move || {
                            let at = slot.get();
                            if at == 0 { String::new() } else { at.to_string() }
                        }
                        on:input=move |ev| {
                            let text = number(event_target_value(&ev));
                            if text.is_empty() {
                                set_slot(0);
                            } else if let Ok(slot) = text.parse::<u32>() {
                                set_slot(slot.min(slots()));
                            }
                        }
                    />
                    <span class="auto-tag">"auto"</span>
                </div>
            </Row>
            <Row title=t!(i18n, settings_frequency).into_any() hint=view! { {band_hint} }.into_any()>
                <div class="pref-control auto-field" class:auto=move || frequency.get() == 0.0>
                    <input
                        type="number"
                        step="0.001"
                        min="0"
                        placeholder=auto_frequency
                        prop:value=move || {
                            let mhz = frequency.get();
                            if mhz == 0.0 { String::new() } else { mhz.to_string() }
                        }
                        on:change=move |ev| {
                            let text = number(event_target_value(&ev));
                            if text.is_empty() {
                                set_frequency(0.0);
                            } else if let Ok(mhz) = text.parse() {
                                set_frequency(mhz);
                            }
                        }
                    />
                    <span class="auto-tag">"auto"</span>
                </div>
            </Row>
            <Row
                title=t!(i18n, settings_offset).into_any()
                hint=t!(i18n, settings_offset_hint).into_any()
            >
                <input
                    class="pref-control"
                    type="number"
                    step="0.001"
                    min="-1"
                    max="1"
                    prop:value=move || offset.get().to_string()
                    on:change=move |ev| {
                        if let Ok(mhz) = number(event_target_value(&ev)).parse() {
                            set_offset(mhz);
                        }
                    }
                />
            </Row>
            <div class:gone=move || !limited()>
                {switch_row(
                    t!(i18n, settings_duty).into_any(),
                    view! { {duty_hint} }.into_any(),
                    duty,
                    Box::new(set_duty),
                    Signal::derive(move || t_string!(i18n, settings_duty).to_owned()),
                )}
            </div>
            {switch_row(
                t!(i18n, settings_boosted).into_any(),
                t!(i18n, settings_boosted_hint).into_any(),
                boosted,
                Box::new(set_boosted),
                Signal::derive(move || t_string!(i18n, settings_boosted).to_owned()),
            )}
            {switch_row(
                t!(i18n, settings_ok_mqtt).into_any(),
                t!(i18n, settings_ok_mqtt_hint).into_any(),
                ok_mqtt,
                Box::new(set_ok_mqtt),
                Signal::derive(move || t_string!(i18n, settings_ok_mqtt).to_owned()),
            )}
            {switch_row(
                t!(i18n, settings_ignore_mqtt).into_any(),
                t!(i18n, settings_ignore_mqtt_hint).into_any(),
                ignore_mqtt,
                Box::new(set_ignore_mqtt),
                Signal::derive(move || t_string!(i18n, settings_ignore_mqtt).to_owned()),
            )}
            {switch_row(
                t!(i18n, settings_fan).into_any(),
                t!(i18n, settings_fan_hint).into_any(),
                fan_off,
                Box::new(set_fan_off),
                Signal::derive(move || t_string!(i18n, settings_fan).to_owned()),
            )}
        </div>
    }
}

/// A select over a list of codes: the radio's own names for its roles,
/// regions and presets, as they are, or as `label` reads them. The one the
/// radio has is there even if it is not one offered, as a role deprecated:
/// it is not lost, only not offered again once left.
#[component]
fn Choice(
    #[prop(optional)] class: &'static str,
    options: impl Fn() -> Vec<String> + Send + Sync + 'static,
    value: Signal<String>,
    set: impl Fn(String) + Copy + Send + Sync + 'static,
    #[prop(optional)] label: Option<fn(Locale, &str) -> String>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <select class=class prop:value=value on:change=move |ev| set(event_target_value(&ev))>
            {move || {
                let locale = i18n.get_locale();
                let mut codes = options();
                let current = value.get_untracked();
                if !current.is_empty() && !codes.contains(&current) {
                    codes.push(current);
                }
                codes
                    .into_iter()
                    .map(|code| {
                        let selected = value.get_untracked() == code;
                        let shown = label.map_or_else(|| code.clone(), |label| label(locale, &code));
                        view! {
                            <option value=code.clone() selected=selected>
                                {shown}
                            </option>
                        }
                    })
                    .collect_view()
            }}
        </select>
    }
}

#[cfg(test)]
mod tests {
    use super::djb2;

    #[test]
    fn longfast_in_the_us_is_slot_20() {
        // 906.875 MHz, as the apps say for LongFast in the US.
        assert_eq!(djb2("LongFast") % 104 + 1, 20);
    }
}
