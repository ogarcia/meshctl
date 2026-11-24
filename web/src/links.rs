// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Adding nodes and channels from their links, as the apps share them: one
//! pasted is read at once, what it holds shown, and added once confirmed.
//! Nodes are added from the list of nodes, channels from their settings:
//! each window takes its own kind of link, and says where the other goes.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::names;
use crate::qr::QrScanner;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use meshctl_api::{ChannelKey, ErrorCode, LinkContents, LinkMode, RadioKind};

/// A node added: Tabler's antenna, as the nodes' tab has it, with the plus
/// its `*-plus` icons carry in a corner (`user-plus`'s), which none of its
/// strokes reach.
const ANTENNA_PLUS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 4v8" /><path d="M16 4.5v7" /><path d="M12 5v16" /><path d="M8 5.5v5" /><path d="M4 6v4" /><path d="M20 8h-16" /><path d="M16 19h6" /><path d="M19 16v6" /></svg>"#;
const LINK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/link.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const QRCODE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/qrcode.svg");

/// What a window is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkWant {
    Node,
    Channels,
    /// A configuration's LoRa settings, for the form of the radio's.
    Lora,
}

/// The action of the list of nodes, in its heading, that adds one from its
/// link, as the radios' adds a radio: only with a radio picked.
#[component]
pub fn AddNodeLink() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || {
        live.selected_kind().is_some().then(|| {
            view! {
                <div class="actions">
                    <button
                        type="button"
                        class="primary with-icon"
                        title=move || t_string!(i18n, link_add_node)
                        aria-label=move || t_string!(i18n, link_add_node)
                        on:click=move |_| live.adding_link.set(Some(LinkWant::Node))
                    >
                        <span class="icon" inner_html=ANTENNA_PLUS></span>
                        <span class="label">{t!(i18n, node_add)}</span>
                    </button>
                </div>
            }
        })
    }
}

/// The button by the one that adds a channel, that adds them from a link.
#[component]
pub fn AddChannelsLink() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    view! {
        <button
            type="button"
            class="add"
            on:click=move |_| live.adding_link.set(Some(LinkWant::Channels))
        >
            <span class="icon" inner_html=LINK></span>
            {t!(i18n, link_from)}
        </button>
    }
}

/// The button in a Meshtastic radio's LoRa settings that takes them from a
/// configuration's link, into the form.
#[component]
pub fn LoraLink() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    view! {
        <button
            type="button"
            class="with-icon from-link"
            title=move || t_string!(i18n, link_lora_title)
            aria-label=move || t_string!(i18n, link_lora_title)
            on:click=move |_| live.adding_link.set(Some(LinkWant::Lora))
        >
            <span class="icon" inner_html=LINK></span>
            <span class="label">{t!(i18n, link_from)}</span>
        </button>
    }
}

/// What it says of a failure.
fn failure_words(i18n: I18nContext<Locale>, failure: Failure) -> String {
    match failure {
        Failure::Rejected(ErrorCode::InvalidLink) => t_string!(i18n, link_invalid).to_owned(),
        Failure::Rejected(ErrorCode::NoFreeSlots) => t_string!(i18n, link_no_slots).to_owned(),
        Failure::Rejected(ErrorCode::RadioNotConnected) => {
            t_string!(i18n, chat_not_connected).to_owned()
        }
        Failure::Rejected(ErrorCode::RadioDidNotAnswer) => {
            t_string!(i18n, settings_no_answer).to_owned()
        }
        Failure::Rejected(ErrorCode::RadioRefused) => t_string!(i18n, link_refused).to_owned(),
        _ => t_string!(i18n, error_failed).to_owned(),
    }
}

/// The key of a channel, in words.
fn key_words(i18n: I18nContext<Locale>, key: ChannelKey) -> &'static str {
    match key {
        ChannelKey::None => t_string!(i18n, key_none),
        ChannelKey::Default => t_string!(i18n, key_default),
        ChannelKey::Simple => t_string!(i18n, key_simple),
        ChannelKey::Aes128 => "AES-128",
        ChannelKey::Aes256 => "AES-256",
    }
}

/// A channel's name as the list shows it: MeshCore's `#name`s have their
/// mark already.
fn channel_name(name: &str) -> String {
    if name.starts_with('#') {
        name.to_owned()
    } else {
        format!("# {name}")
    }
}

/// A link shared to meshctl, as an installed app is shared things: what it
/// is for, which mesh's apps make it, and the link itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shared {
    pub want: LinkWant,
    pub kind: RadioKind,
    pub url: String,
}

/// The link among what was shared to meshctl, the address first and then
/// the text and the title, as each app shares it its own way: none if none
/// is a node's or channels' link of either mesh.
pub fn shared_link(parts: &[&str]) -> Option<Shared> {
    parts
        .iter()
        .flat_map(|part| part.split_whitespace())
        .find_map(|word| {
            let word =
                word.trim_matches(|c: char| matches!(c, '<' | '>' | '"' | '\'' | '(' | ')' | ','));
            let lower = word.to_ascii_lowercase();
            let meshtastic = lower
                .strip_prefix("https://meshtastic.org/")
                .or_else(|| lower.strip_prefix("http://meshtastic.org/"));
            let (want, kind) = if let Some(rest) = meshtastic {
                if rest.starts_with("v/") {
                    (LinkWant::Node, RadioKind::Meshtastic)
                } else if rest.starts_with("e/") {
                    (LinkWant::Channels, RadioKind::Meshtastic)
                } else {
                    return None;
                }
            } else {
                let rest = lower.strip_prefix("meshcore://")?;
                if rest.starts_with("contact") {
                    (LinkWant::Node, RadioKind::Meshcore)
                } else if rest.starts_with("channel") {
                    (LinkWant::Channels, RadioKind::Meshcore)
                } else {
                    return None;
                }
            };
            Some(Shared {
                want,
                kind,
                url: word.to_owned(),
            })
        })
}

/// The link shared to meshctl as an installed app, when it was opened for
/// that: at `/share`, with what was shared as the share target in its
/// manifest says. The address goes back to the start at once, so that a
/// reload does not share it again.
fn read_shared() -> Option<Shared> {
    let window = web_sys::window()?;
    let location = window.location();
    if location.pathname().ok()? != "/share" {
        return None;
    }
    let params = web_sys::UrlSearchParams::new_with_str(&location.search().ok()?).ok()?;
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/"));
    }
    let parts: Vec<String> = ["url", "text", "title"]
        .iter()
        .filter_map(|name| params.get(name))
        .collect();
    shared_link(&parts.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Whether a link holds what the window is for.
fn wanted(want: LinkWant, read: &LinkContents) -> bool {
    matches!(
        (want, read),
        (LinkWant::Node, LinkContents::Contact { .. })
            | (LinkWant::Channels, LinkContents::Channels { .. })
            | (LinkWant::Lora, LinkContents::Channels { lora: Some(_), .. })
    )
}

/// The window to paste a link in: read as it is pasted, what it holds
/// shown, and added once confirmed. Escape, its ✕ or a click off it close
/// it.
#[component]
pub fn LinkAdder() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let url = RwSignal::new(String::new());
    let contents = RwSignal::new(None::<LinkContents>);
    let problem = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    // For channels that replace: whether they do, and bring their LoRa.
    let mode = RwSignal::new(LinkMode::Add);
    let lora = RwSignal::new(false);
    // A link shared to meshctl: once the radios are known, through one of
    // the mesh it is of (the selected one if it is), read as if pasted.
    let shared = StoredValue::new(read_shared());
    Effect::new(move |_| {
        if shared.with_value(Option::is_none) {
            return;
        }
        let Some(radios) = live.radios.get() else {
            return;
        };
        let Some(link) = shared.get_value() else {
            return;
        };
        shared.set_value(None);
        if live.selected_kind() != Some(link.kind)
            && let Some(radio) = radios.iter().find(|r| r.kind == link.kind && r.enabled)
        {
            live.select(radio.id);
        }
        url.set(link.url);
        live.adding_link.set(Some(link.want));
    });
    // Whether a QR is being read for it.
    let scanning = RwSignal::new(false);
    let close = move || {
        live.adding_link.set(None);
        scanning.set(false);
        url.set(String::new());
        contents.set(None);
        problem.set(None);
        mode.set(LinkMode::Add);
        lora.set(false);
    };
    let escaping = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" && live.adding_link.get_untracked().is_some() {
            close();
        }
    });
    on_cleanup(move || escaping.remove());
    // Read as it is written or pasted: the latest only.
    let asked = StoredValue::new(0_u32);
    Effect::new(move |_| {
        let link = url.get();
        contents.set(None);
        problem.set(None);
        mode.set(LinkMode::Add);
        lora.set(false);
        let (Some(radio), false) = (live.selected.get_untracked(), link.trim().is_empty()) else {
            return;
        };
        let turn = asked.get_value() + 1;
        asked.set_value(turn);
        spawn_local(async move {
            let read = daemon::read_link(radio, link.trim()).await;
            if asked.get_value() != turn {
                return;
            }
            match read {
                Ok(read) => contents.set(Some(read)),
                Err(failure) => problem.set(Some(failure)),
            }
        });
    });
    let add = move |_| {
        let (Some(radio), Some(read)) = (live.selected.get_untracked(), contents.get_untracked())
        else {
            return;
        };
        // A LoRa taken goes into the form, not to the radio: saved from there.
        if live.adding_link.get_untracked() == Some(LinkWant::Lora) {
            if let LinkContents::Channels {
                lora: Some(lora), ..
            } = read
            {
                live.lora_from_link.set(Some(lora));
                close();
            }
            return;
        }
        let link = meshctl_api::Link {
            url: url.get_untracked().trim().to_owned(),
            mode: mode.get_untracked(),
            lora: mode.get_untracked() == LinkMode::Replace && lora.get_untracked(),
        };
        busy.set(true);
        spawn_local(async move {
            match daemon::add_link(radio, &link).await {
                Ok(()) => {
                    close();
                    // The node added, its page; the channels come as the
                    // radio is read again.
                    if let LinkContents::Contact { node, .. } = read {
                        let _ = live.refresh_selected().await;
                        live.open_node(node);
                    }
                }
                Err(failure) => problem.set(Some(failure)),
            }
            busy.set(false);
        });
    };

    let contact = move |read: LinkContents| {
        let LinkContents::Contact {
            id,
            long_name,
            short_name,
            hw_model,
            key,
            known,
            ..
        } = read
        else {
            return ().into_any();
        };
        view! {
            <div class="link-contents">
                <p class="what">{t!(i18n, link_contact)}</p>
                <div class="link-node">
                    <span class="short">{short_name}</span>
                    <span class="names">
                        <b>{long_name}</b>
                        <span class="mono dim">
                            {id}
                            {hw_model.map(|m| format!(" · {}", crate::mesh::model(&m)))}
                        </span>
                    </span>
                </div>
                {if key {
                    view! { <p class="ok">{t!(i18n, link_key)}</p> }.into_any()
                } else {
                    view! { <p class="warn">{t!(i18n, link_no_key)}</p> }.into_any()
                }}
                {known.then(|| view! { <p class="dim">{t!(i18n, link_known)}</p> })}
            </div>
        }
        .into_any()
    };
    let channels = move |read: LinkContents| {
        let LinkContents::Channels {
            channels,
            free_slots,
            preset,
            lora_differs,
            region,
            replaces,
            removed,
            ..
        } = read
        else {
            return ().into_any();
        };
        let count = channels.len();
        let added = channels.iter().filter(|c| !c.present).count() as u32;
        let fits = added <= free_slots;
        let adding = if added == 0 {
            t_string!(i18n, link_nothing).to_owned()
        } else if fits {
            t_string!(i18n, link_channels_fit)
                .replace("{n}", &added.to_string())
                .replace("{free}", &free_slots.to_string())
        } else {
            t_string!(i18n, link_channels_full)
                .replace("{n}", &added.to_string())
                .replace("{free}", &free_slots.to_string())
        };
        let preset = preset.unwrap_or_default();
        let settings = match &region {
            Some(region) => format!("{preset} · {region}"),
            None => preset.clone(),
        };
        let removed = (!removed.is_empty()).then(|| removed.join(", "));
        let pick = move |picked: LinkMode| {
            view! {
                <button
                    type="button"
                    role="tab"
                    class:on=move || mode.get() == picked
                    aria-selected=move || (mode.get() == picked).to_string()
                    on:click=move |_| mode.set(picked)
                >
                    {match picked {
                        LinkMode::Add => t!(i18n, link_mode_add).into_any(),
                        LinkMode::Replace => t!(i18n, link_mode_replace).into_any(),
                    }}
                </button>
            }
        };
        let replacing = move || mode.get() == LinkMode::Replace;
        view! {
            <div class="link-contents">
                <p class="what">{t!(i18n, link_channels)}</p>
                <ul class="link-channels">
                    {channels
                        .into_iter()
                        .enumerate()
                        .map(|(at, c)| {
                            view! {
                                <li class:present=c.present>
                                    <span class="name">{channel_name(&c.name)}</span>
                                    <span class="dim">
                                        {key_words(i18n, c.key)}
                                        {c.present.then(|| format!(" · {}", t_string!(i18n, link_present)))}
                                        {move || (replacing() && at == 0).then(|| format!(" · {}", t_string!(i18n, link_primary)))}
                                    </span>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
                {replaces.then(|| view! {
                    <p class="dim">{t!(i18n, link_replaces)}</p>
                    <div
                        class="segmented link-mode"
                        role="tablist"
                        style=move || format!("--at: {}; --n: 2", u8::from(replacing()))
                    >
                        {pick(LinkMode::Add)}
                        {pick(LinkMode::Replace)}
                    </div>
                })}
                {move || if replacing() {
                    view! {
                        <p class="warn">
                            {t_string!(i18n, link_replace_summary).replace("{n}", &count.to_string())}
                        </p>
                        {removed.clone().map(|names| view! {
                            <p class="warn">{t_string!(i18n, link_removed).replace("{names}", &names)}</p>
                        })}
                        {lora_differs.then(|| {
                            let settings = settings.clone();
                            view! {
                                <label class="link-lora">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || lora.get()
                                        on:change=move |ev| lora.set(event_target_checked(&ev))
                                    />
                                    <span>
                                        {t_string!(i18n, link_lora_apply).replace("{settings}", &settings)}
                                        <small class="dim">{t!(i18n, link_lora_warning)}</small>
                                    </span>
                                </label>
                            }
                        })}
                    }
                    .into_any()
                } else {
                    view! {
                        <p class:warn=!fits>{adding.clone()}</p>
                        {lora_differs.then(|| view! {
                            <p class="warn">
                                {t_string!(i18n, link_lora_differs).replace("{preset}", &preset)}
                            </p>
                        })}
                    }
                    .into_any()
                }}
            </div>
        }
        .into_any()
    };
    // A link's LoRa: what the form would take, and whether the radio has it.
    let lora_of = move |read: LinkContents| {
        let LinkContents::Channels {
            lora: Some(lora),
            lora_differs,
            ..
        } = read
        else {
            return ().into_any();
        };
        let locale = i18n.get_locale_untracked();
        let modem = if lora.use_preset {
            names::preset(locale, &lora.modem_preset)
        } else {
            format!(
                "{} · SF{} · 4/{}",
                crate::settings::bandwidth_label(lora.bandwidth),
                lora.spread_factor,
                lora.coding_rate
            )
        };
        let rows = [
            (
                t_string!(i18n, settings_region).to_owned(),
                names::region(locale, &lora.region),
            ),
            (t_string!(i18n, link_lora_modem).to_owned(), modem),
            (
                t_string!(i18n, settings_hop_limit).to_owned(),
                lora.hop_limit.to_string(),
            ),
        ];
        view! {
            <div class="link-contents">
                <p class="what">{t!(i18n, link_lora)}</p>
                <dl class="link-lora-values">
                    {rows
                        .into_iter()
                        .map(|(name, value)| view! { <dt class="dim">{name}</dt> <dd>{value}</dd> })
                        .collect_view()}
                </dl>
                {if lora_differs {
                    view! { <p class="warn">{t!(i18n, link_lora_will_change)}</p> }.into_any()
                } else {
                    view! { <p class="dim">{t!(i18n, link_lora_same)}</p> }.into_any()
                }}
            </div>
        }
        .into_any()
    };
    let shown = move || {
        let want = live.adding_link.get()?;
        let read = contents.get()?;
        Some(match (want, read) {
            (LinkWant::Node, read @ LinkContents::Contact { .. }) => contact(read),
            (LinkWant::Channels, read @ LinkContents::Channels { .. }) => channels(read),
            (LinkWant::Lora, read @ LinkContents::Channels { lora: Some(_), .. }) => lora_of(read),
            (LinkWant::Lora, LinkContents::Channels { .. }) => {
                view! { <p class="error">{t!(i18n, link_no_lora)}</p> }.into_any()
            }
            (LinkWant::Lora, _) => {
                view! { <p class="error">{t!(i18n, link_for_lora)}</p> }.into_any()
            }
            // Not what it is for: where it goes.
            (LinkWant::Node, _) => {
                view! { <p class="error">{t!(i18n, link_for_channels)}</p> }.into_any()
            }
            (LinkWant::Channels, _) => {
                view! { <p class="error">{t!(i18n, link_for_nodes)}</p> }.into_any()
            }
        })
    };
    // Whether there is anything to do.
    let can_add = move || {
        let Some(want) = live.adding_link.get() else {
            return false;
        };
        !busy.get()
            && contents.with(|read| match read {
                Some(read) if !wanted(want, read) => false,
                Some(LinkContents::Channels { .. }) if want == LinkWant::Lora => true,
                Some(LinkContents::Contact { .. }) => true,
                Some(LinkContents::Channels { .. }) if mode.get() == LinkMode::Replace => true,
                Some(LinkContents::Channels {
                    channels,
                    free_slots,
                    ..
                }) => {
                    let added = channels.iter().filter(|c| !c.present).count() as u32;
                    added > 0 && added <= *free_slots
                }
                None => false,
            })
    };
    let action = move || match (live.adding_link.get(), mode.get()) {
        (Some(LinkWant::Channels), LinkMode::Replace) => t_string!(i18n, link_replace_channels),
        (Some(LinkWant::Channels), LinkMode::Add) => t_string!(i18n, link_add_channels),
        (Some(LinkWant::Lora), _) => t_string!(i18n, link_use_lora),
        _ => t_string!(i18n, link_add_contact),
    };

    move || {
        live.adding_link.get().map(|want| {
            // Whose apps the links come from, by the radio's kind.
            let meshcore = live.selected_kind() == Some(RadioKind::Meshcore);
            let apps = if meshcore { "MeshCore" } else { "Meshtastic" };
            view! {
                <div class="modal-back" on:click=move |_| close()>
                    <div
                        class="card link-adder"
                        role="dialog"
                        aria-modal="true"
                        on:click=|ev| ev.stop_propagation()
                    >
                        <header>
                            <h2>
                                {match want {
                                    LinkWant::Node => t!(i18n, link_add_node).into_any(),
                                    LinkWant::Channels => t!(i18n, link_add_channels_title).into_any(),
                                    LinkWant::Lora => t!(i18n, link_lora_title).into_any(),
                                }}
                            </h2>
                            <button
                                class="icon-button"
                                title=move || t_string!(i18n, drawer_close)
                                aria-label=move || t_string!(i18n, drawer_close)
                                inner_html=CLOSE
                                on:click=move |_| close()
                            ></button>
                        </header>
                        <div class="modal-body">
                            <div
                                class="segmented link-source"
                                role="tablist"
                                style=move || format!("--at: {}; --n: 2", u8::from(scanning.get()))
                            >
                                <button
                                    type="button"
                                    role="tab"
                                    class:on=move || !scanning.get()
                                    aria-selected=move || (!scanning.get()).to_string()
                                    on:click=move |_| scanning.set(false)
                                >
                                    <span class="icon" inner_html=LINK></span>
                                    {t!(i18n, link_tab_paste)}
                                </button>
                                <button
                                    type="button"
                                    role="tab"
                                    class:on=move || scanning.get()
                                    aria-selected=move || scanning.get().to_string()
                                    on:click=move |_| scanning.set(true)
                                >
                                    <span class="icon" inner_html=QRCODE></span>
                                    {t!(i18n, link_tab_qr)}
                                </button>
                            </div>
                            {move || {
                                if scanning.get() {
                                    // Once read, back to the link, to see what it holds.
                                    let found = Callback::new(move |text: String| {
                                        scanning.set(false);
                                        url.set(text.trim().to_owned());
                                    });
                                    view! { <QrScanner found=found /> }.into_any()
                                } else {
                                    view! {
                                        <textarea
                                            class="mono"
                                            rows="3"
                                            placeholder=match (want, meshcore) {
                                                (LinkWant::Node, false) => "https://meshtastic.org/v/#…",
                                                (LinkWant::Channels | LinkWant::Lora, false) => {
                                                    "https://meshtastic.org/e/#…"
                                                }
                                                (LinkWant::Lora, true) => "",
                                                (LinkWant::Node, true) => "meshcore://contact/add?…",
                                                (LinkWant::Channels, true) => "meshcore://channel/add?…",
                                            }
                                            prop:value=move || url.get()
                                            on:input=move |ev| url.set(event_target_value(&ev))
                                        ></textarea>
                                        <p class="hint dim">
                                            {match want {
                                                LinkWant::Node => t_string!(i18n, link_hint_node),
                                                LinkWant::Lora => t_string!(i18n, link_hint_lora),
                                                LinkWant::Channels => t_string!(i18n, link_hint_channels),
                                            }
                                            .replace("{apps}", apps)}
                                        </p>
                                    }
                                    .into_any()
                                }
                            }}
                            {move || (!scanning.get()).then(shown)}
                            {move || problem.get().map(|p| view! { <p class="error">{failure_words(i18n, p)}</p> })}
                        </div>
                        <div class="buttons">
                            <button type="button" on:click=move |_| close()>
                                {t!(i18n, link_cancel)}
                            </button>
                            <button
                                type="button"
                                class="primary"
                                disabled=move || !can_add()
                                on:click=add
                            >
                                {action}
                            </button>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_shared_is_found_wherever_the_app_put_it() {
        // Alone, as the address.
        let shared = shared_link(&["https://meshtastic.org/v/#CgsSAQ"]).unwrap();
        assert_eq!(
            (shared.want, shared.kind),
            (LinkWant::Node, RadioKind::Meshtastic)
        );
        // In the text, among words.
        let shared = shared_link(&[
            "",
            "Join us: (https://meshtastic.org/e/?add=true#CgsSAQ), see you",
        ])
        .unwrap();
        assert_eq!(shared.want, LinkWant::Channels);
        assert_eq!(shared.url, "https://meshtastic.org/e/?add=true#CgsSAQ");
        // MeshCore's, either kind.
        let contact = shared_link(&["meshcore://contact/add?name=Ana&public_key=41"]).unwrap();
        assert_eq!(
            (contact.want, contact.kind),
            (LinkWant::Node, RadioKind::Meshcore)
        );
        let channel = shared_link(&["meshcore://channel/add?name=Moire&secret=00"]).unwrap();
        assert_eq!(channel.want, LinkWant::Channels);
        // Anything else: none.
        assert_eq!(shared_link(&["https://meshtastic.org/docs", "hello"]), None);
    }
}
