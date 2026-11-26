// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Where a radio is, written or picked: its coordinates as written, and a
//! map to pick them on, with where this device is at hand.

use crate::app::Live;
use crate::i18n::{t, t_string, use_i18n};
use leptos::html;
use leptos::prelude::*;
use serde::Serialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

const MAP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/map.svg");
const PIN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/filled/map-pin.svg");
const HERE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/current-location.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = picker)]
    fn map_picker(element: &web_sys::HtmlElement, json: &str, moved: &JsValue);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = pickerAt)]
    fn map_picker_at(element: &web_sys::HtmlElement, lat: f64, lon: f64);
    #[wasm_bindgen(js_namespace = meshctlMap, js_name = pickerClose)]
    fn map_picker_close(element: &web_sys::HtmlElement);
}

/// A coordinate as written, with a comma or a point; none if it does not
/// read as one.
pub fn coordinate(text: &str) -> Option<f64> {
    text.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite())
}

/// A coordinate to be written: to the millionth of a degree, about ten
/// centimetres, without the zeros it ends in.
pub fn written(degrees: f64) -> String {
    let text = format!("{degrees:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// What a browser's object has under a name; undefined if nothing.
fn read(object: &JsValue, name: &str) -> JsValue {
    js_sys::Reflect::get(object, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

/// Where a map to pick on first looks.
#[derive(Debug, Serialize)]
struct Start {
    lat: f64,
    lon: f64,
    zoom: f64,
    /// Whether a pin is there already.
    pin: bool,
    icon: &'static str,
}

/// Asking this device where it is, and how it went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Locating {
    Idle,
    Asking,
    /// Not allowed to know.
    Denied,
    /// Allowed, but it could not tell.
    Failed,
}

/// The coordinates of a place, written, and a button to pick them on a
/// map: latitude and longitude, and the altitude when given one to fill.
/// `changed` is told after each change, typed or picked.
#[component]
pub fn PositionInputs(
    latitude: RwSignal<String>,
    longitude: RwSignal<String>,
    #[prop(optional)] altitude: Option<RwSignal<String>>,
    #[prop(into)] changed: Callback<()>,
    /// Whether it is off, as while a GPS says where the radio is.
    #[prop(optional, into)]
    disabled: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let picking = RwSignal::new(false);
    let pick_label = move || t_string!(i18n, position_pick);
    view! {
        <div class="pref-control position-inputs" class:with-altitude=altitude.is_some()>
            <input
                inputmode="decimal"
                disabled=disabled
                placeholder=move || t_string!(i18n, position_latitude)
                aria-label=move || t_string!(i18n, position_latitude)
                prop:value=move || latitude.get()
                on:input=move |ev| {
                    latitude.set(event_target_value(&ev));
                    changed.run(());
                }
            />
            <input
                inputmode="decimal"
                disabled=disabled
                placeholder=move || t_string!(i18n, position_longitude)
                aria-label=move || t_string!(i18n, position_longitude)
                prop:value=move || longitude.get()
                on:input=move |ev| {
                    longitude.set(event_target_value(&ev));
                    changed.run(());
                }
            />
            {altitude
                .map(|altitude| {
                    view! {
                        <input
                            inputmode="numeric"
                            disabled=disabled
                            placeholder=move || t_string!(i18n, position_altitude)
                            aria-label=move || t_string!(i18n, position_altitude)
                            title=move || t_string!(i18n, position_altitude)
                            prop:value=move || altitude.get()
                            on:input=move |ev| {
                                altitude.set(event_target_value(&ev));
                                changed.run(());
                            }
                        />
                    }
                })}
            <button
                type="button"
                class="position-pick"
                disabled=disabled
                title=pick_label
                aria-label=pick_label
                inner_html=MAP
                on:click=move |_| picking.set(true)
            ></button>
        </div>
        <Show when=move || picking.get()>
            <PositionDialog
                latitude
                longitude
                altitude
                changed
                close=Callback::new(move |()| picking.set(false))
            />
        </Show>
    }
}

/// A map to pick where the radio is on: a pin put by a touch or dragged,
/// and where this device is, if it can tell.
#[component]
fn PositionDialog(
    latitude: RwSignal<String>,
    longitude: RwSignal<String>,
    altitude: Option<RwSignal<String>>,
    changed: Callback<()>,
    close: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let written_at = coordinate(&latitude.get_untracked())
        .zip(coordinate(&longitude.get_untracked()))
        .filter(|(lat, lon)| lat.abs() <= 90.0 && lon.abs() <= 180.0);
    let picked = RwSignal::new(written_at);
    // The altitude this device gave with where it is, to be filled too.
    let height = RwSignal::new(None::<i32>);
    let locating = RwSignal::new(Locating::Idle);
    // Only a secure context is let ask: a page over plain HTTP, not.
    let can_locate = window().is_secure_context();

    let element = NodeRef::<html::Div>::new();
    let drawn = StoredValue::new_local(None::<web_sys::HtmlElement>);
    Effect::new(move |_| {
        let Some(div) = element.get() else {
            return;
        };
        if drawn.with_value(Option::is_some) {
            return;
        }
        let div: web_sys::HtmlElement = div.into();
        // Where it is, written; or else the radio's own node or any other
        // it knows the place of, nearby; or else the whole world.
        let start = match written_at {
            Some((lat, lon)) => Start {
                lat,
                lon,
                zoom: 15.0,
                pin: true,
                icon: PIN,
            },
            None => {
                let local = live
                    .status
                    .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
                let near = live.nodes.with_untracked(|nodes| {
                    local
                        .and_then(|num| nodes.get(&num)?.position)
                        .or_else(|| nodes.values().find_map(|n| n.position))
                });
                match near {
                    Some(at) => Start {
                        lat: at.latitude,
                        lon: at.longitude,
                        zoom: 11.0,
                        pin: false,
                        icon: PIN,
                    },
                    None => Start {
                        lat: 30.0,
                        lon: 0.0,
                        zoom: 2.0,
                        pin: false,
                        icon: PIN,
                    },
                }
            }
        };
        let moved = Closure::<dyn Fn(f64, f64)>::new(move |lat, lon| {
            let _ = picked.try_set(Some((lat, lon)));
            // Moved by hand, the device's altitude no longer goes with it.
            let _ = height.try_set(None);
        });
        if let Ok(json) = serde_json::to_string(&start) {
            map_picker(&div, &json, &moved.into_js_value());
        }
        drawn.set_value(Some(div));
    });
    let escape = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            close.run(());
        }
    });
    on_cleanup(move || {
        escape.remove();
        if let Some(div) = drawn.get_value() {
            map_picker_close(&div);
        }
    });

    let locate = move |_| {
        let Ok(geolocation) = window().navigator().geolocation() else {
            locating.set(Locating::Failed);
            return;
        };
        locating.set(Locating::Asking);
        let found = Closure::<dyn Fn(JsValue)>::new(move |found: JsValue| {
            let coords = read(&found, "coords");
            let (Some(lat), Some(lon)) = (
                read(&coords, "latitude").as_f64(),
                read(&coords, "longitude").as_f64(),
            ) else {
                let _ = locating.try_set(Locating::Failed);
                return;
            };
            let _ = locating.try_set(Locating::Idle);
            let _ = picked.try_set(Some((lat, lon)));
            // Rounded to the metre, as the radio keeps it.
            #[allow(clippy::cast_possible_truncation)]
            let _ = height.try_set(read(&coords, "altitude").as_f64().map(|a| a.round() as i32));
            if let Some(div) = drawn.try_get_value().flatten() {
                map_picker_at(&div, lat, lon);
            }
        });
        let failed = Closure::<dyn Fn(JsValue)>::new(move |error: JsValue| {
            // As the standard numbers it.
            let denied = read(&error, "code").as_f64() == Some(1.0);
            let _ = locating.try_set(if denied {
                Locating::Denied
            } else {
                Locating::Failed
            });
        });
        let options = web_sys::PositionOptions::new();
        options.set_enable_high_accuracy(true);
        options.set_timeout(20_000);
        let asked = geolocation.get_current_position_with_error_callback_and_options(
            found.as_ref().unchecked_ref(),
            Some(failed.as_ref().unchecked_ref()),
            &options,
        );
        if asked.is_err() {
            locating.set(Locating::Failed);
        }
        // Each answered once at most; kept until then.
        found.forget();
        failed.forget();
    };

    let accept = move |_| {
        if let Some((lat, lon)) = picked.get_untracked() {
            latitude.set(written(lat));
            longitude.set(written(lon));
            if let (Some(altitude), Some(height)) = (altitude, height.get_untracked()) {
                altitude.set(height.to_string());
            }
            changed.run(());
        }
        close.run(());
    };

    let readout = move || match picked.get() {
        Some((lat, lon)) => format!("{}, {}", written(lat), written(lon)),
        None => t_string!(i18n, position_pick_hint).to_owned(),
    };
    let locate_note = move || {
        if !can_locate {
            return Some(t!(i18n, position_here_insecure).into_any());
        }
        match locating.get() {
            Locating::Idle => None,
            Locating::Asking => Some(t!(i18n, position_here_asking).into_any()),
            Locating::Denied => Some(t!(i18n, position_here_denied).into_any()),
            Locating::Failed => Some(t!(i18n, position_here_failed).into_any()),
        }
    };
    let close_label = move || t_string!(i18n, radio_cancel);

    view! {
        <div class="modal-back">
            <div class="card radio-editor modal position-dialog" role="dialog" aria-modal="true">
                <header class="editor-head">
                    <span class="who">
                        <h2>{t!(i18n, position_pick_title)}</h2>
                    </span>
                    <button
                        type="button"
                        class="close"
                        title=close_label
                        aria-label=close_label
                        on:click=move |_| close.run(())
                        inner_html=CLOSE
                    ></button>
                </header>
                <div class="modal-body">
                    <div class="position-map" node_ref=element></div>
                    <div class="position-under">
                        <span class="position-readout mono" class:dim=move || picked.with(Option::is_none)>
                            {readout}
                        </span>
                        <button
                            type="button"
                            class="position-here"
                            disabled=move || !can_locate || locating.get() == Locating::Asking
                            on:click=locate
                        >
                            <span class="icon" inner_html=HERE></span>
                            {t!(i18n, position_here)}
                        </button>
                    </div>
                    {move || locate_note().map(|note| view! { <p class="hint dim position-note">{note}</p> })}
                </div>
                <div class="buttons">
                    <button type="button" on:click=move |_| close.run(())>
                        {t!(i18n, radio_cancel)}
                    </button>
                    <button
                        type="button"
                        class="primary"
                        disabled=move || picked.with(Option::is_none)
                        on:click=accept
                    >
                        {t!(i18n, position_use)}
                    </button>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{coordinate, written};

    #[test]
    fn coordinates_read_and_written() {
        assert_eq!(coordinate(" 43,3623 "), Some(43.3623));
        assert_eq!(coordinate("-8.4115"), Some(-8.4115));
        assert_eq!(coordinate(""), None);
        assert_eq!(coordinate("NaN"), None);
        assert_eq!(written(43.362_300_4), "43.3623");
        assert_eq!(written(-8.0), "-8");
    }
}
