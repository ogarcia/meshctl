// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! How the interface looks and speaks. The daemon keeps it, so every browser
//! that opens meshctl finds it as it was left, and each one open changes with
//! it at once.

use crate::app::Live;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::{Accent, Language, MapColors, MapEngine, MapTheme, Preferences, Theme};

/// The accents, each as its swatch shows it: as it looks on the dark theme,
/// the brightest; gray, between its light and its dark, to be seen on both.
const ACCENTS: [(Accent, &str); 6] = [
    (Accent::Amber, "#f0a63a"),
    (Accent::Blue, "#4dabf7"),
    (Accent::Teal, "#38d9a9"),
    (Accent::Violet, "#9775fa"),
    (Accent::Coral, "#ff8a6b"),
    (Accent::Gray, "#868e96"),
];

#[component]
pub fn PreferencesView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let current = move || live.preferences.get();
    // Each option changes one preference, and leaves the rest as they are.
    let choose = move |edit: Box<dyn Fn(&mut Preferences) + Send + Sync>| {
        move || {
            let mut preferences = live.preferences.get_untracked();
            edit(&mut preferences);
            live.prefer(preferences);
        }
    };

    let themes = [Theme::System, Theme::Light, Theme::Dark].map(|theme| {
        let label = move || match theme {
            Theme::System => t_string!(i18n, theme_system).to_string(),
            Theme::Light => t_string!(i18n, theme_light).to_string(),
            Theme::Dark => t_string!(i18n, theme_dark).to_string(),
        };
        let pick = choose(Box::new(move |p| p.theme = theme));
        (
            Signal::derive(move || current().theme == theme),
            Box::new(pick) as Box<dyn Fn() + Send + Sync>,
            Signal::derive(label),
        )
    });
    let maps = [MapTheme::Page, MapTheme::Light, MapTheme::Dark].map(|map| {
        let label = move || match map {
            MapTheme::Page => t_string!(i18n, map_page).to_string(),
            MapTheme::Light => t_string!(i18n, map_light).to_string(),
            MapTheme::Dark => t_string!(i18n, map_dark).to_string(),
        };
        let pick = choose(Box::new(move |p| p.map_theme = map));
        (
            Signal::derive(move || current().map_theme == map),
            Box::new(pick) as Box<dyn Fn() + Send + Sync>,
            Signal::derive(label),
        )
    });
    let map_engines = [MapEngine::Png, MapEngine::Vector].map(|engine| {
        let label = move || match engine {
            MapEngine::Png => t_string!(i18n, map_engine_png).to_string(),
            MapEngine::Vector => t_string!(i18n, map_engine_vector).to_string(),
        };
        let pick = choose(Box::new(move |p| p.map_engine = engine));
        (
            Signal::derive(move || current().map_engine == engine),
            Box::new(pick) as Box<dyn Fn() + Send + Sync>,
            Signal::derive(label),
        )
    });
    // The map's colours, as its type offers them: OpenStreetMap's tiles in
    // colour or greys; vector maps detailed, vivid or plain. Vivid is in
    // colour on the tiles, which have none of their own.
    let map_colors = move || {
        let vector = current().map_engine == MapEngine::Vector;
        let offered: &[MapColors] = if vector {
            &[MapColors::Full, MapColors::Vivid, MapColors::Gray]
        } else {
            &[MapColors::Full, MapColors::Gray]
        };
        offered
            .iter()
            .map(|&colors| {
                let label = move || match (colors, vector) {
                    (MapColors::Full, false) => t_string!(i18n, map_colors_full).to_string(),
                    (MapColors::Gray, false) => t_string!(i18n, map_colors_gray).to_string(),
                    (MapColors::Full, true) => t_string!(i18n, map_colors_detailed).to_string(),
                    (MapColors::Vivid, _) => t_string!(i18n, map_colors_vivid).to_string(),
                    (MapColors::Gray, true) => t_string!(i18n, map_colors_plain).to_string(),
                };
                let on = move || {
                    let now = current().map_colors;
                    now == colors
                        || (!vector && colors == MapColors::Full && now == MapColors::Vivid)
                };
                let pick = choose(Box::new(move |p| p.map_colors = colors));
                (
                    Signal::derive(on),
                    Box::new(pick) as Box<dyn Fn() + Send + Sync>,
                    Signal::derive(label),
                )
            })
            .collect::<Vec<_>>()
    };
    let accents = ACCENTS.map(|(accent, color)| {
        let on = move || current().accent == accent;
        let name = move || match accent {
            Accent::Amber => t_string!(i18n, accent_amber).to_owned(),
            Accent::Blue => t_string!(i18n, accent_blue).to_owned(),
            Accent::Teal => t_string!(i18n, accent_teal).to_owned(),
            Accent::Violet => t_string!(i18n, accent_violet).to_owned(),
            Accent::Coral => t_string!(i18n, accent_coral).to_owned(),
            Accent::Gray => t_string!(i18n, accent_gray).to_owned(),
        };
        view! {
            <button
                type="button"
                role="radio"
                class="swatch"
                class:on=on
                aria-checked=move || on().to_string()
                title=name
                aria-label=name
                style=format!("--swatch: {color}")
                on:click=move |_| choose(Box::new(move |p| p.accent = accent))()
            ></button>
        }
    });

    let language_value = move || language_code(current().language);
    let pick_language = move |ev: leptos::ev::Event| {
        let language = language_of(&event_target_value(&ev));
        choose(Box::new(move |p| p.language = language))();
    };

    view! {
        <section class="preferences">
            <Row title=t!(i18n, prefs_theme).into_any() hint=t!(i18n, prefs_theme_hint).into_any()>
                {pills(Vec::from(themes))}
            </Row>
            <Row title=t!(i18n, prefs_map).into_any() hint=t!(i18n, prefs_map_hint).into_any()>
                {pills(Vec::from(maps))}
            </Row>
            <Row
                title=t!(i18n, prefs_map_engine).into_any()
                hint=t!(i18n, prefs_map_engine_hint).into_any()
            >
                {pills(Vec::from(map_engines))}
            </Row>
            <Row
                title=t!(i18n, prefs_map_colors).into_any()
                hint=(move || {
                    if current().map_engine == MapEngine::Vector {
                        t!(i18n, prefs_map_colors_vector_hint).into_any()
                    } else {
                        t!(i18n, prefs_map_colors_hint).into_any()
                    }
                })
                .into_any()
            >
                {move || pills(map_colors())}
            </Row>
            <Row title=t!(i18n, prefs_accent).into_any() hint=t!(i18n, prefs_accent_hint).into_any()>
                <div class="pref-control accents" role="radiogroup">
                    {accents.into_iter().collect_view()}
                </div>
            </Row>
            <Row
                title=t!(i18n, prefs_language).into_any()
                hint=t!(i18n, prefs_language_hint).into_any()
            >
                <select class="pref-control" prop:value=language_value on:change=pick_language>
                    {language_options()}
                </select>
            </Row>
        </section>
    }
}

/// One preference: what it is, on the left, and its control on the right.
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

/// One option: whether it is the one chosen, choosing it, and its name.
pub type Choice = (Signal<bool>, Box<dyn Fn() + Send + Sync>, Signal<String>);

/// A few options, of which one is chosen, under a pill that slides to it:
/// each as wide as its word, the room left shared out. Its word again, in
/// bold and unseen, keeps it as wide chosen as not.
pub fn pills(options: Vec<Choice>) -> impl IntoView {
    let n = options.len();
    let ons: Vec<Signal<bool>> = options.iter().map(|(on, _, _)| *on).collect();
    let at = Memo::new(move |_| ons.iter().position(|on| on.get()).unwrap_or(0));
    let strip = NodeRef::<leptos::html::Div>::new();
    let pill = slide(strip, at);
    let buttons = options
        .into_iter()
        .map(|(on, pick, label)| {
            view! {
                <button
                    type="button"
                    role="radio"
                    data-label=move || label.get()
                    class:on=move || on.get()
                    aria-checked=move || on.get().to_string()
                    on:click=move |_| pick()
                >
                    {move || label.get()}
                </button>
            }
        })
        .collect_view();
    view! {
        <div
            class="segmented fitted pref-control"
            role="radiogroup"
            node_ref=strip
            style=move || format!("--n: {n}; --at: {}; {}", at.get(), pill.get())
        >
            {buttons}
        </div>
    }
}

/// The languages meshctl speaks, each in its own words, by their names:
/// a list and not pills, for there will be more than fit side by side.
pub const LANGUAGES: [(Language, &str, &str); 2] = [
    (Language::En, "en", "English"),
    (Language::Es, "es", "Español"),
];

/// A language's value in a list of them; none, the browser's.
pub fn language_code(language: Option<Language>) -> &'static str {
    language.map_or("", |language| {
        LANGUAGES
            .iter()
            .find(|(l, _, _)| *l == language)
            .map_or("", |(_, code, _)| code)
    })
}

/// The language of a value in a list of them.
pub fn language_of(code: &str) -> Option<Language> {
    LANGUAGES
        .iter()
        .find(|(_, c, _)| *c == code)
        .map(|(l, _, _)| *l)
}

/// The options of a list of languages: the browser's first, then each.
pub fn language_options() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <option value="">{t!(i18n, language_browser)}</option>
        {LANGUAGES
            .iter()
            .map(|(_, code, name)| view! { <option value=*code>{*name}</option> })
            .collect_view()}
    }
}

/// Where the pill of a strip of options goes and how wide, as the one at
/// `at` is laid out (`--x` and `--w`, for its style). It slides as another
/// is chosen, or the language changes; as the strip changes size (the
/// window's, its fonts loaded, a phone turned, or shown after being hidden)
/// it is placed at once, before the screen is drawn, and does not stretch
/// to it.
pub fn slide(strip: NodeRef<leptos::html::Div>, at: Memo<usize>) -> RwSignal<String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let i18n = use_i18n();
    let pill = RwSignal::new(String::new());
    // Whether it slides: not until it has been placed and seen once.
    let sliding = RwSignal::new(false);

    // The one at `at`, measured: none while hidden, as it measures nothing.
    let place = move |at: usize, slides: bool| {
        let Some(strip) = strip.get_untracked() else {
            return;
        };
        let Some(option) = strip
            .children()
            .item(at as u32)
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            .filter(|o| o.offset_width() > 0)
        else {
            return;
        };
        pill.set(format!(
            "--x: {}px; --w: {}px; --shown: 1{}",
            option.offset_left(),
            option.offset_width(),
            if slides { "; --slide: 0.2s" } else { "" }
        ));
        // Sliding again, or for the first time, from the next frame on:
        // from where it is now.
        if !slides {
            request_animation_frame(move || {
                sliding.set(true);
                pill.update(|p| {
                    if !p.contains("--slide") {
                        p.push_str("; --slide: 0.2s");
                    }
                });
            });
        }
    };

    // Another chosen, or the language changed: once laid out, sliding.
    Effect::new(move |_| {
        let at = at.get();
        let _ = i18n.get_locale();
        if strip.get().is_none() {
            return;
        }
        request_animation_frame(move || {
            if let Some(sliding) = sliding.try_get_untracked() {
                place(at, sliding);
            }
        });
    });

    // Its size changed: placed at once, as its size is known and before
    // the screen is drawn. What watches it, and what it calls, kept to be
    // let go with it.
    let watch = StoredValue::new_local(None::<(web_sys::ResizeObserver, Closure<dyn FnMut()>)>);
    Effect::new(move |_| {
        let Some(element) = strip.get() else { return };
        if watch.with_value(Option::is_some) {
            return;
        }
        let call = Closure::<dyn FnMut()>::new(move || place(at.get_untracked(), false));
        if let Ok(observer) = web_sys::ResizeObserver::new(call.as_ref().unchecked_ref()) {
            observer.observe(&element);
            watch.set_value(Some((observer, call)));
        }
    });
    on_cleanup(move || {
        watch.update_value(|w| {
            if let Some((observer, _)) = w.take() {
                observer.disconnect();
            }
        });
    });
    pill
}
