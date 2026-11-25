// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A number field's buttons, less and more, in meshctl's look rather than
//! the browser's arrows, which the style sheet hides. Beside the field, in
//! a `.number-field` with it, they step it as it takes it, within its
//! least and most, told as if it were written, so that whatever follows
//! the field follows the buttons too; held, as the browser's own, again
//! and again.

use crate::i18n::{t_string, use_i18n};
use leptos::leptos_dom::helpers::{IntervalHandle, TimeoutHandle};
use leptos::prelude::*;
use std::time::Duration;
use wasm_bindgen::JsCast;

const MINUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/minus.svg");
const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");

/// Steps a field once, up or down, and tells it as written.
fn step(field: &web_sys::HtmlInputElement, up: bool) {
    if field.disabled() || field.read_only() {
        return;
    }
    let attribute = |name: &str| field.get_attribute(name).map(|a| a.trim().to_owned());
    let bound = |name: &str| attribute(name)?.parse::<f64>().ok();
    // Its step, by one when it takes any; to as many decimals as it or the
    // value has, so that neither is rounded away.
    let written = attribute("step").filter(|s| s.parse::<f64>().is_ok_and(|s| s > 0.0));
    let by = written.as_deref().map_or(1.0, |s| s.parse().unwrap_or(1.0));
    let value = field.value();
    let decimals = |text: &str| text.split_once('.').map_or(0, |(_, d)| d.len());
    let decimals = decimals(written.as_deref().unwrap_or("")).max(decimals(value.trim()));
    let next = match value.trim().parse::<f64>() {
        Ok(now) if up => now + by,
        Ok(now) => now - by,
        // Empty: from its least, as the browser does.
        Err(_) => bound("min").unwrap_or(0.0),
    };
    let next = bound("min").map_or(next, |least| next.max(least));
    let next = bound("max").map_or(next, |most| next.min(most));
    field.set_value(&format!("{next:.decimals$}"));
    tell(field, "input");
}

/// Tells a field's listeners it changed, as `input` or `change`.
fn tell(field: &web_sys::HtmlInputElement, kind: &str) {
    let init = web_sys::EventInit::new();
    init.set_bubbles(true);
    if let Ok(event) = web_sys::Event::new_with_event_init_dict(kind, &init) {
        let _ = field.dispatch_event(&event);
    }
}

/// How long a button is held before it steps again, and how often then.
const HOLD: Duration = Duration::from_millis(400);
const REPEAT: Duration = Duration::from_millis(80);

/// Less and more, for the number field beside it: a step on pressing one,
/// and more, quicker, while it is held; the field told it changed once it
/// is let go.
#[component]
pub fn Stepper() -> impl IntoView {
    let i18n = use_i18n();
    // The field being stepped, and the timers that step it while held.
    let held = StoredValue::new_local(None::<web_sys::HtmlInputElement>);
    let waiting = StoredValue::new_local(None::<TimeoutHandle>);
    let repeating = StoredValue::new_local(None::<IntervalHandle>);
    let stop = move || {
        if let Some(wait) = waiting.try_update_value(Option::take).flatten() {
            wait.clear();
        }
        if let Some(every) = repeating.try_update_value(Option::take).flatten() {
            every.clear();
        }
        if let Some(field) = held.try_update_value(Option::take).flatten() {
            tell(&field, "change");
        }
    };
    on_cleanup(stop);
    let button = move |up: bool| {
        let label = move || {
            if up {
                t_string!(i18n, number_more)
            } else {
                t_string!(i18n, number_less)
            }
        };
        let start = move |ev: leptos::ev::PointerEvent| {
            // The main button alone; the field keeps its focus.
            if ev.button() != 0 {
                return;
            }
            ev.prevent_default();
            stop();
            let field = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|e| e.closest(".number-field").ok().flatten())
                .and_then(|f| f.query_selector("input").ok().flatten())
                .and_then(|i| i.dyn_into::<web_sys::HtmlInputElement>().ok());
            let Some(field) = field else {
                return;
            };
            step(&field, up);
            held.set_value(Some(field));
            let wait = set_timeout_with_handle(
                move || {
                    let every = set_interval_with_handle(
                        move || {
                            if let Some(field) = held.try_get_value().flatten() {
                                step(&field, up);
                            }
                        },
                        REPEAT,
                    )
                    .ok();
                    repeating.try_set_value(every);
                },
                HOLD,
            )
            .ok();
            waiting.set_value(wait);
        };
        view! {
            <button
                type="button"
                tabindex="-1"
                title=label
                aria-label=label
                on:pointerdown=start
                on:pointerup=move |_| stop()
                on:pointerleave=move |_| stop()
                on:pointercancel=move |_| stop()
                // No menu on a long press by touch.
                on:contextmenu=|ev| ev.prevent_default()
                inner_html=if up { PLUS } else { MINUS }
            ></button>
        }
    };
    view! { <span class="stepper">{button(false)} {button(true)}</span> }
}
