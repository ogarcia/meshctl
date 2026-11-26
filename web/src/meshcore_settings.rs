// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A MeshCore radio's own settings, which are not Meshtastic's: what it
//! advertises and where, how it transmits, and the rest. Saved whole, as
//! the radio applies each at once; read back from it after.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::position::{PositionInputs, coordinate, written};
use crate::radios::Switch;
use crate::sections::SaveBar;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    ErrorCode, MeshcoreAutoAdd, MeshcorePosition, MeshcoreSettings, MeshcoreTuning, TelemetryShare,
};

/// Which part of the settings is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshcoreTab {
    /// What it advertises itself as, and where.
    User,
    /// How it transmits.
    Radio,
    /// Its channels, each saved on its own.
    Channels,
    /// The rest.
    Advanced,
}

pub const TABS: [MeshcoreTab; 4] = [
    MeshcoreTab::User,
    MeshcoreTab::Radio,
    MeshcoreTab::Channels,
    MeshcoreTab::Advanced,
];

/// The bandwidths MeshCore radios use, in Hz.
const BANDWIDTHS: [u32; 10] = [
    7_800, 10_400, 15_600, 20_800, 31_250, 41_700, 62_500, 125_000, 250_000, 500_000,
];

/// Whether a tab's settings differ between two.
fn differs(tab: MeshcoreTab, a: &MeshcoreSettings, b: &MeshcoreSettings) -> bool {
    match tab {
        MeshcoreTab::User => a.name != b.name || a.position != b.position,
        MeshcoreTab::Radio => {
            (a.frequency, a.bandwidth, a.spreading_factor, a.coding_rate)
                != (b.frequency, b.bandwidth, b.spreading_factor, b.coding_rate)
                || a.tx_power != b.tx_power
                || a.repeat != b.repeat
                || a.default_scope != b.default_scope
        }
        MeshcoreTab::Channels => false,
        MeshcoreTab::Advanced => {
            a.manual_add_contacts != b.manual_add_contacts
                || a.share_position != b.share_position
                || a.telemetry != b.telemetry
                || a.multi_acks != b.multi_acks
                || a.path_hash_bytes != b.path_hash_bytes
                || a.auto_add != b.auto_add
                || a.tuning != b.tuning
        }
    }
}

/// The hop limits offered for auto-add, past straight: the one it has
/// joins them if it is another.
const HOPS: [u8; 11] = [1, 2, 3, 4, 5, 6, 7, 8, 16, 32, 63];

/// The frequencies a radio repeats on, as people say them: `433,000,
/// 869,495 o 918,000`.
fn frequencies_words(settings: &MeshcoreSettings, or: &str) -> String {
    let ranges: Vec<String> = settings
        .repeat_frequencies
        .iter()
        .map(|r| {
            if r.from == r.to {
                mhz(r.from)
            } else {
                format!("{}–{}", mhz(r.from), mhz(r.to))
            }
        })
        .collect();
    match ranges.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} {or} {last}", rest.join(", ")),
        Some((last, _)) => last.clone(),
        None => String::new(),
    }
}

/// A number as written, either decimal mark.
fn decimal(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse().ok()
}

/// A frequency in kHz, as people write it in MHz.
fn mhz(khz: u32) -> String {
    format!("{:.3}", f64::from(khz) / 1000.0)
}

/// A bandwidth in Hz, in kHz as people write it.
fn bandwidth_label(hz: u32) -> String {
    let khz = f64::from(hz) / 1000.0;
    if hz.is_multiple_of(1000) {
        format!("{khz:.0} kHz")
    } else {
        format!("{khz} kHz")
    }
}

type Draft = RwSignal<Option<MeshcoreSettings>>;

/// A field of the draft, read and written.
fn field<T: Clone + Default + Send + Sync + 'static>(
    draft: Draft,
    get: impl Fn(&MeshcoreSettings) -> T + Copy + Send + Sync + 'static,
    set: impl Fn(&mut MeshcoreSettings, T) + Copy + Send + Sync + 'static,
) -> (Signal<T>, impl Fn(T) + Copy) {
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
pub fn MeshcoreSettingsView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let loaded = RwSignal::new(None::<MeshcoreSettings>);
    let draft: Draft = RwSignal::new(None);
    let notice = RwSignal::new(None::<Result<(), Failure>>);
    // Said until the tab changes or it is saved again: the radio is read
    // back at once, which would take it away before it is seen.
    let busy = RwSignal::new(false);
    let tab = live.meshcore_tab;

    // Read on opening, and again each time the radio is read: after a change
    // above all. What is being edited stays, unless nothing was.
    Effect::new(move |_| {
        live.synced.track();
        let Some(radio) = live.selected.get() else {
            return;
        };
        spawn_local(async move {
            if let Ok(read) = daemon::meshcore_settings(radio).await
                && live.selected.get_untracked() == Some(radio)
            {
                let clean = draft.with_untracked(|d| d.is_none() || *d == loaded.get_untracked());
                if clean {
                    draft.set(Some(read.clone()));
                }
                loaded.set(Some(read));
            }
        });
    });

    let dirty = Signal::derive(move || {
        loaded.with(|l| draft.with(|d| l.is_some() && d.is_some() && l != d))
    });
    let edited = move |this: MeshcoreTab| {
        loaded.with(|l| draft.with(|d| matches!((l, d), (Some(l), Some(d)) if differs(this, l, d))))
    };
    let save = move || {
        let (Some(radio), Some(wanted), Some(before)) = (
            live.selected.get_untracked(),
            draft.get_untracked(),
            loaded.get_untracked(),
        ) else {
            return;
        };
        // Another frequency or modulation leaves it hearing only those on
        // the same: said before, not after.
        let moved = (
            wanted.frequency,
            wanted.bandwidth,
            wanted.spreading_factor,
            wanted.coding_rate,
        ) != (
            before.frequency,
            before.bandwidth,
            before.spreading_factor,
            before.coding_rate,
        );
        if moved {
            let sure = web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(t_string!(i18n, mc_radio_confirm))
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
            let done = daemon::set_meshcore_settings(radio, &wanted).await;
            if done.is_ok() {
                loaded.set(Some(wanted));
            }
            notice.set(Some(done));
            busy.set(false);
        });
    };
    let undo = move || draft.set(loaded.get_untracked());

    let notice_view = move || {
        notice.get().map(|outcome| match outcome {
            Ok(()) => view! { <p class="card note">{t!(i18n, mc_applied)}</p> }.into_any(),
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
                MeshcoreTab::User => t_string!(i18n, settings_tab_user).to_string(),
                MeshcoreTab::Radio => t_string!(i18n, mc_tab_radio).to_string(),
                MeshcoreTab::Channels => t_string!(i18n, settings_tab_channels).to_string(),
                MeshcoreTab::Advanced => t_string!(i18n, mc_tab_advanced).to_string(),
            };
            view! {
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
    let strip = NodeRef::<leptos::html::Div>::new();
    let pill = crate::preferences::slide(strip, at);

    // Built again only as the tab changes, not as anything in it is typed.
    let has_draft = Memo::new(move |_| draft.with(Option::is_some));
    let body = move || {
        let this = tab.get();
        if !has_draft.get() {
            return ().into_any();
        }
        let bar = view! {
            <SaveBar
                dirty
                busy=Signal::derive(move || busy.get())
                save=save
                undo=undo
            />
        };
        match this {
            MeshcoreTab::User => view! { <UserTab draft /> {bar} }.into_any(),
            MeshcoreTab::Radio => view! { <RadioTab draft /> {bar} }.into_any(),
            // Each channel saved on its own, as it is edited.
            MeshcoreTab::Channels => {
                view! { <crate::meshcore_channels::MeshcoreChannelsTab /> }.into_any()
            }
            MeshcoreTab::Advanced => view! { <AdvancedTab draft /> {bar} }.into_any(),
        }
    };

    view! {
        <div class="node-settings">
            <div
                class="segmented fitted settings-tabs"
                role="tablist"
                node_ref=strip
                style=move || format!("--n: {}; --at: {}; {}", TABS.len(), at.get(), pill.get())
            >
                {tabs}
            </div>
            {notice_view}
            <div class="settings">{body}</div>
        </div>
    }
}

#[component]
fn UserTab(draft: Draft) -> impl IntoView {
    let i18n = use_i18n();
    let (name, set_name) = field(draft, |d| d.name.clone(), |d, v| d.name = v);
    let (position, set_position) = field(draft, |d| d.position, |d, v| d.position = v);
    // Each coordinate as written, so that half a number is not lost; the
    // position changes once both read, and goes once both are empty.
    let latitude = RwSignal::new(
        position
            .get_untracked()
            .map_or_else(String::new, |p| written(p.latitude)),
    );
    let longitude = RwSignal::new(
        position
            .get_untracked()
            .map_or_else(String::new, |p| written(p.longitude)),
    );
    let put = move || {
        let (lat, lon) = (latitude.get_untracked(), longitude.get_untracked());
        if lat.trim().is_empty() && lon.trim().is_empty() {
            set_position(None);
        } else if let (Some(latitude), Some(longitude)) = (coordinate(&lat), coordinate(&lon)) {
            set_position(Some(MeshcorePosition {
                latitude,
                longitude,
            }));
        }
    };
    view! {
        <div class="preferences">
            <Row title=t!(i18n, mc_name).into_any() hint=t!(i18n, mc_name_hint).into_any()>
                <input
                    class="pref-control"
                    maxlength="31"
                    prop:value=name
                    on:input=move |ev| set_name(event_target_value(&ev))
                />
            </Row>
            <Row title=t!(i18n, mc_position).into_any() hint=t!(i18n, mc_position_hint).into_any()>
                <PositionInputs latitude longitude changed=move |()| put() />
            </Row>
        </div>
    }
}

#[component]
fn RadioTab(draft: Draft) -> impl IntoView {
    let i18n = use_i18n();
    let (frequency, set_frequency) = field(draft, |d| d.frequency, |d, v| d.frequency = v);
    let (bandwidth, set_bandwidth) = field(draft, |d| d.bandwidth, |d, v| d.bandwidth = v);
    let (sf, set_sf) = field(draft, |d| d.spreading_factor, |d, v| d.spreading_factor = v);
    let (cr, set_cr) = field(draft, |d| d.coding_rate, |d, v| d.coding_rate = v);
    let (power, set_power) = field(draft, |d| d.tx_power, |d, v| d.tx_power = v);
    let (max_power, _) = field(draft, |d| d.max_tx_power, |_, _: u8| {});
    let (repeat, set_repeat) = field(draft, |d| d.repeat, |d, v| d.repeat = v);
    let (scopes, _) = field(draft, |d| d.scopes, |_, _: bool| {});
    let (scope, set_scope) = field(
        draft,
        |d| d.default_scope.clone(),
        |d, v| d.default_scope = v,
    );
    let (repeat_on, _) = field(draft, move |d| frequencies_words(d, "o"), |_, _: String| {});
    // The region as written, `#` and all; kept without it.
    let region = RwSignal::new(
        draft
            .with_untracked(|d| d.as_ref().and_then(|d| d.default_scope.clone()))
            .map_or_else(String::new, |name| format!("#{name}")),
    );
    // A region found, picked: written as typed, `#` and all.
    let pick_region = move |name: Option<String>| {
        region.set(name.as_ref().map_or_else(String::new, |n| format!("#{n}")));
        set_scope(name);
    };
    // The frequency as written, read into kHz once it is a number.
    let typed = RwSignal::new(mhz(frequency.get_untracked()));
    let bandwidths = move || {
        let mut all = BANDWIDTHS.to_vec();
        let now = bandwidth.get_untracked();
        if !all.contains(&now) {
            all.push(now);
            all.sort_unstable();
        }
        all
    };
    view! {
        <div class="preferences">
            <Row title=t!(i18n, mc_frequency).into_any() hint=t!(i18n, mc_frequency_hint).into_any()>
                <span class="pref-control mc-unit">
                    <input
                        inputmode="decimal"
                        prop:value=move || typed.get()
                        on:input=move |ev| {
                            let text = event_target_value(&ev);
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            if let Ok(mhz) = text.trim().replace(',', ".").parse::<f64>()
                                && mhz > 0.0
                            {
                                set_frequency((mhz * 1000.0).round() as u32);
                            }
                            typed.set(text);
                        }
                    />
                    <span class="dim">"MHz"</span>
                </span>
            </Row>
            <Row title=t!(i18n, mc_bandwidth).into_any() hint=t!(i18n, mc_bandwidth_hint).into_any()>
                <select
                    class="pref-control"
                    on:change=move |ev| {
                        if let Ok(hz) = event_target_value(&ev).parse() {
                            set_bandwidth(hz);
                        }
                    }
                >
                    {bandwidths()
                        .into_iter()
                        .map(|hz| {
                            view! {
                                <option value=hz.to_string() selected=move || bandwidth.get() == hz>
                                    {bandwidth_label(hz)}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </Row>
            <Row title=t!(i18n, mc_sf).into_any() hint=t!(i18n, mc_sf_hint).into_any()>
                <select
                    class="pref-control"
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse() {
                            set_sf(v);
                        }
                    }
                >
                    {(5..=12u8)
                        .map(|v| {
                            view! {
                                <option value=v.to_string() selected=move || sf.get() == v>
                                    {format!("SF{v}")}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </Row>
            <Row title=t!(i18n, mc_cr).into_any() hint=t!(i18n, mc_cr_hint).into_any()>
                <select
                    class="pref-control"
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse() {
                            set_cr(v);
                        }
                    }
                >
                    {(5..=8u8)
                        .map(|v| {
                            view! {
                                <option value=v.to_string() selected=move || cr.get() == v>
                                    {format!("4/{v}")}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </Row>
            <Row
                title=t!(i18n, mc_tx_power).into_any()
                hint=view! {
                    {move || t_string!(i18n, mc_tx_power_hint).replace("{max}", &max_power.get().to_string())}
                }
                .into_any()
            >
                <span class="pref-control mc-unit">
                    <span class="number-field">
                        <input
                            type="number"
                            min="-9"
                            max=move || max_power.get().to_string()
                            prop:value=move || power.get().to_string()
                            on:change=move |ev| {
                                if let Ok(dbm) = event_target_value(&ev).trim().parse::<i8>() {
                                    set_power(dbm);
                                }
                            }
                        />
                        <crate::stepper::Stepper />
                    </span>
                    <span class="dim">"dBm"</span>
                </span>
            </Row>
            // Only where the firmware has it.
            <div class:gone=move || !scopes.get()>
                <Row title=t!(i18n, mc_scope).into_any() hint=t!(i18n, mc_scope_hint).into_any()>
                    <input
                        class="pref-control"
                        maxlength="31"
                        placeholder="#galicia"
                        prop:value=move || region.get()
                        on:input=move |ev| {
                            let text = event_target_value(&ev);
                            let name = text.trim();
                            let name = name.strip_prefix('#').unwrap_or(name).trim();
                            set_scope((!name.is_empty()).then(|| name.to_owned()));
                            region.set(text);
                        }
                    />
                </Row>
                <crate::meshcore_regions::RegionFinder
                    scope
                    pick=pick_region
                />
            </div>
            <div class:gone=move || repeat.get().is_none()>
                <Row
                    title=t!(i18n, mc_repeat).into_any()
                    hint=view! { {move || t_string!(i18n, mc_repeat_hint).replace("{frequencies}", &repeat_on.get())} }.into_any()
                >
                    <div class="pref-control switch-control">
                        <Switch
                            on=Signal::derive(move || repeat.get().unwrap_or(false))
                            label=Signal::derive(move || t_string!(i18n, mc_repeat).to_owned())
                            flip=move |_| set_repeat(repeat.get_untracked().map(|on| !on))
                        />
                    </div>
                </Row>
            </div>
        </div>
    }
}

#[component]
fn AdvancedTab(draft: Draft) -> impl IntoView {
    let i18n = use_i18n();
    let (manual, set_manual) = field(
        draft,
        |d| d.manual_add_contacts,
        |d, v| d.manual_add_contacts = v,
    );
    let (share, set_share) = field(draft, |d| d.share_position, |d, v| d.share_position = v);
    let (acks, set_acks) = field(draft, |d| d.multi_acks, |d, v| d.multi_acks = v);
    let (hash, set_hash) = field(draft, |d| d.path_hash_bytes, |d, v| d.path_hash_bytes = v);
    let (add, set_add) = field(draft, |d| d.auto_add, |d, v| d.auto_add = v);
    let (tuning, set_tuning) = field(draft, |d| d.tuning, |d, v| d.tuning = v);
    // A change to auto-add, on the one it has.
    let change_add = move |edit: fn(&mut MeshcoreAutoAdd)| {
        if let Some(mut now) = add.get_untracked() {
            edit(&mut now);
            set_add(Some(now));
        }
    };
    let kind = move |label: fn(leptos_i18n::I18nContext<crate::i18n::Locale>) -> &'static str,
                     on: fn(&MeshcoreAutoAdd) -> bool,
                     flip: fn(&mut MeshcoreAutoAdd)| {
        view! {
            <button
                type="button"
                class="choice"
                class:on=move || add.get().as_ref().is_some_and(on)
                on:click=move |_| change_add(flip)
            >
                {move || label(i18n)}
            </button>
        }
    };
    // The hop limits offered, and the one it has if it is another.
    let hops = move || {
        let mut all = HOPS.to_vec();
        if let Some(now) = add.get_untracked().and_then(|a| a.max_hops)
            && now > 0
            && !all.contains(&now)
        {
            all.push(now);
            all.sort_unstable();
        }
        all
    };
    let hops_label = move |n: u8| {
        if n == 1 {
            t_string!(i18n, mc_path_hop).to_owned()
        } else {
            t_string!(i18n, chat_hops).replace("{n}", &n.to_string())
        }
    };
    // Each tuning figure as written, set once it reads.
    let rx_delay = RwSignal::new(
        tuning
            .get_untracked()
            .map_or_else(String::new, |t| t.rx_delay.to_string()),
    );
    let airtime = RwSignal::new(
        tuning
            .get_untracked()
            .map_or_else(String::new, |t| t.airtime_factor.to_string()),
    );
    let put_tuning = move || {
        if let (Some(rx_delay), Some(airtime_factor)) = (
            decimal(&rx_delay.get_untracked()),
            decimal(&airtime.get_untracked()),
        ) {
            set_tuning(Some(MeshcoreTuning {
                rx_delay,
                airtime_factor,
            }));
        }
    };
    // Who it answers each kind of telemetry to.
    let telemetry = move |get: fn(&MeshcoreSettings) -> TelemetryShare,
                          set: fn(&mut MeshcoreSettings, TelemetryShare)| {
        let (value, put) = field(
            draft,
            move |d| Some(get(d)),
            move |d, v: Option<TelemetryShare>| {
                if let Some(v) = v {
                    set(d, v);
                }
            },
        );
        let code = |share: TelemetryShare| match share {
            TelemetryShare::Nobody => "nobody",
            TelemetryShare::Allowed => "allowed",
            TelemetryShare::Anyone => "anyone",
        };
        view! {
            <select
                class="pref-control"
                on:change=move |ev| {
                    let share = match event_target_value(&ev).as_str() {
                        "nobody" => TelemetryShare::Nobody,
                        "allowed" => TelemetryShare::Allowed,
                        _ => TelemetryShare::Anyone,
                    };
                    put(Some(share));
                }
            >
                {[TelemetryShare::Nobody, TelemetryShare::Allowed, TelemetryShare::Anyone]
                    .into_iter()
                    .map(|share| {
                        let label = move || match share {
                            TelemetryShare::Nobody => t_string!(i18n, mc_share_nobody),
                            TelemetryShare::Allowed => t_string!(i18n, mc_share_allowed),
                            TelemetryShare::Anyone => t_string!(i18n, mc_share_anyone),
                        };
                        view! {
                            <option value=code(share) selected=move || value.get() == Some(share)>
                                {label}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>
        }
    };
    view! {
        <div class="preferences">
            <Row title=t!(i18n, mc_manual_add).into_any() hint=t!(i18n, mc_manual_add_hint).into_any()>
                <div class="pref-control switch-control">
                    <Switch
                        on=manual
                        label=Signal::derive(move || t_string!(i18n, mc_manual_add).to_owned())
                        flip=move |_| set_manual(!manual.get_untracked())
                    />
                </div>
            </Row>
            // Which kinds it adds even so, with contacts added by hand.
            <div class:gone=move || !manual.get() || add.get().is_none()>
                <Row title=t!(i18n, mc_autoadd_kinds).into_any() hint=t!(i18n, mc_autoadd_kinds_hint).into_any()>
                    <div class="pref-control kinds-box">
                    <div class="flag-chips kinds">
                        {kind(|i18n| t_string!(i18n, mc_kind_companions), |a| a.companions, |a| a.companions = !a.companions)}
                        {kind(|i18n| t_string!(i18n, mc_kind_repeaters), |a| a.repeaters, |a| a.repeaters = !a.repeaters)}
                        {kind(|i18n| t_string!(i18n, mc_kind_rooms), |a| a.rooms, |a| a.rooms = !a.rooms)}
                        {kind(|i18n| t_string!(i18n, mc_kind_sensors), |a| a.sensors, |a| a.sensors = !a.sensors)}
                    </div>
                    </div>
                </Row>
            </div>
            <div class:gone=move || add.get().is_none()>
                <Row title=t!(i18n, mc_autoadd_hops).into_any() hint=t!(i18n, mc_autoadd_hops_hint).into_any()>
                    <select
                        class="pref-control"
                        on:change=move |ev| {
                            let value = event_target_value(&ev);
                            if let Some(mut now) = add.get_untracked() {
                                now.max_hops = value.parse().ok();
                                set_add(Some(now));
                            }
                        }
                    >
                        <option value="any" selected=move || add.get().is_some_and(|a| a.max_hops.is_none())>
                            {move || t_string!(i18n, mc_hops_any)}
                        </option>
                        <option value="0" selected=move || add.get().is_some_and(|a| a.max_hops == Some(0))>
                            {move || t_string!(i18n, mc_hops_direct)}
                        </option>
                        {hops()
                            .into_iter()
                            .map(|n| {
                                view! {
                                    <option value=n.to_string() selected=move || add.get().is_some_and(|a| a.max_hops == Some(n))>
                                        {move || hops_label(n)}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </Row>
                <Row title=t!(i18n, mc_overwrite).into_any() hint=t!(i18n, mc_overwrite_hint).into_any()>
                    <div class="pref-control switch-control">
                        <Switch
                            on=Signal::derive(move || add.get().is_some_and(|a| a.overwrite_oldest))
                            label=Signal::derive(move || t_string!(i18n, mc_overwrite).to_owned())
                            flip=move |_| change_add(|a| a.overwrite_oldest = !a.overwrite_oldest)
                        />
                    </div>
                </Row>
            </div>
            <Row title=t!(i18n, mc_share_position).into_any() hint=t!(i18n, mc_share_position_hint).into_any()>
                <div class="pref-control switch-control">
                    <Switch
                        on=share
                        label=Signal::derive(move || t_string!(i18n, mc_share_position).to_owned())
                        flip=move |_| set_share(!share.get_untracked())
                    />
                </div>
            </Row>
            <Row title=t!(i18n, mc_telemetry_base).into_any() hint=t!(i18n, mc_telemetry_base_hint).into_any()>
                {telemetry(|d| d.telemetry.base, |d, v| d.telemetry.base = v)}
            </Row>
            <Row title=t!(i18n, mc_telemetry_location).into_any() hint=t!(i18n, mc_telemetry_location_hint).into_any()>
                {telemetry(|d| d.telemetry.location, |d, v| d.telemetry.location = v)}
            </Row>
            <Row title=t!(i18n, mc_telemetry_environment).into_any() hint=t!(i18n, mc_telemetry_environment_hint).into_any()>
                {telemetry(|d| d.telemetry.environment, |d, v| d.telemetry.environment = v)}
            </Row>
            <Row title=t!(i18n, mc_multi_acks).into_any() hint=t!(i18n, mc_multi_acks_hint).into_any()>
                <div class="pref-control switch-control">
                    <Switch
                        on=acks
                        label=Signal::derive(move || t_string!(i18n, mc_multi_acks).to_owned())
                        flip=move |_| set_acks(!acks.get_untracked())
                    />
                </div>
            </Row>
            <div class:gone=move || hash.get().is_none()>
                <Row title=t!(i18n, mc_hash).into_any() hint=t!(i18n, mc_hash_hint).into_any()>
                    <select
                        class="pref-control"
                        on:change=move |ev| set_hash(event_target_value(&ev).parse().ok())
                    >
                        {(1..=3u8)
                            .map(|n| {
                                view! {
                                    <option value=n.to_string() selected=move || hash.get() == Some(n)>
                                        {move || {
                                            if n == 1 {
                                                t_string!(i18n, mc_hash_byte).to_owned()
                                            } else {
                                                t_string!(i18n, mc_hash_bytes).replace("{n}", &n.to_string())
                                            }
                                        }}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </Row>
            </div>
            <div class:gone=move || tuning.get().is_none()>
                <Row title=t!(i18n, mc_rx_delay).into_any() hint=t!(i18n, mc_rx_delay_hint).into_any()>
                    <input
                        class="pref-control"
                        inputmode="decimal"
                        prop:value=move || rx_delay.get()
                        on:input=move |ev| {
                            rx_delay.set(event_target_value(&ev));
                            put_tuning();
                        }
                    />
                </Row>
                <Row title=t!(i18n, mc_airtime).into_any() hint=t!(i18n, mc_airtime_hint).into_any()>
                    <input
                        class="pref-control"
                        inputmode="decimal"
                        prop:value=move || airtime.get()
                        on:input=move |ev| {
                            airtime.set(event_target_value(&ev));
                            put_tuning();
                        }
                    />
                </Row>
            </div>
        </div>
    }
}
