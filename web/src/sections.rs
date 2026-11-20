// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The rest of the radio's configuration and its modules, drawn from what
//! the daemon says of each field: one form for all of them.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{ErrorCode, FieldKind, SettingsField, SettingsSection};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// A name from the protobufs as words: `external_notification` reads as
/// "External notification".
pub fn words(name: &str) -> String {
    let spaced = name.replace(['_', '.'], " ");
    let mut chars = spaced.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The sections, as read from the radio: the configuration's, then the
/// modules'.
#[component]
pub fn Sections(sections: RwSignal<Vec<SettingsSection>>) -> impl IntoView {
    let i18n = use_i18n();
    let of = move |module: bool| {
        move || {
            sections.with(|all| {
                all.iter()
                    .filter(|s| s.module == module)
                    .cloned()
                    .collect::<Vec<_>>()
            })
        }
    };
    view! {
        <section class="card sections">
            <h2>{t!(i18n, sections_device)}</h2>
            <For each=of(false) key=|s| format!("{s:?}") let:section>
                <SectionForm section />
            </For>
        </section>
        <section class="card sections">
            <h2>{t!(i18n, sections_modules)}</h2>
            <For each=of(true) key=|s| format!("{s:?}") let:section>
                <SectionForm section />
            </For>
        </section>
    }
}

#[component]
fn SectionForm(section: SettingsSection) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // Only what was touched: the rest goes back as the radio has it.
    let edits = RwSignal::new(BTreeMap::<String, Json>::new());
    let notice = RwSignal::new(None::<Result<(), Failure>>);
    let busy = RwSignal::new(false);
    let id = section.id.clone();
    // Off or on, as the radio has it: what most modules start with.
    let enabled = section
        .fields
        .iter()
        .find(|f| f.name == "enabled")
        .and_then(|f| f.value.as_ref().and_then(Json::as_bool));

    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let values = edits.get_untracked();
        if values.is_empty() {
            return;
        }
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
                edits.set(BTreeMap::new());
            }
            notice.set(Some(outcome.map(|_| ())));
            busy.set(false);
        });
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
        .map(|field| view! { <FieldInput field edits /> })
        .collect_view();

    view! {
        <details class="section">
            <summary>
                <span>{words(&section.name)}</span>
                {enabled.map(|on| {
                    view! {
                        <span class=if on { "badge ready" } else { "badge disabled" }>
                            {if on { t!(i18n, section_on).into_any() } else { t!(i18n, section_off).into_any() }}
                        </span>
                    }
                })}
                <span class="dim count">{move || {
                    let n = edits.with(BTreeMap::len);
                    (n > 0).then(|| format!("· {n} ✎"))
                }}</span>
            </summary>
            // Checked by the daemon, which says what is wrong: a browser left
            // to it refuses silently, as it does a setting at zero below a
            // floor the firmware reads as "the default".
            <form novalidate on:submit=save>
                <div class="fields">{fields}</div>
                {notice_view}
                <div class="buttons">
                    <button
                        type="submit"
                        class="primary"
                        disabled=move || busy.get() || edits.with(BTreeMap::is_empty)
                    >
                        {t!(i18n, settings_save)}
                    </button>
                </div>
            </form>
        </details>
    }
}

/// One field, with the input its kind takes.
#[component]
fn FieldInput(field: SettingsField, edits: RwSignal<BTreeMap<String, Json>>) -> impl IntoView {
    let i18n = use_i18n();
    let name = field.name.clone();
    let label = field.label.clone().unwrap_or_else(|| words(&field.name));
    let title = field.description.clone().unwrap_or_default();
    let unit = field.unit.clone();
    let original = field.value.clone().unwrap_or(Json::Null);

    // What it shows: the edit if there is one, what the radio has otherwise.
    let current = {
        let name = name.clone();
        let original = original.clone();
        move || {
            edits
                .with(|e| e.get(&name).cloned())
                .unwrap_or_else(|| original.clone())
        }
    };
    // An edit back to what the radio has is no edit.
    let put = {
        let name = name.clone();
        move |value: Json| {
            edits.update(|e| {
                if value == original {
                    e.remove(&name);
                } else {
                    e.insert(name.clone(), value);
                }
            });
        }
    };

    let input = match field.kind {
        FieldKind::Bool => {
            let current = current.clone();
            view! {
                <input
                    type="checkbox"
                    prop:checked=move || current().as_bool().unwrap_or(false)
                    on:change=move |ev| put(Json::Bool(event_target_checked(&ev)))
                />
            }
            .into_any()
        }
        FieldKind::Choice => {
            let current = current.clone();
            let choices = field.choices.clone();
            view! {
                <select
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
            view! {
                <input
                    type="password"
                    autocomplete="new-password"
                    placeholder=move || {
                        if set { t_string!(i18n, secret_set) } else { t_string!(i18n, secret_unset) }
                    }
                    on:input=move |ev| {
                        let text = event_target_value(&ev);
                        // Left empty, the secret stays as it is.
                        edits.update(|e| {
                            if text.is_empty() {
                                e.remove(&name);
                            } else {
                                e.insert(name.clone(), Json::String(text));
                            }
                        });
                    }
                />
            }
            .into_any()
        }
        FieldKind::Text => {
            let current = current.clone();
            view! {
                <input
                    prop:value=move || current().as_str().unwrap_or_default().to_owned()
                    on:input=move |ev| put(Json::String(event_target_value(&ev)))
                />
            }
            .into_any()
        }
    };

    let check = field.kind == FieldKind::Bool;
    view! {
        <label class=if check { "check" } else { "" } title=title>
            {(!check).then(|| view! {
                <span>
                    {label.clone()}
                    {unit.clone().map(|u| format!(" ({u})"))}
                    {field.diy_only.then_some(" · DIY")}
                </span>
            })}
            {input}
            {check.then(|| view! {
                <span>{label.clone()}{field.diy_only.then_some(" · DIY")}</span>
            })}
        </label>
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
