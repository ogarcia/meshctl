// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What MeshCore's apps share in a conversation, which goes as text, as
//! MeshCore has nothing else: a contact as `<its key in hex:its kind:its
//! name>`, a channel as its `meshcore://channel/add` link, a place as
//! `latitude,longitude` to the millionth. Read in what is heard, to be shown
//! as what it is; and written the same, so that their apps show it too.
//!
//! Meshtastic's apps share contacts and channels by their links, which go as
//! a message's text as well; its positions, by a packet of their own.

use crate::app::Live;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{Node, Peer};

/// Something shared, as a message's text holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Shared {
    /// A node: its key, 64 hex digits in lower case; its kind, 1 a chat's,
    /// 2 a repeater's, 3 a room's, 4 a sensor's; and its name.
    Contact { key: String, kind: u8, name: String },
    /// A channel: its link, as it came, and its name.
    Channel { link: String, name: String },
    /// A place.
    Place { lat: f64, lon: f64 },
    /// A Meshtastic link, a contact's or channels', read by the daemon.
    Link(String),
}

/// What a message's text shares, if that is all it holds.
pub fn read(text: &str) -> Option<Shared> {
    let text = text.trim();
    contact(text)
        .or_else(|| channel(text))
        .or_else(|| place(text))
}

/// What a Meshtastic message's text shares, if it is only a link of its
/// apps: a contact's, `https://meshtastic.org/v/#…`, or channels',
/// `https://meshtastic.org/e/…#…`.
pub fn read_meshtastic(text: &str) -> Option<Shared> {
    let text = text.trim();
    let rest = ["https://meshtastic.org/", "http://meshtastic.org/"]
        .iter()
        .find_map(|start| {
            text.get(..start.len())
                .filter(|s| s.eq_ignore_ascii_case(start))
                .map(|_| &text[start.len()..])
        })?;
    let (path, data) = rest.split_once('#')?;
    let linked = path.starts_with("v/") || path.starts_with("e/") || path == "e" || path == "v";
    (linked && !data.is_empty() && !text.contains(char::is_whitespace))
        .then(|| Shared::Link(text.to_owned()))
}

fn contact(text: &str) -> Option<Shared> {
    let inner = text.strip_prefix('<')?.strip_suffix('>')?;
    let mut parts = inner.splitn(3, ':');
    let key = parts.next()?;
    let kind = parts.next()?.parse::<u8>().ok()?;
    let name = parts.next()?.trim();
    (key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty()).then(|| {
        Shared::Contact {
            key: key.to_ascii_lowercase(),
            kind,
            name: name.to_owned(),
        }
    })
}

fn channel(text: &str) -> Option<Shared> {
    let scheme = "meshcore://channel/add?";
    let query = text
        .get(..scheme.len())
        .filter(|s| s.eq_ignore_ascii_case(scheme))
        .map(|_| &text[scheme.len()..])?;
    if text.contains(char::is_whitespace) {
        return None;
    }
    let name = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("name="))
        .map(decoded)?;
    (!name.is_empty()).then(|| Shared::Channel {
        link: text.to_owned(),
        name,
    })
}

fn place(text: &str) -> Option<Shared> {
    let (lat, lon) = text.split_once(',')?;
    let number = |s: &str| {
        let s = s.trim();
        // Digits, a sign first and a point, as the apps write it: no more.
        let plain = s
            .strip_prefix('-')
            .unwrap_or(s)
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.');
        (plain && s.contains('.'))
            .then(|| s.parse::<f64>().ok())
            .flatten()
    };
    let (lat, lon) = (number(lat)?, number(lon)?);
    (lat.abs() <= 90.0 && lon.abs() <= 180.0).then_some(Shared::Place { lat, lon })
}

/// A link's field, as it is written there: `+` and `%xx` read.
fn decoded(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[i], escaped) {
            (_, Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', None) => {
                out.push(b' ');
                i += 1;
            }
            (byte, None) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A contact to share, as the apps write it.
pub fn contact_text(key: &str, kind: u8, name: &str) -> String {
    format!("<{key}:{kind}:{name}>")
}

/// A place to share, as the apps write it: to the millionth of a degree.
pub fn place_text(lat: f64, lon: f64) -> String {
    format!("{lat:.6},{lon:.6}")
}

/// A contact's link, for the dialog that adds links to read.
fn contact_link(key: &str, kind: u8, name: &str) -> String {
    let name = String::from(js_sys::encode_uri_component(name));
    format!("meshcore://contact/add?name={name}&public_key={key}&type={kind}")
}

/// A node's number, as MeshCore's are: its key's first four bytes.
fn num_of(key: &str) -> Option<u32> {
    u32::from_str_radix(key.get(..8)?, 16).ok()
}

/// A node's kind, by the role it says, as MeshCore numbers them.
fn kind_of(role: Option<&str>) -> u8 {
    match role {
        Some("REPEATER") => 2,
        Some("ROOM") => 3,
        Some("SENSOR") => 4,
        _ => 1,
    }
}

/// A node's role, by MeshCore's kind.
fn role_of(kind: u8) -> &'static str {
    match kind {
        2 => "REPEATER",
        3 => "ROOM",
        4 => "SENSOR",
        _ => "CHAT",
    }
}

/// A node's key in hex, from the base64 the API gives it in.
fn key_hex(node: &Node) -> Option<String> {
    let base64 = node.public_key.as_deref()?;
    let bytes = window().atob(base64).ok()?;
    let hex: String = bytes.chars().map(|c| format!("{:02x}", c as u32)).collect();
    (hex.len() == 64).then_some(hex)
}

/// What a message shares, shown as what it is: a contact or a channel, as
/// a card to add it from; a place, on a small map.
#[component]
pub fn SharedCard(shared: Shared, mine: bool, from: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    match shared {
        Shared::Place { lat, lon } => {
            let at = meshctl_api::Position {
                latitude: lat,
                longitude: lon,
                altitude: None,
                radius: None,
                source: None,
            };
            view! { <crate::chat::Place at mine from point=true /> }.into_any()
        }
        Shared::Contact { key, kind, name } => {
            let num = num_of(&key);
            // Known and kept by the radio, its page; else, added.
            let kept = move || {
                num.is_some_and(|num| {
                    live.nodes
                        .with(|nodes| nodes.get(&num).is_some_and(|n| n.on_radio != Some(false)))
                })
            };
            let link = contact_link(&key, kind, &name);
            let short = format!("{}…{}", &key[..6], &key[58..]);
            let initials = crate::badge::initials(&name);
            let role = role_of(kind);
            view! {
                <div class="shared-card">
                    <span class="shared-mark avatar">{initials}</span>
                    <span class="shared-what">
                        <span class="shared-said">{t!(i18n, shared_contact)}</span>
                        <b>{name}</b>
                        <span class="dim">
                            {move || crate::names::role_name(i18n.get_locale(), role)}
                            " · "
                            <span class="mono">{short.clone()}</span>
                        </span>
                    </span>
                    {move || {
                        if kept() {
                            let num = num.unwrap_or_default();
                            view! {
                                <button type="button" on:click=move |ev| {
                                    ev.stop_propagation();
                                    live.open_node(num);
                                }>
                                    {t!(i18n, shared_open_node)}
                                </button>
                            }
                            .into_any()
                        } else {
                            let link = link.clone();
                            view! {
                                <button type="button" class="primary" on:click=move |ev| {
                                    ev.stop_propagation();
                                    live.offered_link.set(Some(link.clone()));
                                }>
                                    {t!(i18n, node_keep)}
                                </button>
                            }
                            .into_any()
                        }
                    }}
                </div>
            }
            .into_any()
        }
        Shared::Link(link) => view! { <LinkCard link /> }.into_any(),
        Shared::Channel { link, name } => {
            // Whether the radio has it, name and key, as the link says:
            // asked again as its channels change, as when it is added.
            let present = RwSignal::new(None::<bool>);
            let asked = link.clone();
            Effect::new(move |_| {
                live.channels.track();
                let Some(radio) = live.selected.get_untracked() else {
                    return;
                };
                let link = asked.clone();
                spawn_local(async move {
                    if let Ok(meshctl_api::LinkContents::Channels { channels, .. }) =
                        crate::daemon::read_link(radio, &link).await
                    {
                        let _ = present.try_set(Some(channels.iter().all(|c| c.present)));
                    }
                });
            });
            // Its slot, by its name, to see it.
            let slot = {
                let name = name.clone();
                move || {
                    live.channels.with(|channels| {
                        channels
                            .iter()
                            .find(|c| c.role != "DISABLED" && c.name.eq_ignore_ascii_case(&name))
                            .map(|c| c.index)
                    })
                }
            };
            view! {
                <div class="shared-card">
                    <span class="shared-mark channel">"#"</span>
                    <span class="shared-what">
                        <span class="shared-said">{t!(i18n, shared_channel)}</span>
                        <b>{name}</b>
                    </span>
                    {move || match (present.get(), slot()) {
                        (Some(true), Some(channel)) => view! {
                            <button type="button" on:click=move |ev| {
                                ev.stop_propagation();
                                live.open_chat(Peer::Channel { channel });
                            }>
                                {t!(i18n, shared_open_channel)}
                            </button>
                        }
                        .into_any(),
                        _ => {
                            let link = link.clone();
                            view! {
                                <button type="button" class="primary" on:click=move |ev| {
                                    ev.stop_propagation();
                                    live.offered_link.set(Some(link.clone()));
                                }>
                                    {t!(i18n, shared_add_channel)}
                                </button>
                            }
                            .into_any()
                        }
                    }}
                </div>
            }
            .into_any()
        }
    }
}

/// A Meshtastic link shared, as the daemon reads it: a contact, to see or
/// to add; channels, to see or to add, asked again as the radio's change.
#[component]
fn LinkCard(link: String) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let read = RwSignal::new(None::<meshctl_api::LinkContents>);
    let asked = link.clone();
    Effect::new(move |_| {
        live.channels.track();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        let link = asked.clone();
        spawn_local(async move {
            if let Ok(contents) = crate::daemon::read_link(radio, &link).await {
                let _ = read.try_set(Some(contents));
            }
        });
    });
    let offer = move |link: String| {
        view! {
            <button type="button" class="primary" on:click=move |ev| {
                ev.stop_propagation();
                live.offered_link.set(Some(link.clone()));
            }>
                {t!(i18n, shared_add_channel)}
            </button>
        }
    };
    move || {
        let link = link.clone();
        match read.get() {
            None => view! {
                <div class="shared-card">
                    <span class="shared-mark channel">"…"</span>
                    <span class="shared-what">
                        <span class="mono shared-link">{link}</span>
                    </span>
                </div>
            }
            .into_any(),
            Some(meshctl_api::LinkContents::Contact {
                node,
                id,
                long_name,
                short_name,
                ..
            }) => {
                // Known, with its key, its page; else, added.
                let kept = move || {
                    live.nodes.with(|nodes| {
                        nodes
                            .get(&node)
                            .is_some_and(|n| n.on_radio != Some(false) && n.public_key.is_some())
                    })
                };
                view! {
                    <div class="shared-card">
                        <span class="shared-mark avatar">{short_name}</span>
                        <span class="shared-what">
                            <span class="shared-said">{t!(i18n, shared_contact)}</span>
                            <b>{long_name}</b>
                            <span class="dim mono">{id}</span>
                        </span>
                        {move || {
                            if kept() {
                                view! {
                                    <button type="button" on:click=move |ev| {
                                        ev.stop_propagation();
                                        live.open_node(node);
                                    }>
                                        {t!(i18n, shared_open_node)}
                                    </button>
                                }
                                .into_any()
                            } else {
                                let link = link.clone();
                                view! {
                                    <button type="button" class="primary" on:click=move |ev| {
                                        ev.stop_propagation();
                                        live.offered_link.set(Some(link.clone()));
                                    }>
                                        {t!(i18n, node_keep)}
                                    </button>
                                }
                                .into_any()
                            }
                        }}
                    </div>
                }
                .into_any()
            }
            Some(meshctl_api::LinkContents::Channels { channels, .. }) => {
                let present = !channels.is_empty() && channels.iter().all(|c| c.present);
                let names = channels
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                // Its slot, by its name, to see it: an unnamed primary goes
                // by its preset's.
                let first = channels.first().map(|c| c.name.clone()).unwrap_or_default();
                let slot = live.channels.with(|have| {
                    have.iter()
                        .find(|c| c.role != "DISABLED" && c.name.eq_ignore_ascii_case(&first))
                        .or_else(|| have.iter().find(|c| c.index == 0 && c.name.is_empty()))
                        .map(|c| c.index)
                });
                let key = (channels.len() == 1).then(|| channels[0].key);
                view! {
                    <div class="shared-card">
                        <span class="shared-mark channel">"#"</span>
                        <span class="shared-what">
                            <span class="shared-said">
                                {if channels.len() > 1 {
                                    t!(i18n, shared_channels).into_any()
                                } else {
                                    t!(i18n, shared_channel).into_any()
                                }}
                            </span>
                            <b>{names}</b>
                            {key.map(|key| view! {
                                <span class="dim">{crate::channel_settings::key_view(i18n, key)}</span>
                            })}
                        </span>
                        {match (present, slot) {
                            (true, Some(channel)) => view! {
                                <button type="button" on:click=move |ev| {
                                    ev.stop_propagation();
                                    live.open_chat(Peer::Channel { channel });
                                }>
                                    {t!(i18n, shared_open_channel)}
                                </button>
                            }
                            .into_any(),
                            _ => offer(link).into_any(),
                        }}
                    </div>
                }
                .into_any()
            }
        }
    }
}

/// What can be shared from a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sharing {
    Nothing,
    Menu,
    Contact,
    Channel,
    Place,
}

/// The way to share in a conversation, beside what is written: a contact,
/// a channel or a place, each picked and sent as the apps send them. On
/// Meshtastic the place is where the radio is, sent by `position` as its
/// packet, unless `unplaceable` says why not.
#[component]
pub fn ShareButton(
    peer: Peer,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(optional)] position: Option<Callback<()>>,
    #[prop(optional, into)] unplaceable: MaybeProp<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let meshtastic = position.is_some();
    let sharing = RwSignal::new(Sharing::Nothing);
    let closing = window_event_listener(leptos::ev::click, move |_| {
        if sharing.get_untracked() == Sharing::Menu {
            sharing.set(Sharing::Nothing);
        }
    });
    let escaping = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            sharing.set(Sharing::Nothing);
        }
    });
    on_cleanup(move || {
        closing.remove();
        escaping.remove();
    });
    let send = move |text: String| {
        sharing.set(Sharing::Nothing);
        spawn_local(async move {
            let _ = crate::chat::send(live, peer, text, None, false, None).await;
        });
    };
    // Where a place starts: where the radio says it is, if it says; set
    // before its dialog opens, which reads it then.
    let (latitude, longitude) = (RwSignal::new(String::new()), RwSignal::new(String::new()));
    let start_place = move || {
        let here = live.status.with_untracked(|s| {
            let num = s.as_ref()?.local.as_ref()?.num;
            live.nodes.with_untracked(|nodes| nodes.get(&num)?.position)
        });
        latitude.set(here.map_or_else(String::new, |at| crate::position::written(at.latitude)));
        longitude.set(here.map_or_else(String::new, |at| crate::position::written(at.longitude)));
    };
    let label = move || t_string!(i18n, shared_share);
    let item = move |what: Sharing, icon: &'static str, words: Signal<String>| {
        view! {
            <button role="menuitem" type="button" on:click=move |ev| {
                ev.stop_propagation();
                if what == Sharing::Place {
                    start_place();
                }
                sharing.set(what);
            }>
                <span class="icon" inner_html=icon></span>
                {move || words.get()}
            </button>
        }
    };
    view! {
        <button
            type="button"
            class="icon-button share-toggle"
            class:on=move || sharing.get() == Sharing::Menu
            disabled=move || disabled.get()
            aria-haspopup="menu"
            aria-expanded=move || (sharing.get() == Sharing::Menu).to_string()
            title=label
            aria-label=label
            inner_html=PLUS
            on:click=move |ev| {
                ev.stop_propagation();
                sharing.update(|s| {
                    *s = if *s == Sharing::Menu { Sharing::Nothing } else { Sharing::Menu };
                });
            }
        ></button>
        <Show when=move || sharing.get() == Sharing::Menu>
            <div class="heading-menu share-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                {item(Sharing::Contact, CONTACT, Signal::derive(move || t_string!(i18n, shared_send_contact).to_owned()))}
                {item(Sharing::Channel, HASH, Signal::derive(move || t_string!(i18n, shared_send_channel).to_owned()))}
                {match position {
                    None => item(Sharing::Place, crate::position::PIN, Signal::derive(move || t_string!(i18n, shared_send_place).to_owned())).into_any(),
                    Some(position) => view! {
                        <button
                            role="menuitem"
                            type="button"
                            disabled=move || unplaceable.get().is_some()
                            title=move || unplaceable.get()
                            on:click=move |ev| {
                                ev.stop_propagation();
                                sharing.set(Sharing::Nothing);
                                position.run(());
                            }
                        >
                            <span class="icon" inner_html=crate::position::PIN></span>
                            {move || t_string!(i18n, chat_position_send)}
                        </button>
                    }
                    .into_any(),
                }}
            </div>
        </Show>
        <Show when=move || sharing.get() == Sharing::Contact>
            <ContactPicker
                meshtastic
                pick=Callback::new(move |text: String| send(text))
                close=Callback::new(move |()| sharing.set(Sharing::Nothing))
            />
        </Show>
        <Show when=move || sharing.get() == Sharing::Channel>
            <ChannelPicker
                meshtastic
                pick=Callback::new(move |text: String| send(text))
                close=Callback::new(move |()| sharing.set(Sharing::Nothing))
            />
        </Show>
        <Show when=move || sharing.get() == Sharing::Place>
            <crate::position::PositionDialog
                latitude
                longitude
                altitude=None
                changed=Callback::new(move |()| {
                    let (lat, lon) = (latitude.get_untracked(), longitude.get_untracked());
                    if let (Some(lat), Some(lon)) = (
                        crate::position::coordinate(&lat),
                        crate::position::coordinate(&lon),
                    ) {
                        send(place_text(lat, lon));
                    }
                })
                close=Callback::new(move |()| sharing.set(Sharing::Nothing))
                title=Signal::derive(move || t_string!(i18n, shared_send_place).to_owned())
                accept_label=Signal::derive(move || t_string!(i18n, shared_send).to_owned())
            />
        </Show>
    }
}

/// The nodes to share, the radio's own first, then by name: those whose
/// key is known. On MeshCore as its apps write a contact; on Meshtastic, by
/// its link, which the daemon makes.
#[component]
fn ContactPicker(meshtastic: bool, pick: Callback<String>, close: Callback<()>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let filter = RwSignal::new(String::new());
    let failed = RwSignal::new(false);
    let own = live
        .status
        .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
    let nodes = move || {
        let needle = filter.get().to_lowercase();
        let mut shown: Vec<(bool, String, String, Option<String>, u32)> =
            live.nodes.with(|nodes| {
                nodes
                    .values()
                    .filter_map(|n| {
                        let name = n.long_name.clone()?;
                        let key = key_hex(n)?;
                        (needle.is_empty() || name.to_lowercase().contains(&needle))
                            .then(|| (Some(n.num) == own, name, key, n.role.clone(), n.num))
                    })
                    .collect()
            });
        shown.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        shown
    };
    let title = Signal::derive(move || t_string!(i18n, shared_send_contact).to_owned());
    view! {
        <Picker title close>
            <input
                class="picker-search"
                type="search"
                placeholder=move || t_string!(i18n, search)
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))
            />
            <ul class="picker-list">
                {move || {
                    nodes()
                        .into_iter()
                        .map(|(mine, name, key, role, num)| {
                            let kind = kind_of(role.as_deref());
                            let text = contact_text(&key, kind, &name);
                            let role = if meshtastic {
                                role.unwrap_or_else(|| "CLIENT".to_owned())
                            } else {
                                role_of(kind).to_owned()
                            };
                            let chosen = move |_| {
                                if !meshtastic {
                                    pick.run(text.clone());
                                    return;
                                }
                                let Some(radio) = live.selected.get_untracked() else {
                                    return;
                                };
                                spawn_local(async move {
                                    let link = if mine {
                                        crate::daemon::contact(radio).await
                                    } else {
                                        crate::daemon::node_contact(radio, num).await
                                    };
                                    match link {
                                        Ok(link) => pick.run(link.url),
                                        Err(_) => {
                                            let _ = failed.try_set(true);
                                        }
                                    }
                                });
                            };
                            view! {
                                <li>
                                    <button type="button" on:click=chosen>
                                        <span class="shared-mark avatar">{crate::badge::initials(&name)}</span>
                                        <span class="shared-what">
                                            <b>{name.clone()}</b>
                                            <span class="dim">
                                                {move || {
                                                    if mine {
                                                        t_string!(i18n, shared_own).to_owned()
                                                    } else {
                                                        crate::names::role_name(i18n.get_locale(), &role)
                                                    }
                                                }}
                                            </span>
                                        </span>
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()
                }}
            </ul>
            <p class="error" class:gone=move || !failed.get()>{t!(i18n, error_failed)}</p>
        </Picker>
    }
}

/// A channel to pick: its slot, its name, and how its key is said.
#[derive(Clone)]
struct Pickable {
    slot: u32,
    name: String,
    said: Said,
}

#[derive(Clone, Copy)]
enum Said {
    Meshcore(meshctl_api::MeshcoreChannelKind),
    Meshtastic(meshctl_api::ChannelKey),
}

/// The radio's channels to share, by their links, each said as the
/// channels' list says it: on MeshCore public, a #name or private; on
/// Meshtastic by its key.
#[component]
fn ChannelPicker(meshtastic: bool, pick: Callback<String>, close: Callback<()>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let failed = RwSignal::new(false);
    let channels = RwSignal::new(None::<Vec<Pickable>>);
    if let Some(radio) = live.selected.get_untracked() {
        spawn_local(async move {
            let read = if meshtastic {
                crate::daemon::settings(radio).await.map(|settings| {
                    settings
                        .channels
                        .iter()
                        .filter(|c| c.role != "DISABLED")
                        .map(|c| Pickable {
                            slot: c.index,
                            name: crate::channel_settings::shown_name(&settings, c),
                            said: Said::Meshtastic(c.key),
                        })
                        .collect()
                })
            } else {
                crate::daemon::meshcore_channels(radio).await.map(|read| {
                    read.channels
                        .into_iter()
                        .filter(|c| !c.name.is_empty())
                        .map(|c| Pickable {
                            slot: c.index,
                            name: c.name,
                            said: Said::Meshcore(c.kind),
                        })
                        .collect()
                })
            };
            match read {
                Ok(read) => {
                    let _ = channels.try_set(Some(read));
                }
                Err(_) => {
                    let _ = failed.try_set(true);
                }
            }
        });
    }
    let share = move |slot: u32| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match crate::daemon::channel_link(radio, slot).await {
                Ok(link) => pick.run(link.url),
                Err(_) => {
                    let _ = failed.try_set(true);
                }
            }
        });
    };
    let title = Signal::derive(move || t_string!(i18n, shared_send_channel).to_owned());
    view! {
        <Picker title close>
            <ul class="picker-list">
                {move || {
                    channels
                        .get()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|c| {
                            let slot = c.slot;
                            let said = match c.said {
                                Said::Meshcore(kind) => {
                                    crate::meshcore_channels::kind_view(i18n, kind).into_any()
                                }
                                Said::Meshtastic(key) => view! {
                                    <span class="dim">{crate::channel_settings::key_view(i18n, key)}</span>
                                }
                                .into_any(),
                            };
                            view! {
                                <li>
                                    <button type="button" on:click=move |_| share(slot)>
                                        <span class="shared-mark channel">"#"</span>
                                        <span class="shared-what">
                                            <b>{c.name.clone()}</b>
                                            {said}
                                        </span>
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()
                }}
            </ul>
            <p class="error" class:gone=move || !failed.get()>{t!(i18n, error_failed)}</p>
        </Picker>
    }
}

/// A list to pick from, over the page: its heading, its way out, and it.
#[component]
fn Picker(title: Signal<String>, close: Callback<()>, children: Children) -> impl IntoView {
    let i18n = use_i18n();
    let close_label = move || t_string!(i18n, radio_cancel);
    view! {
        <div class="modal-back" on:click=move |_| close.run(())>
            <div
                class="card radio-editor modal picker"
                role="dialog"
                aria-modal="true"
                on:click=|ev| ev.stop_propagation()
            >
                <header class="editor-head">
                    <span class="who">
                        <h2>{move || title.get()}</h2>
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
                <div class="modal-body">{children()}</div>
            </div>
        </div>
    }
}

const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const CONTACT: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/user.svg");
const HASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/hash.svg");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_apps_share_is_read() {
        let key = "c0".repeat(32);
        assert_eq!(
            read(&format!("<{key}:1:Solo probando>")),
            Some(Shared::Contact {
                key: key.clone(),
                kind: 1,
                name: "Solo probando".into()
            })
        );
        // A name with a colon keeps it.
        assert!(matches!(
            read(&format!("<{key}:2:Monte: norte>")),
            Some(Shared::Contact { name, .. }) if name == "Monte: norte"
        ));
        assert_eq!(
            read("meshcore://channel/add?name=ACoru%C3%B1a&secret=3542734af0e006d2df48d44586299a4c"),
            Some(Shared::Channel {
                link: "meshcore://channel/add?name=ACoru%C3%B1a&secret=3542734af0e006d2df48d44586299a4c".into(),
                name: "ACoruña".into()
            })
        );
        assert_eq!(
            read("43.362300,-8.411500"),
            Some(Shared::Place {
                lat: 43.3623,
                lon: -8.4115
            })
        );
        assert_eq!(
            read(" -28.756453,152.316431 "),
            Some(Shared::Place {
                lat: -28.756_453,
                lon: 152.316_431
            })
        );
        // What only looks like them, not.
        assert_eq!(read("1,2"), None);
        assert_eq!(read("43.36,-8.41 en la playa"), None);
        assert_eq!(read("95.0,-8.4"), None);
        assert_eq!(read("<abc:1:Ana>"), None);
        assert_eq!(read("hola"), None);
        assert_eq!(read("mira meshcore://channel/add?name=X&secret=00"), None);
    }

    #[test]
    fn meshtastic_links_are_read_alone() {
        let contact = "https://meshtastic.org/v/#CM2rNBISCglDYXNh";
        assert_eq!(read_meshtastic(contact), Some(Shared::Link(contact.into())));
        let channels = "https://meshtastic.org/e/?add=true#CgMSAQE";
        assert_eq!(
            read_meshtastic(channels),
            Some(Shared::Link(channels.into()))
        );
        assert_eq!(
            read_meshtastic("https://meshtastic.org/e/#CgMSAQE"),
            Some(Shared::Link("https://meshtastic.org/e/#CgMSAQE".into()))
        );
        assert_eq!(read_meshtastic("mira https://meshtastic.org/v/#CM2r"), None);
        assert_eq!(read_meshtastic("https://meshtastic.org/docs/#x"), None);
        assert_eq!(read_meshtastic("https://meshtastic.org/e/"), None);
    }

    #[test]
    fn what_is_shared_is_written_as_the_apps_write_it() {
        assert_eq!(place_text(43.3623, -8.4115), "43.362300,-8.411500");
        assert_eq!(contact_text("ab", 1, "Ana"), "<ab:1:Ana>");
        assert_eq!(num_of(&"41484f56".repeat(8)), Some(0x4148_4f56));
    }
}
