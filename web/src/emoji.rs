// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! An emoji picker, for a radio's button and the chat's reactions alike. The
//! emojis, and their names in the interface's language to look them up by,
//! come with meshctl (Emojibase's data): they are read the first time it
//! opens, and kept.

use crate::i18n::{Locale, t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use std::cell::RefCell;
use std::sync::Arc;

/// Where the data is served, a directory per language.
const DATA: &str = "/emojibase-data-18.0.0";

/// The most found at once: a search shows the best of them.
const FOUND_MAX: usize = 160;

/// The tab of the emojis most used, when the picker is given them.
const FREQUENT: u8 = u8::MAX;

/// What the most used start as, before any is: those most said in a chat,
/// and the mesh's own.
pub const COMMON: [&str; 18] = [
    "👍", "❤️", "😂", "😮", "😢", "🙏", "👋", "🎉", "📡", "✅", "👌", "🔥", "😊", "🤔", "👀", "💪",
    "🙌", "⚡",
];

/// The groups shown as tabs, each with the emoji that stands for it.
/// Emojibase's group 2 (skin tones and hair) is left out: they are parts.
const GROUPS: [(u8, &str); 9] = [
    (0, "😀"),
    (1, "👋"),
    (3, "🐻"),
    (4, "🍎"),
    (5, "🚗"),
    (6, "⚽"),
    (7, "📡"),
    (8, "❤️"),
    (9, "🏁"),
];

#[derive(Deserialize)]
struct Raw {
    unicode: String,
    label: String,
    #[serde(default)]
    tags: Vec<String>,
    group: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Emoji {
    pub unicode: String,
    pub label: String,
    group: u8,
    /// Its name and tags, folded to be looked up by.
    words: String,
}

/// Text in lower case and without accents: "Corazón" is found as "corazon".
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

fn emojis(raw: Vec<Raw>) -> Vec<Emoji> {
    raw.into_iter()
        .filter_map(|raw| {
            let group = raw.group.filter(|group| *group != 2)?;
            let words = fold(&format!("{} {}", raw.label, raw.tags.join(" ")));
            Some(Emoji {
                unicode: raw.unicode,
                label: raw.label,
                group,
                words,
            })
        })
        .collect()
}

/// The emojis whose words hold every word of the search, those whose name
/// starts with it first.
fn find<'a>(all: &'a [Emoji], search: &str) -> Vec<&'a Emoji> {
    let needles: Vec<String> = fold(search).split_whitespace().map(str::to_owned).collect();
    let mut found: Vec<&Emoji> = all
        .iter()
        .filter(|emoji| needles.iter().all(|needle| emoji.words.contains(needle)))
        .collect();
    if let Some(first) = needles.first() {
        found.sort_by_key(|emoji| !fold(&emoji.label).starts_with(first.as_str()));
    }
    found.truncate(FOUND_MAX);
    found
}

/// Whether this browser draws an emoji: the newest are missing from older
/// fonts, which show a box instead, or nothing. It is drawn once, out of
/// sight, and drawn if it comes out in colour, as emojis do and those boxes
/// do not; with no font of emojis in colour, every one is taken as drawn.
/// Each answer is kept.
pub fn draws(emoji: &str) -> bool {
    use wasm_bindgen::JsCast;
    const SIZE: u32 = 24;
    thread_local! {
        static CANVAS: Option<web_sys::CanvasRenderingContext2d> = {
            let canvas = document()
                .create_element("canvas")
                .ok()
                .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok());
            canvas.and_then(|canvas| {
                canvas.set_width(SIZE);
                canvas.set_height(SIZE);
                // Read back once per emoji: the browser keeps it where that
                // is fast.
                let options = web_sys::ContextAttributes2d::new();
                options.set_will_read_frequently(true);
                let context = canvas
                    .get_context_with_context_options("2d", &options)
                    .ok()
                    .flatten()
                    .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())?;
                context.set_font(&format!("{}px sans-serif", SIZE - 4));
                context.set_text_baseline("top");
                context.set_fill_style_str("#000");
                Some(context)
            })
        };
        static DRAWN: RefCell<std::collections::HashMap<String, bool>> = RefCell::default();
    }
    if let Some(drawn) = DRAWN.with_borrow(|drawn| drawn.get(emoji).copied()) {
        return drawn;
    }
    let coloured = |emoji: &str| {
        CANVAS.with(|context| {
            let Some(context) = context else { return true };
            context.clear_rect(0.0, 0.0, f64::from(SIZE), f64::from(SIZE));
            let _ = context.fill_text(emoji, 0.0, 2.0);
            context
                .get_image_data(0.0, 0.0, f64::from(SIZE), f64::from(SIZE))
                .map(|image| {
                    image.data().as_chunks::<4>().0.iter().any(|&[r, g, b, a]| {
                        a > 0 && (r.abs_diff(g) > 24 || g.abs_diff(b) > 24 || r.abs_diff(b) > 24)
                    })
                })
                .unwrap_or(true)
        })
    };
    // A face every font of emojis has: without it, none is in colour.
    let drawn = !coloured("😀") || coloured(emoji);
    DRAWN.with_borrow_mut(|known| known.insert(emoji.to_owned(), drawn));
    drawn
}

/// Those of the emojis this browser draws.
fn drawable(all: Vec<Emoji>) -> Vec<Emoji> {
    all.into_iter().filter(|e| draws(&e.unicode)).collect()
}

/// The emojis read in a language.
type Read = (&'static str, Arc<Vec<Emoji>>);

thread_local! {
    /// What was read, by language.
    static READ: RefCell<Vec<Read>> = const { RefCell::new(Vec::new()) };
}

async fn load(language: &'static str) -> Option<Arc<Vec<Emoji>>> {
    if let Some(read) = READ.with_borrow(|read| {
        read.iter()
            .find(|(lang, _)| *lang == language)
            .map(|(_, emojis)| emojis.clone())
    }) {
        return Some(read);
    }
    let response = gloo_net::http::Request::get(&format!("{DATA}/{language}/compact.json"))
        .send()
        .await
        .ok()
        .filter(|response| response.ok())?;
    let raw: Vec<Raw> = response.json().await.ok()?;
    let read = Arc::new(drawable(emojis(raw)));
    READ.with_borrow_mut(|cache| cache.push((language, read.clone())));
    Some(read)
}

/// The picker. `picked` is the one chosen, marked as such; `pick` is told of
/// each one chosen.
#[component]
pub fn EmojiPicker(
    #[prop(into)] picked: Signal<Option<String>>,
    #[prop(into)] pick: Callback<String>,
    /// The group it opens on.
    #[prop(default = 0)]
    start: u8,
    /// The emojis most used, the first ones first: given, a tab of its own,
    /// the one it opens on.
    #[prop(optional, into)]
    frequent: Option<Signal<Vec<String>>>,
) -> impl IntoView {
    let i18n = use_i18n();
    // `None` while it is read; `Some(None)` if it could not be.
    let data = RwSignal::new(None::<Option<Arc<Vec<Emoji>>>>);
    let group = RwSignal::new(if frequent.is_some() { FREQUENT } else { start });
    let search = RwSignal::new(String::new());

    Effect::new(move |_| {
        let language = match i18n.get_locale() {
            Locale::es => "es",
            Locale::en => "en",
        };
        spawn_local(async move {
            data.set(Some(load(language).await));
        });
    });

    let group_name = move |group: u8| match group {
        0 => t_string!(i18n, emoji_smileys),
        1 => t_string!(i18n, emoji_people),
        3 => t_string!(i18n, emoji_animals),
        4 => t_string!(i18n, emoji_food),
        5 => t_string!(i18n, emoji_travel),
        6 => t_string!(i18n, emoji_activities),
        7 => t_string!(i18n, emoji_objects),
        8 => t_string!(i18n, emoji_symbols),
        FREQUENT => t_string!(i18n, emoji_frequent),
        _ => t_string!(i18n, emoji_flags),
    };
    let groups = frequent
        .is_some()
        .then_some((FREQUENT, "🕘"))
        .into_iter()
        .chain(GROUPS);
    // Each group where it was left, and one not seen yet at its top: kept
    // as the grid is left, put back once it is drawn.
    let grid = NodeRef::<leptos::html::Div>::new();
    let scrolls = StoredValue::new(Vec::<(u8, i32)>::new());
    let scroll_to = move |top: i32| {
        request_animation_frame(move || {
            if let Some(grid) = grid.try_get_untracked().flatten() {
                grid.set_scroll_top(top);
            }
        });
    };
    let show = move |id: u8| {
        if let Some(element) = grid.get_untracked()
            && search.with_untracked(|s| s.trim().is_empty())
        {
            let (left, top) = (group.get_untracked(), element.scroll_top());
            scrolls.update_value(|scrolls| {
                scrolls.retain(|(g, _)| *g != left);
                scrolls.push((left, top));
            });
        }
        search.set(String::new());
        group.set(id);
        let top = scrolls.with_value(|scrolls| {
            scrolls
                .iter()
                .find(|(g, _)| *g == id)
                .map_or(0, |(_, top)| *top)
        });
        scroll_to(top);
    };
    let tabs = groups.map(|(id, face)| {
        let on = move || search.with(|s| s.trim().is_empty()) && group.get() == id;
        view! {
            <button
                type="button"
                role="tab"
                class:on=on
                aria-selected=move || on().to_string()
                title=move || group_name(id)
                aria-label=move || group_name(id)
                on:click=move |_| {
                    show(id);
                }
            >
                {face}
            </button>
        }
    });

    let shown = move || -> Vec<Emoji> {
        let Some(Some(all)) = data.get() else {
            return Vec::new();
        };
        let needle = search.get();
        if needle.trim().is_empty() {
            let group = group.get();
            if group == FREQUENT {
                // Each by its name; those this browser cannot draw, left out.
                return frequent
                    .map(|f| f.get())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|unicode| all.iter().find(|e| e.unicode == unicode).cloned())
                    .collect();
            }
            all.iter().filter(|e| e.group == group).cloned().collect()
        } else {
            find(&all, &needle).into_iter().cloned().collect()
        }
    };

    view! {
        <div class="emoji-picker">
            <input
                type="search"
                placeholder=move || t_string!(i18n, emoji_search)
                prop:value=move || search.get()
                on:input=move |ev| {
                    // What is found, from the first.
                    search.set(event_target_value(&ev));
                    scroll_to(0);
                }
            />
            <div class="emoji-groups" role="tablist">{tabs.collect_view()}</div>
            <div class="emoji-grid" node_ref=grid>
                {move || match data.get() {
                    None => view! { <p class="dim">{t!(i18n, emoji_loading)}</p> }.into_any(),
                    Some(None) => view! { <p class="dim">{t!(i18n, emoji_failed)}</p> }.into_any(),
                    Some(Some(_)) => view! {
                        <For each=shown key=|emoji| emoji.unicode.clone() let:emoji>
                            {
                                let unicode = emoji.unicode.clone();
                                let on = {
                                    let unicode = unicode.clone();
                                    move || picked.get().as_deref() == Some(unicode.as_str())
                                };
                                view! {
                                    <button
                                        type="button"
                                        class:on=on
                                        title=emoji.label.clone()
                                        aria-label=emoji.label.clone()
                                        on:click=move |_| pick.run(unicode.clone())
                                    >
                                        {emoji.unicode.clone()}
                                    </button>
                                }
                            }
                        </For>
                    }
                    .into_any(),
                }}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emoji(unicode: &str, label: &str, tags: &[&str], group: Option<u8>) -> Raw {
        Raw {
            unicode: unicode.into(),
            label: label.into(),
            tags: tags.iter().map(|t| (*t).into()).collect(),
            group,
        }
    }

    #[test]
    fn emojis_are_found_by_their_words() {
        let all = emojis(vec![
            emoji("🇦", "indicador regional A", &[], None),
            emoji("🏻", "tono de piel claro", &[], Some(2)),
            emoji("❤️", "corazón rojo", &["amor", "corazón"], Some(8)),
            emoji(
                "📡",
                "antena de satélite",
                &["antena", "parabólica"],
                Some(7),
            ),
            emoji("💔", "corazón roto", &["roto"], Some(8)),
            emoji("🫀", "órgano del corazón", &["anatomía"], Some(1)),
        ]);
        // Neither the letters to build flags with, nor the parts.
        assert_eq!(all.len(), 4);
        let found = |search| -> Vec<&str> {
            find(&all, search)
                .iter()
                .map(|e| e.unicode.as_str())
                .collect()
        };
        assert_eq!(found("ANTENA"), ["📡"]);
        assert_eq!(found("parabolica"), ["📡"]);
        // Those named so first.
        assert_eq!(found("corazon"), ["❤️", "💔", "🫀"]);
        assert_eq!(found("corazon roto"), ["💔"]);
        assert!(found("coche").is_empty());
    }
}
