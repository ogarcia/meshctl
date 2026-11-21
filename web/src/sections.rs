// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The rest of the radio's configuration and its modules, drawn from what
//! the daemon says of each field: one form for all of them.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::names;
use crate::radios::Switch;

const EYE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye.svg");
const EYE_OFF: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye-off.svg");
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{ErrorCode, FieldKind, SettingsField, SettingsSection};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// What is being changed of each section, by its id and then its fields'
/// names: kept by the page, so that going to another section or tab and
/// back loses nothing.
pub type SectionEdits = RwSignal<BTreeMap<String, BTreeMap<String, Json>>>;

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
fn enabled(section: &SettingsSection) -> Option<bool> {
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
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="save-bar">
            <button type="button" disabled=move || !dirty.get() on:click=move |_| undo()>
                {t!(i18n, settings_undo)}
            </button>
            <button
                type="button"
                class="primary"
                disabled=move || busy.get() || !dirty.get()
                on:click=move |_| save()
            >
                {t!(i18n, settings_save)}
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
    // The one picked, or else the first.
    let current = Memo::new(move |_| {
        let picked = picked.get();
        list.with(|l| {
            picked
                .and_then(|id| l.iter().find(|s| s.id == id))
                .or(l.first())
                .cloned()
        })
    });
    let current_id = move || current.with(|c| c.as_ref().map(|c| c.id.clone()));

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
            </nav>
            <div class="section-panel">
                {move || current.get().map(|section| view! { <SectionForm section edits /> })}
            </div>
        </div>
    }
}

#[component]
fn SectionForm(section: SettingsSection, edits: SectionEdits) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let notice = RwSignal::new(None::<Result<(), Failure>>);
    clear_when_synced(live, notice);
    let busy = RwSignal::new(false);
    let id = section.id.clone();
    let on = enabled(&section);
    let dirty = {
        let id = id.clone();
        Signal::derive(move || edits.with(|e| e.contains_key(&id)))
    };

    let save = {
        let id = id.clone();
        move || {
            let Some(radio) = live.selected.get_untracked() else {
                return;
            };
            let Some(values) = edits.with_untracked(|e| e.get(&id).cloned()) else {
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
            let id = id.clone();
            busy.set(true);
            spawn_local(async move {
                let outcome = daemon::change_section(radio, &id, values).await;
                if outcome.is_ok() {
                    edits.update(|e| {
                        e.remove(&id);
                    });
                }
                notice.set(Some(outcome.map(|_| ())));
                busy.set(false);
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
            Ok(()) => view! { <p class="note">{t!(i18n, settings_applied_reboot)}</p> }.into_any(),
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
        .map(|field| view! { <FieldInput section=id.clone() field edits /> })
        .collect_view();

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
}

/// One field, with the input its kind takes.
#[component]
fn FieldInput(section: String, field: SettingsField, edits: SectionEdits) -> impl IntoView {
    let i18n = use_i18n();
    let name = field.name.clone();
    // In the interface's language: Spanish from our own words for it, any
    // other the protobufs'.
    let english = (
        field.label.clone().unwrap_or_else(|| words(&field.name)),
        field.description.clone().unwrap_or_default(),
    );
    let id = format!("{section}.{name}");
    let said = Signal::derive(
        move || match (i18n.get_locale(), crate::fields::spanish(&id)) {
            (Locale::es, Some((label, description))) => (label.to_owned(), description.to_owned()),
            _ => english.clone(),
        },
    );
    let label = Signal::derive(move || said.with(|s| s.0.clone()));
    let title = Signal::derive(move || said.with(|s| s.1.clone()));
    let unit = field.unit.clone();
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
        FieldKind::Choice => {
            let current = current.clone();
            let choices = field.choices.clone();
            view! {
                <select
                    class="pref-control"
                    prop:value=move || current().as_str().unwrap_or_default().to_owned()
                    on:change=move |ev| put(Json::String(event_target_value(&ev)))
                >
                    {choices
                        .into_iter()
                        .map(|code| {
                            let shown = words(&code.to_lowercase());
                            view! { <option value=code>{shown}</option> }
                        })
                        .collect_view()}
                </select>
            }
            .into_any()
        }
        FieldKind::Integer | FieldKind::Number => {
            let current = current.clone();
            let whole = field.kind == FieldKind::Integer;
            view! {
                <input
                    class="pref-control"
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
            }
            .into_any()
        }
        FieldKind::Text if field.secret => {
            let set = field.set;
            let live = expect_context::<Live>();
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
                        if let Ok(secret) = daemon::section_secret(radio, &section, &name).await {
                            shown.set(Some(secret.value));
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
        <div class="pref-row">
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
