// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The rest of the radio's configuration and its modules, drawn from what
//! the daemon says of each field: one form for all of them.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::names;
use crate::radios::Switch;

/// Intervals common enough to offer, in seconds.
pub const INTERVALS: &[i64] = &[
    30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 14400, 21600, 43200, 86400, 129600,
    172800, 259200,
];

/// An interval in the largest unit that fits it whole.
fn interval_label(secs: i64) -> String {
    match secs {
        0 => "0".to_owned(),
        s if s % 3600 == 0 => format!("{} h", s / 3600),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

const EYE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye.svg");
const EYE_OFF: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye-off.svg");
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{ErrorCode, FieldKind, FixedPosition, SettingsField, SettingsSection};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// What is being changed of each section, by its id and then its fields'
/// names: kept by the page, so that going to another section or tab and
/// back loses nothing.
pub type SectionEdits = RwSignal<BTreeMap<String, BTreeMap<String, Json>>>;

/// The radio's admin keys, as a section among the configuration's.
const SECURITY: &str = "security";
/// What the radio can be told to do, as a section among the
/// configuration's.
const ACTIONS: &str = "actions";
/// A node's own list of nodes, administered over the mesh: not the radio's,
/// which its nodes' pages have.
const NODES: &str = "nodes";

/// How saving each section went, by its id, kept by the tab.
#[derive(Debug, Clone, Copy)]
struct SectionNotices(RwSignal<BTreeMap<String, Result<bool, Failure>>>);

/// The section where the radio's position can be fixed.
const POSITION: &str = "config.position";
/// Where the radio is, fixed, among a section's edits: by a name no field
/// of the protobufs has, its coordinates as written.
const FIXED: &str = "@fixed_position";

/// Where the radio is, as written in the edits of its position, or why
/// not: latitude and longitude, on the globe, and a whole altitude if any.
fn fixed_position(written: &Json) -> Option<FixedPosition> {
    let text = |name: &str| written.get(name).and_then(Json::as_str).unwrap_or_default();
    let latitude = crate::position::coordinate(text("latitude")).filter(|d| d.abs() <= 90.0)?;
    let longitude = crate::position::coordinate(text("longitude")).filter(|d| d.abs() <= 180.0)?;
    let altitude = match text("altitude").trim() {
        "" => None,
        metres => Some(metres.parse::<i32>().ok()?),
    };
    Some(FixedPosition {
        latitude,
        longitude,
        altitude,
    })
}

/// A name from the protobufs as words: `external_notification` reads as
/// "External notification".
pub fn words(name: &str) -> String {
    let spaced = name.replace(['_', '.'], " ");
    let mut chars = spaced.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Whether a section is on, as the radio has it, for those that say: by
/// their `enabled`, or, for the network, by Wi-Fi or Ethernet being so.
pub(crate) fn enabled(section: &SettingsSection) -> Option<bool> {
    let switches: &[&str] = match section.id.as_str() {
        "config.network" => &["wifi_enabled", "eth_enabled"],
        _ => &["enabled"],
    };
    let states: Vec<bool> = section
        .fields
        .iter()
        .filter(|f| switches.contains(&f.name.as_str()))
        .filter_map(|f| f.value.as_ref().and_then(Json::as_bool))
        .collect();
    (!states.is_empty()).then(|| states.contains(&true))
}

/// Lets a notice of something sent to the radio go once the radio has been
/// read back with it, as the notice says it will be; or, if it rebooted to
/// apply it, once it is back. One of something gone wrong stays, to be read,
/// until the next try.
pub fn clear_when_synced<T: Clone + Send + Sync + 'static>(
    live: Live,
    notice: RwSignal<Option<Result<T, Failure>>>,
) {
    // How many syncs there had been when it was given.
    let since = StoredValue::new(None::<u32>);
    Effect::new(move |_| {
        let synced = live.synced.get();
        if !notice.with(|n| matches!(n, Some(Ok(_)))) {
            since.set_value(None);
            return;
        }
        match since.get_value() {
            None => since.set_value(Some(synced)),
            Some(at) if synced != at => {
                since.set_value(None);
                notice.set(None);
            }
            Some(_) => {}
        }
    });
}

/// Undo and save, at the foot of what they act on.
#[component]
pub fn SaveBar(
    dirty: Signal<bool>,
    busy: Signal<bool>,
    save: impl Fn() + Send + Sync + 'static,
    undo: impl Fn() + Send + Sync + 'static,
    /// What goes at its start, away from saving: another way to fill it.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let i18n = use_i18n();
    // To the radio, or to the node administered.
    let to_node = crate::settings::node_of().is_some();
    view! {
        <div class="save-bar">
            {children.map(|start| view! { <div class="start">{start()}</div> })}
            <button type="button" disabled=move || !dirty.get() on:click=move |_| undo()>
                {t!(i18n, settings_undo)}
            </button>
            <button
                type="button"
                class="primary"
                disabled=move || busy.get() || !dirty.get()
                on:click=move |_| save()
            >
                {if to_node {
                    t!(i18n, admin_save).into_any()
                } else {
                    t!(i18n, settings_save).into_any()
                }}
            </button>
        </div>
    }
}

/// The configuration's sections, or the modules: a list of them aside, and
/// the one picked.
#[component]
pub fn SectionsTab(
    sections: RwSignal<Vec<SettingsSection>>,
    module: bool,
    edits: SectionEdits,
) -> impl IntoView {
    let i18n = use_i18n();
    let list = Memo::new(move |_| {
        sections.with(|all| {
            all.iter()
                .filter(|s| s.module == module)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let picked = RwSignal::new(None::<String>);
    // The radio's admin keys, among the configuration's, with a page of
    // their own: the rest of its security is left alone.
    let security = move || !module && picked.get().as_deref() == Some(SECURITY);
    // And what it can be told to do.
    let actions = move || !module && picked.get().as_deref() == Some(ACTIONS);
    // And, of a node administered over the mesh, its own list of nodes.
    let nodes = move || {
        !module && crate::settings::node_of().is_some() && picked.get().as_deref() == Some(NODES)
    };
    // The one picked, or else the first.
    let current = Memo::new(move |_| {
        let picked = picked.get();
        if matches!(picked.as_deref(), Some(SECURITY | ACTIONS | NODES)) {
            return None;
        }
        list.with(|l| {
            picked
                .and_then(|id| l.iter().find(|s| s.id == id))
                .or(l.first())
                .cloned()
        })
    });
    let current_id = move || current.with(|c| c.as_ref().map(|c| c.id.clone()));

    // A node's, each read of it as it is picked, once: until then it has
    // no fields. Why not, if it could not be.
    let live = expect_context::<Live>();
    let node = crate::settings::node_of();
    let asked = RwSignal::new(std::collections::BTreeSet::<String>::new());
    // How saving each went, kept here: a section is drawn anew as it is
    // read again, and says it still.
    provide_context(SectionNotices(RwSignal::new(BTreeMap::new())));
    provide_context(sections);
    let failed = RwSignal::new(None::<(String, Failure)>);
    let unread =
        move || node.is_some() && current.with(|c| c.as_ref().is_some_and(|c| c.fields.is_empty()));
    Effect::new(move |_| {
        let (Some(node), Some(radio)) = (node, live.selected.get_untracked()) else {
            return;
        };
        let Some(id) = current_id() else { return };
        failed.track();
        if !unread() || asked.with_untracked(|a| a.contains(&id)) {
            return;
        }
        asked.update(|a| {
            a.insert(id.clone());
        });
        // Over the mesh, a while: the page may be gone by the time it comes.
        spawn_local(async move {
            match daemon::node_section(radio, node, &id).await {
                Ok(read) => {
                    let _ = asked.try_update(|a| {
                        a.remove(&read.id);
                    });
                    let _ = sections.try_update(|all| {
                        if let Some(section) = all.iter_mut().find(|s| s.id == read.id) {
                            *section = read;
                        }
                    });
                }
                Err(failure) => {
                    let _ = asked.try_update(|a| {
                        a.remove(&id);
                    });
                    let _ = failed.try_set(Some((id, failure)));
                }
            }
        });
    });
    let waiting = move || {
        unread().then(|| {
            let id = current_id().unwrap_or_default();
            match failed.get().filter(|(at, _)| *at == id) {
                Some((_, failure)) => view! {
                    <crate::node_admin::Failed failure retry=Callback::new(move |()| failed.set(None)) />
                }
                .into_any(),
                None => view! {
                    <p class="dim admin-waiting">
                        <span class="spinner" aria-hidden="true"></span>
                        {t!(i18n, admin_section_asking)}
                    </p>
                }
                .into_any(),
            }
        })
    };

    view! {
        <div class="sections-tab">
            <nav class="section-list">
                <For
                    each=move || list.get()
                    key=|s| (s.id.clone(), enabled(s))
                    let:section
                >
                    {
                        let id = section.id.clone();
                        let on = {
                            let id = id.clone();
                            move || current_id().as_deref() == Some(id.as_str())
                        };
                        let touched = {
                            let id = id.clone();
                            move || edits.with(|e| e.contains_key(&id))
                        };
                        // Room for it kept by those without one, so that the
                        // names line up.
                        let state = enabled(&section);
                        let state = view! {
                            <span
                                class="state"
                                class:on=state == Some(true)
                                class:none=state.is_none()
                            ></span>
                        };
                        view! {
                            <button
                                type="button"
                                class:on=on
                                on:click=move |_| picked.set(Some(id.clone()))
                            >
                                {state}
                                <span class="label">
                                    {move || names::section(i18n.get_locale(), &section.name)}
                                </span>
                                <span class="edited" class:gone=move || !touched()>
                                    "✎"
                                </span>
                            </button>
                        }
                    }
                </For>
                {(!module).then(|| view! {
                    <button
                        type="button"
                        class:on=security
                        on:click=move |_| picked.set(Some(SECURITY.to_owned()))
                    >
                        <span class="state none"></span>
                        <span class="label">
                            {move || names::section(i18n.get_locale(), "security")}
                        </span>
                        <span class="edited gone">"✎"</span>
                    </button>
                    {crate::settings::node_of().is_some().then(|| view! {
                        <button
                            type="button"
                            class:on=nodes
                            on:click=move |_| picked.set(Some(NODES.to_owned()))
                        >
                            <span class="state none"></span>
                            <span class="label">
                                {move || names::section(i18n.get_locale(), "nodes")}
                            </span>
                            <span class="edited gone">"✎"</span>
                        </button>
                    })}
                    <button
                        type="button"
                        class:on=actions
                        on:click=move |_| picked.set(Some(ACTIONS.to_owned()))
                    >
                        <span class="state none"></span>
                        <span class="label">
                            {move || names::section(i18n.get_locale(), "actions")}
                        </span>
                        <span class="edited gone">"✎"</span>
                    </button>
                })}
            </nav>
            // Drawn anew for each section, and as the radio tells it anew,
            // rather than one's elements taken over by the next: a switch on
            // in one and off in the other would be seen to flip.
            <div class="section-panel">
                {waiting}
                <For
                    each=move || current.get().into_iter().filter(|s| !s.fields.is_empty())
                    key=|section| (section.id.clone(), serde_json::to_string(section).unwrap_or_default())
                    let:section
                >
                    <SectionForm section edits />
                </For>
                {move || security().then(|| view! { <crate::admin_keys::AdminKeysPanel /> })}
                {move || nodes().then(|| view! { <crate::remote_nodes::RemoteNodesPanel /> })}
                {move || actions().then(|| view! { <crate::node_actions::ActionsPanel /> })}
            </div>
        </div>
    }
}

/// A section's form; only shown, with nothing to change, if `read_only`,
/// as another node's are.
#[component]
pub(crate) fn SectionForm(
    section: SettingsSection,
    edits: SectionEdits,
    #[prop(optional)] read_only: bool,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let node = crate::settings::node_of();
    // How it went: whether the radio reboots for it, or why not; kept by
    // the tab too, as a node's section is drawn anew once read again.
    let notices = use_context::<SectionNotices>();
    let notice = RwSignal::new(
        notices.and_then(|n| n.0.with_untracked(|all| all.get(&section.id).cloned())),
    );
    if let Some(SectionNotices(all)) = notices {
        let id = section.id.clone();
        Effect::new(move |_| {
            let now = notice.get();
            all.update_untracked(|all| match now {
                Some(now) => {
                    all.insert(id.clone(), now);
                }
                None => {
                    all.remove(&id);
                }
            });
        });
    }
    clear_when_synced(live, notice);
    // A node's sections, to read this one again once changed.
    let sections = use_context::<RwSignal<Vec<SettingsSection>>>();
    let busy = RwSignal::new(false);
    let id = section.id.clone();
    let on = enabled(&section);
    let dirty = {
        let id = id.clone();
        Signal::derive(move || edits.with(|e| e.contains_key(&id)))
    };

    // What each field is now, edited or as the radio has it: some are only
    // shown as another is so.
    let originals: std::collections::HashMap<String, Json> = section
        .fields
        .iter()
        .filter_map(|f| Some((f.name.clone(), f.value.clone()?)))
        .collect();
    let value_of = {
        let id = id.clone();
        move |name: &str| {
            edits
                .with(|e| e.get(&id).and_then(|fields| fields.get(name).cloned()))
                .or_else(|| originals.get(name).cloned())
        }
    };

    let save = {
        let id = id.clone();
        let value_of = value_of.clone();
        move || {
            let Some(radio) = live.selected.get_untracked() else {
                return;
            };
            let Some(mut values) = edits.with_untracked(|e| e.get(&id).cloned()) else {
                return;
            };
            // Where the radio is goes apart, and only with its position
            // fixed: otherwise it is not even shown.
            let fixed = values.remove(FIXED);
            let fixed_on = untrack(|| value_of("fixed_position")) == Some(Json::Bool(true));
            let fixed = match fixed {
                Some(written) if fixed_on => match fixed_position(&written) {
                    Some(at) => Some(at),
                    None => {
                        notice.set(Some(Err(Failure::Rejected(ErrorCode::InvalidSettings))));
                        return;
                    }
                },
                _ => None,
            };
            if values.is_empty() && fixed.is_none() {
                return;
            }
            // Moving where it is alone reboots nothing; anything goes to
            // another node over the mesh, asked first.
            if (!values.is_empty() || node.is_some()) && !crate::settings::sure(i18n, node, false) {
                return;
            }
            let id = id.clone();
            busy.set(true);
            spawn_local(async move {
                let outcome = daemon::change_section(radio, node, &id, values, fixed).await;
                // As said by then, the page perhaps gone.
                let done = outcome.is_ok();
                if done {
                    let _ = edits.try_update(|e| {
                        e.remove(&id);
                    });
                }
                let _ = notice.try_set(Some(outcome.map(|applied| applied.reboots)));
                let _ = busy.try_set(false);
                // A node's, read from it again, as it has it now.
                if done
                    && node.is_some()
                    && let Some(sections) = sections
                {
                    let _ = sections.try_update(|all| {
                        if let Some(section) = all.iter_mut().find(|s| s.id == id) {
                            section.fields.clear();
                        }
                    });
                }
            });
        }
    };
    let undo = {
        let id = id.clone();
        move || {
            edits.update(|e| {
                e.remove(&id);
            });
        }
    };

    let notice_view = move || {
        notice.get().map(|outcome| match outcome {
            Ok(true) if node.is_some() => {
                view! { <p class="note">{t!(i18n, admin_applied_reboot)}</p> }.into_any()
            }
            Ok(false) if node.is_some() => {
                view! { <p class="note">{t!(i18n, admin_applied)}</p> }.into_any()
            }
            Ok(true) => {
                view! { <p class="note">{t!(i18n, settings_applied_reboot)}</p> }.into_any()
            }
            Ok(false) => view! { <p class="note">{t!(i18n, position_applied)}</p> }.into_any(),
            Err(Failure::Rejected(ErrorCode::InvalidSettings)) => {
                view! { <p class="note bad">{t!(i18n, section_invalid)}</p> }.into_any()
            }
            Err(Failure::Rejected(ErrorCode::RadioDidNotAnswer)) => {
                view! { <p class="note bad">{t!(i18n, settings_no_answer)}</p> }.into_any()
            }
            Err(_) => view! { <p class="note bad">{t!(i18n, error_failed)}</p> }.into_any(),
        })
    };

    let fields = section
        .fields
        .into_iter()
        .map(|field| {
            let visible = {
                let (id, name, value_of) = (id.clone(), field.name.clone(), value_of.clone());
                Signal::derive(move || crate::fields::shown(&id, &name, &value_of))
            };
            // Where the radio is, under the switch that fixes it.
            let fixed =
                (!read_only && id == POSITION && field.name == "fixed_position").then(|| {
                    let value_of = value_of.clone();
                    let visible = Signal::derive(move || {
                        visible.get() && value_of("fixed_position") == Some(Json::Bool(true))
                    });
                    view! { <FixedRow edits visible /> }
                });
            view! {
                <FieldInput section=id.clone() field edits visible />
                {fixed}
            }
        })
        .collect_view();

    if read_only {
        return view! {
            <fieldset class="read-only" disabled>
                <div class="preferences">{fields}</div>
            </fieldset>
        }
        .into_any();
    }
    view! {
        <div class="section-head">
            <h2>{move || names::section(i18n.get_locale(), &section.name)}</h2>
            {on.map(|on| {
                view! {
                    <span class=if on { "badge ready" } else { "badge disabled" }>
                        {if on { t!(i18n, section_on).into_any() } else { t!(i18n, section_off).into_any() }}
                    </span>
                }
            })}
        </div>
        // Checked by the daemon, which says what is wrong: a browser left to
        // it refuses silently, as it does a setting at zero below a floor the
        // firmware reads as "the default".
        <form novalidate on:submit=|ev| ev.prevent_default()>
            <div class="preferences">{fields}</div>
            {notice_view}
            <SaveBar dirty busy=Signal::derive(move || busy.get()) save undo />
        </form>
    }
    .into_any()
}

/// Where the radio is, with its position fixed: as it says it is now, or
/// as written, and picked on a map.
#[component]
fn FixedRow(edits: SectionEdits, visible: Signal<bool>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // The radio's, or the node administered.
    let node = crate::settings::node_of();
    // Where it says it is, as written: by its GPS until fixed, and then
    // where it was fixed; again as it says anew. A node's, as last heard.
    let said = Memo::new(move |_| {
        let num = node.or_else(|| live.status.with(|s| Some(s.as_ref()?.local.as_ref()?.num)));
        let now = num.and_then(|num| live.nodes.with(|nodes| nodes.get(&num)?.position));
        (
            now.map(|at| crate::position::written(at.latitude))
                .unwrap_or_default(),
            now.map(|at| crate::position::written(at.longitude))
                .unwrap_or_default(),
            now.and_then(|at| at.altitude)
                .map(|metres| metres.to_string())
                .unwrap_or_default(),
        )
    });
    let (latitude, longitude, altitude) = said.with_untracked(|(lat, lon, alt)| {
        (
            RwSignal::new(lat.clone()),
            RwSignal::new(lon.clone()),
            RwSignal::new(alt.clone()),
        )
    });
    // As written, among the edits while it differs from what the radio
    // says, even half written: saving says if it does not read.
    let changed = move |()| {
        let written = (
            latitude.get_untracked(),
            longitude.get_untracked(),
            altitude.get_untracked(),
        );
        let same = said.with_untracked(|said| *said == written);
        edits.update(|e| {
            let fields = e.entry(POSITION.to_owned()).or_default();
            if same {
                fields.remove(FIXED);
            } else {
                fields.insert(
                    FIXED.to_owned(),
                    serde_json::json!({
                        "latitude": written.0,
                        "longitude": written.1,
                        "altitude": written.2,
                    }),
                );
            }
            if fields.is_empty() {
                e.remove(POSITION);
            }
        });
    };
    // Not edited, undone or saved: as the radio says, now and as it says
    // anew.
    Effect::new(move |_| {
        let edited = edits.with(|e| e.get(POSITION).is_some_and(|f| f.contains_key(FIXED)));
        let (lat, lon, alt) = said.get();
        if !edited {
            latitude.set(lat);
            longitude.set(lon);
            altitude.set(alt);
        }
    });
    view! {
        <div class="pref-row" class:gone=move || !visible.get()>
            <div class="pref-what">
                <span class="pref-title">{t!(i18n, position_fixed)}</span>
                <span class="dim">{t!(i18n, position_fixed_hint)}</span>
            </div>
            <crate::position::PositionInputs latitude longitude altitude changed />
        </div>
    }
}

/// One field, with the input its kind takes.
#[component]
fn FieldInput(
    section: String,
    field: SettingsField,
    edits: SectionEdits,
    visible: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let name = field.name.clone();
    // In the interface's language: Spanish from our own words for it, any
    // other the protobufs'.
    let english = (
        field.label.clone().unwrap_or_else(|| words(&field.name)),
        field.description.clone().unwrap_or_default(),
    );
    let id = format!("{section}.{name}");
    let field_id = id.clone();
    let said = Signal::derive(
        move || match (i18n.get_locale(), crate::fields::spanish(&id)) {
            (Locale::es, Some((label, description))) => (label.to_owned(), description.to_owned()),
            _ => english.clone(),
        },
    );
    let label = Signal::derive(move || said.with(|s| s.0.clone()));
    let title = Signal::derive(move || said.with(|s| s.1.clone()));
    // An interval's options say their own unit.
    let interval = field.kind == FieldKind::Integer
        && (field.unit.as_deref() == Some("s") || crate::fields::intervals(&field_id).is_some());
    let unit = field.unit.clone().filter(|_| !interval);
    let precision = (field.kind == FieldKind::Integer)
        .then(|| crate::fields::precision(&field_id))
        .flatten();
    let original = field.value.clone().unwrap_or(Json::Null);

    // The edit of it, if any.
    let edit = {
        let section = section.clone();
        let name = name.clone();
        move || edits.with(|e| e.get(&section)?.get(&name).cloned())
    };
    // Sets or drops the edit; a section left without any has none.
    let store = {
        let name = name.clone();
        let section = section.clone();
        move |value: Option<Json>| {
            edits.update(|e| {
                let fields = e.entry(section.clone()).or_default();
                match value {
                    Some(value) => {
                        fields.insert(name.clone(), value);
                    }
                    None => {
                        fields.remove(&name);
                    }
                }
                if fields.is_empty() {
                    e.remove(&section);
                }
            });
        }
    };
    // What it shows: the edit if there is one, what the radio has otherwise.
    let current = {
        let edit = edit.clone();
        let original = original.clone();
        move || edit().unwrap_or_else(|| original.clone())
    };
    // An edit back to what the radio has is no edit.
    let put = {
        let store = store.clone();
        move |value: Json| store((value != original).then_some(value))
    };

    let input = match field.kind {
        FieldKind::Bool => {
            let on = {
                let current = current.clone();
                Signal::derive(move || current().as_bool().unwrap_or(false))
            };
            let put = put.clone();
            view! {
                <div class="pref-control switch-control">
                    <Switch
                        on
                        label
                        flip=move |_| put(Json::Bool(!on.get_untracked()))
                    />
                </div>
            }
            .into_any()
        }
        FieldKind::Flags => {
            let codes = field.choices.clone();
            let set_of = {
                let current = current.clone();
                move || -> Vec<String> {
                    current()
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default()
                }
            };
            // Each flag on or off, the rest as they are.
            let flip = {
                let set_of = set_of.clone();
                let put = put.clone();
                std::sync::Arc::new(move |code: String| {
                    let mut set = set_of();
                    if let Some(at) = set.iter().position(|c| *c == code) {
                        set.remove(at);
                    } else {
                        set.push(code);
                    }
                    put(Json::from(set));
                })
            };
            if let [only] = codes.as_slice() {
                // One flag: a switch.
                let only = only.clone();
                let on = {
                    let only = only.clone();
                    Signal::derive(move || set_of().contains(&only))
                };
                view! {
                    <div class="pref-control switch-control">
                        <Switch on label flip=move |_| flip(only.clone()) />
                    </div>
                }
                .into_any()
            } else {
                // Several: chips, each on or off.
                let field_id = field_id.clone();
                view! {
                    <div class="pref-control flag-chips">
                        {codes
                            .into_iter()
                            .map(|code| {
                                let on = {
                                    let (code, set_of) = (code.clone(), set_of.clone());
                                    move || set_of().contains(&code)
                                };
                                let flip = flip.clone();
                                let (shown, field_id) = (code.clone(), field_id.clone());
                                view! {
                                    <button
                                        type="button"
                                        class="choice"
                                        class:on=on
                                        on:click=move |_| flip(code.clone())
                                    >
                                        {move || crate::fields::choice(i18n.get_locale(), &field_id, &shown)}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                }
                .into_any()
            }
        }
        // Two or three: a pill that slides to the one chosen.
        FieldKind::Choice if field.choices.len() <= 3 => {
            let options = field
                .choices
                .iter()
                .map(|code| {
                    let on = {
                        let (code, current) = (code.clone(), current.clone());
                        Signal::derive(move || current().as_str() == Some(code.as_str()))
                    };
                    let pick = {
                        let (code, put) = (code.clone(), put.clone());
                        Box::new(move || put(Json::String(code.clone())))
                            as Box<dyn Fn() + Send + Sync>
                    };
                    let (code, field_id) = (code.clone(), field_id.clone());
                    let label = Signal::derive(move || {
                        crate::fields::choice(i18n.get_locale(), &field_id, &code).to_string()
                    });
                    (on, pick, label)
                })
                .collect();
            crate::preferences::pills(options).into_any()
        }
        FieldKind::Choice => {
            let current = current.clone();
            let choices = field.choices.clone();
            view! {
                <select
                    class="pref-control"
                    prop:value=move || current().as_str().unwrap_or_default().to_owned()
                    on:change=move |ev| put(Json::String(event_target_value(&ev)))
                >
                    {
                        let current = current.clone();
                        move || {
                            // Built again in another language: the one the
                            // radio has stays chosen.
                            let locale = i18n.get_locale();
                            let chosen = current().as_str().unwrap_or_default().to_owned();
                            choices
                                .clone()
                                .into_iter()
                                .map(|code| {
                                    let shown = crate::fields::choice(locale, &field_id, &code);
                                    let selected = code == chosen;
                                    view! { <option value=code selected=selected>{shown}</option> }
                                })
                                .collect_view()
                        }
                    }
                </select>
            }
            .into_any()
        }
        // A position's precision: along its range, as a channel's, saying
        // how far off it may be; what the firmware uses for anything else
        // where it has that.
        FieldKind::Integer if precision.is_some() => {
            let (least, most, otherwise) = precision.unwrap_or_default();
            let bits = {
                let current = current.clone();
                Signal::derive(move || {
                    current()
                        .as_u64()
                        .and_then(|b| u32::try_from(b).ok())
                        .filter(|b| (least..=most).contains(b))
                        .unwrap_or(otherwise)
                })
            };
            let far = |bits: u32| {
                crate::mesh::precision_radius(bits)
                    .map(crate::mesh::radius)
                    .unwrap_or_default()
            };
            let put = put.clone();
            view! {
                <label class="pref-control precision-field">
                    <b>{move || far(bits.get())}</b>
                    <input
                        type="range"
                        min=least.to_string()
                        max=most.to_string()
                        step="1"
                        aria-label=label
                        prop:value=move || bits.get().to_string()
                        on:input=move |ev| {
                            if let Ok(bits) = event_target_value(&ev).parse::<u32>() {
                                put(Json::from(bits));
                            }
                        }
                    />
                    <span class="ends dim">
                        <span>{far(least)}</span>
                        <span>{far(most)}</span>
                    </span>
                </label>
            }
            .into_any()
        }
        // An interval in seconds: common ones to pick, or one's own.
        FieldKind::Integer if interval => {
            let floor = field.min.unwrap_or(0.0);
            // Its own, for the few that are short waits or have a value that
            // means more; the common ones otherwise.
            let own_steps = crate::fields::intervals(&field_id);
            let steps: Vec<i64> = std::iter::once(0)
                .chain(
                    own_steps
                        .map_or(INTERVALS, |(steps, _)| steps)
                        .iter()
                        .copied()
                        .filter(|&s| s as f64 >= floor),
                )
                .chain(
                    own_steps
                        .and_then(|(_, special)| special)
                        .map(|(secs, _, _)| secs),
                )
                .collect();
            let special = own_steps.and_then(|(_, special)| special);
            let seconds = {
                let current = current.clone();
                Signal::derive(move || current().as_i64().unwrap_or(0))
            };
            // One's own while it is not one of those, or while one is being
            // written.
            let own = RwSignal::new(!steps.contains(&seconds.get_untracked()));
            let choice = {
                let steps = steps.clone();
                move || {
                    if own.get() || !steps.contains(&seconds.get()) {
                        "own".to_owned()
                    } else {
                        seconds.get().to_string()
                    }
                }
            };
            let pick = {
                let put = put.clone();
                move |ev: leptos::ev::Event| {
                    let value = event_target_value(&ev);
                    if value == "own" {
                        own.set(true);
                    } else if let Ok(secs) = value.parse::<i64>() {
                        own.set(false);
                        put(Json::from(secs));
                    }
                }
            };
            let options = steps
                .iter()
                .map(|&secs| {
                    let field_id = field_id.clone();
                    let label = move || match special {
                        Some((value, es, en)) if value == secs => match i18n.get_locale() {
                            Locale::es => es.to_owned(),
                            Locale::en => en.to_owned(),
                        },
                        _ if secs == 0 => crate::fields::zero(i18n.get_locale(), &field_id),
                        _ => interval_label(secs),
                    };
                    view! { <option value=secs.to_string()>{label}</option> }
                })
                .collect_view();
            view! {
                <div class="pref-control interval-field">
                    <select prop:value=choice on:change=pick>
                        {options}
                        <option value="own">{t!(i18n, interval_own)}</option>
                    </select>
                    <span class="number-field" class:gone=move || !(own.get() || !steps.contains(&seconds.get()))>
                        <input
                            type="number"
                            step="1"
                            min="0"
                            prop:value=move || seconds.get().to_string()
                            on:input=move |ev| {
                                if let Ok(secs) = event_target_value(&ev).parse::<i64>() {
                                    put(Json::from(secs));
                                }
                            }
                        />
                        <crate::stepper::Stepper />
                    </span>
                </div>
            }
            .into_any()
        }
        FieldKind::Integer | FieldKind::Number => {
            let current = current.clone();
            let whole = field.kind == FieldKind::Integer;
            view! {
                <span class="pref-control number-field">
                    <input
                        type="number"
                        step=if whole { "1" } else { "any" }
                        min=field.min.map(|m| m.to_string())
                        max=field.max.map(|m| m.to_string())
                        prop:value=move || current().to_string()
                        on:input=move |ev| {
                            let text = event_target_value(&ev);
                            let number = if whole {
                                text.parse::<i64>().ok().map(Json::from)
                            } else {
                                text.parse::<f64>().ok().map(Json::from)
                            };
                            if let Some(number) = number {
                                put(number);
                            }
                        }
                    />
                    <crate::stepper::Stepper />
                </span>
            }
            .into_any()
        }
        FieldKind::Text if field.secret => {
            let set = field.set;
            let live = expect_context::<Live>();
            let node = crate::settings::node_of();
            // The secret itself, while shown: asked for then, and nowhere
            // else; gone as it is hidden, or the page goes.
            let shown = RwSignal::new(None::<String>);
            let reveal = {
                let section = section.clone();
                let name = name.clone();
                move |_| {
                    if shown.with_untracked(Option::is_some) {
                        shown.set(None);
                        return;
                    }
                    let Some(radio) = live.selected.get_untracked() else {
                        return;
                    };
                    let (section, name) = (section.clone(), name.clone());
                    spawn_local(async move {
                        if let Ok(secret) =
                            daemon::section_secret(radio, node, &section, &name).await
                        {
                            let _ = shown.try_set(Some(secret.value));
                        }
                    });
                }
            };
            view! {
                <div class="pref-control secret-field">
                    <input
                        type=move || if shown.with(Option::is_some) { "text" } else { "password" }
                        autocomplete="new-password"
                        placeholder=move || {
                            if set { t_string!(i18n, secret_set) } else { t_string!(i18n, secret_unset) }
                        }
                        prop:value=move || {
                            edit()
                                .and_then(|v| v.as_str().map(str::to_owned))
                                .or_else(|| shown.get())
                                .unwrap_or_default()
                        }
                        on:input=move |ev| {
                            let text = event_target_value(&ev);
                            // Left empty, the secret stays as it is; as it
                            // was shown, too.
                            let same = shown.with_untracked(|s| s.as_deref() == Some(text.as_str()));
                            store((!text.is_empty() && !same).then_some(Json::String(text)));
                        }
                    />
                    {set.then(|| view! {
                        <button
                            type="button"
                            class="icon-button"
                            class:on=move || shown.with(Option::is_some)
                            title=move || t_string!(i18n, secret_show)
                            aria-label=move || t_string!(i18n, secret_show)
                            on:click=reveal.clone()
                            inner_html=move || if shown.with(Option::is_some) { EYE_OFF } else { EYE }
                        ></button>
                    })}
                </div>
            }
            .into_any()
        }
        FieldKind::Text => {
            let current = current.clone();
            view! {
                <input
                    class="pref-control"
                    prop:value=move || current().as_str().unwrap_or_default().to_owned()
                    on:input=move |ev| put(Json::String(event_target_value(&ev)))
                />
            }
            .into_any()
        }
    };

    // A row, as the other tabs' settings: what it is and a word on it, and
    // its control.
    view! {
        <div class="pref-row" class:gone=move || !visible.get()>
            <div class="pref-what">
                <span class="pref-title">
                    {label}
                    {unit.clone().map(|u| format!(" ({u})"))}
                    {field.diy_only.then(|| view! { <span class="tag">"DIY"</span> })}
                </span>
                <span class="dim">{title}</span>
            </div>
            {input}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::words;

    #[test]
    fn names_read_as_words() {
        assert_eq!(words("external_notification"), "External notification");
        assert_eq!(
            words("map_report_settings.publish_interval_secs"),
            "Map report settings publish interval secs"
        );
    }
}
