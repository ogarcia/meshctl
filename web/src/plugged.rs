// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Where a radio to add is, as the welcome and the dialog that adds one
//! both ask it: what it is, how meshctl reaches it, and, over the network,
//! its address, or, plugged in, the one picked of those plugged in, which
//! the daemon lists, or its device's path, written by hand.

use crate::daemon;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{NearbyRadio, PluggedRadio, RadioKind};
use std::time::Duration;

const WIFI: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/wifi.svg");
const USB: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/usb.svg");
const BLUETOOTH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/bluetooth.svg");

/// How soon radios nearby are looked for again while Android asks for
/// leave to use Bluetooth.
const ASKED_AGAIN: Duration = Duration::from_secs(3);

/// How often the radios plugged in are read again, while they are shown.
const PLUGGED_EVERY: Duration = Duration::from_secs(3);

/// How meshctl reaches the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Network,
    Usb,
    Bluetooth,
}

/// Where a radio to add is, as it is asked.
#[derive(Clone, Copy)]
pub struct Place {
    pub kind: RwSignal<RadioKind>,
    pub via: RwSignal<Via>,
    pub address: RwSignal<String>,
    /// The name of the radio picked of those plugged in, to name it by if
    /// it is given none.
    pub picked_name: RwSignal<Option<String>>,
}

/// How an address reaches its radio: a device's path, or USB, is plugged
/// in; anything else, over the network.
pub fn via_of(address: &str) -> Via {
    let address = address.trim();
    let port = address.to_ascii_uppercase();
    let port = port.split('@').next().unwrap_or(&port);
    let port = port.strip_prefix(r"\\.\").unwrap_or(port);
    if address.starts_with("ble://") {
        return Via::Bluetooth;
    }
    if address.starts_with('/')
        || address.starts_with("serial://")
        || address.starts_with("usb")
        || (port.starts_with("COM")
            && port[3..].chars().all(|c| c.is_ascii_digit())
            && port.len() > 3)
    {
        Via::Usb
    } else {
        Via::Network
    }
}

impl Place {
    /// Where a radio already added is, to change it.
    pub fn of(kind: RadioKind, address: &str) -> Self {
        Self {
            kind: RwSignal::new(kind),
            via: RwSignal::new(via_of(address)),
            address: RwSignal::new(address.to_owned()),
            picked_name: RwSignal::new(None),
        }
    }

    pub fn new() -> Self {
        Self {
            kind: RwSignal::new(RadioKind::Meshtastic),
            via: RwSignal::new(Via::Network),
            address: RwSignal::new(String::new()),
            picked_name: RwSignal::new(None),
        }
    }

    /// The address as the daemon takes it: a device's path written by its
    /// name alone, as `ttyACM0`, under /dev.
    pub fn address(self) -> String {
        let address = self.address.get_untracked().trim().to_owned();
        let bare = !address.is_empty()
            && !address.starts_with('/')
            && !address.starts_with('\\')
            && !address.starts_with("usb")
            && !address.to_ascii_uppercase().starts_with("COM");
        match self.via.get_untracked() {
            Via::Usb if bare => format!("/dev/{address}"),
            _ => address,
        }
    }
}

/// One of [`cards`]: what it picks, its icon if any, its name and its line.
pub type Card<T> = (T, Option<&'static str>, Signal<String>, Signal<String>);

/// Two big choices side by side, as cards: a name, a line under it and,
/// maybe, an icon.
pub fn cards<T: Copy + PartialEq + Send + Sync + 'static>(
    value: RwSignal<T>,
    options: Vec<Card<T>>,
) -> impl IntoView {
    // Three or more: as rows by their icons, across or one under another.
    let many = options.len() > 2;
    let buttons = options
        .into_iter()
        .map(|(this, icon, title, detail)| {
            let on = move || value.get() == this;
            view! {
                <button
                    type="button"
                    role="radio"
                    class="welcome-card"
                    class:on=on
                    aria-checked=move || on().to_string()
                    on:click=move |_| value.set(this)
                >
                    {icon.map(|icon| view! { <span class="icon" inner_html=icon></span> })}
                    <span class="title">{title}</span>
                    <span class="detail">{detail}</span>
                </button>
            }
        })
        .collect_view();
    view! { <div class="welcome-cards" class:many=many role="radiogroup">{buttons}</div> }
}

/// What the radio is, how meshctl reaches it, and where.
#[component]
pub fn PlaceFields(
    place: Place,
    #[prop(optional)] input: NodeRef<leptos::html::Input>,
) -> impl IntoView {
    let i18n = use_i18n();
    let Place {
        kind, via, address, ..
    } = place;
    let kinds = cards(
        kind,
        vec![
            (
                RadioKind::Meshtastic,
                None,
                Signal::derive(|| "Meshtastic".to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_meshtastic_detail).to_owned()),
            ),
            (
                RadioKind::Meshcore,
                None,
                Signal::derive(|| "MeshCore".to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_meshcore_detail).to_owned()),
            ),
        ],
    );
    // Bluetooth, where there is a way to reach radios by it: an app's.
    let bluetooth = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(ways) = daemon::ways().await {
            bluetooth.try_set(ways.bluetooth);
        }
    });
    let vias = move || {
        let mut ways = vec![
            (
                Via::Network,
                Some(WIFI),
                Signal::derive(move || t_string!(i18n, welcome_network).to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_network_detail).to_owned()),
            ),
            (
                Via::Usb,
                Some(USB),
                Signal::derive(move || t_string!(i18n, welcome_usb).to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_usb_detail).to_owned()),
            ),
        ];
        if bluetooth.get() || via.get_untracked() == Via::Bluetooth {
            ways.push((
                Via::Bluetooth,
                Some(BLUETOOTH),
                Signal::derive(move || t_string!(i18n, welcome_bluetooth).to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_bluetooth_detail).to_owned()),
            ));
        }
        cards(via, ways)
    };
    // Another way to reach it takes the address it had, if any: going back
    // and forth loses none, while this is open.
    let kept = StoredValue::new([String::new(), String::new(), String::new()]);
    let slot = |via: Via| match via {
        Via::Network => 0,
        Via::Usb => 1,
        Via::Bluetooth => 2,
    };
    Effect::new(move |before: Option<Via>| {
        let now = via.get();
        if let Some(before) = before.filter(|before| *before != now) {
            kept.update_value(|kept| {
                kept[slot(before)] = address.get_untracked();
                address.set(kept[slot(now)].clone());
            });
            place.picked_name.set(None);
        }
        now
    });
    let hint = move || {
        let port = match kind.get() {
            RadioKind::Meshtastic => 4403,
            RadioKind::Meshcore => 5000,
        };
        t_string!(i18n, welcome_address_hint).replace("{port}", &port.to_string())
    };
    view! {
        <fieldset>
            <legend>{t!(i18n, welcome_kind)}</legend>
            {kinds}
        </fieldset>
        <fieldset>
            <legend>{t!(i18n, welcome_via)}</legend>
            {vias}
        </fieldset>
        <Show when=move || via.get() == Via::Usb>
            <PluggedPicker place />
        </Show>
        <Show when=move || via.get() == Via::Bluetooth>
            <NearbyPicker place />
        </Show>
        <Show when=move || via.get() == Via::Network>
            <label class="field">
                <span class="label">{t!(i18n, welcome_address)}</span>
                <input
                    node_ref=input
                    class="mono"
                    required
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    placeholder="192.168.1.10"
                    prop:value=move || address.get()
                    on:input=move |ev| address.set(event_target_value(&ev))
                />
                <span class="hint">{hint}</span>
            </label>
        </Show>
    }
}

/// What a radio plugged in is called: its product, or its chip, or what
/// it is.
fn plugged_name(
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    radio: &PluggedRadio,
) -> String {
    radio
        .product
        .clone()
        .or_else(|| radio.chip.clone())
        .unwrap_or_else(|| t_string!(i18n, plugged_unknown).to_owned())
}

/// The radios plugged in where the daemon runs, read again and again while
/// shown, to pick one from; or its device's path written by hand, where
/// there are paths to write.
#[component]
fn PluggedPicker(place: Place) -> impl IntoView {
    let i18n = use_i18n();
    let address = place.address;
    let plugged = RwSignal::new(None::<Vec<PluggedRadio>>);
    // Within an app that opens radios by USB, as Android's, there is no
    // path to write by hand.
    let app = RwSignal::new(false);
    let by_hand = RwSignal::new(false);
    // The one picked that waits for the user's leave, Android's to ask.
    let asked = RwSignal::new(None::<String>);
    let read = move || {
        spawn_local(async move {
            if let Ok(read) = daemon::plugged().await {
                // An address given before, a path none of them has: as
                // written by hand, the first time, where there are paths.
                if plugged.get_untracked().is_none()
                    && !app.get_untracked()
                    && address.with_untracked(|a| {
                        !a.trim().is_empty() && !read.iter().any(|p| p.address == *a)
                    })
                {
                    by_hand.try_set(true);
                }
                // One just let be used: no longer waiting.
                if let Some(waiting) = asked.get_untracked()
                    && read.iter().any(|p| p.address == waiting && p.ready)
                {
                    asked.try_set(None);
                }
                plugged.try_set(Some(read));
            }
        });
    };
    spawn_local(async move {
        if let Ok(settings) = daemon::app_settings().await {
            app.try_set(settings.background.is_some());
        }
        read();
    });
    let every = set_interval_with_handle(read, PLUGGED_EVERY).ok();
    on_cleanup(move || {
        if let Some(every) = every {
            every.clear();
        }
    });
    let pick = move |radio: PluggedRadio| {
        address.set(radio.address.clone());
        place.picked_name.set(Some(plugged_name(i18n, &radio)));
        by_hand.set(false);
        if !radio.ready {
            asked.set(Some(radio.address.clone()));
            spawn_local(async move {
                if daemon::ask_plugged(&radio.address).await == Ok(true) {
                    asked.try_set(None);
                }
            });
        }
    };
    let rows = move || {
        plugged
            .get()
            .unwrap_or_default()
            .into_iter()
            .map(|radio| {
                let this = radio.address.clone();
                let on = {
                    let this = this.clone();
                    move || address.with(|a| *a == this) && !by_hand.get()
                };
                let name = plugged_name(i18n, &radio);
                let detail = [radio.maker.clone(), radio.chip.clone(), radio.ids.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let waiting = {
                    let this = this.clone();
                    move || asked.with(|a| a.as_deref() == Some(this.as_str()))
                };
                view! {
                    <button
                        type="button"
                        role="radio"
                        class="plugged-radio"
                        class:on=on.clone()
                        aria-checked=move || on().to_string()
                        on:click=move |_| pick(radio.clone())
                    >
                        <span class="icon" inner_html=USB></span>
                        <span class="what">
                            <span class="title">{name}</span>
                            <span class="detail">{detail}</span>
                            <span class="detail mono">{this.clone()}</span>
                            <span class="waiting" class:gone=move || !waiting()>
                                {t!(i18n, plugged_asked)}
                            </span>
                        </span>
                    </button>
                }
            })
            .collect_view()
    };
    let none = move || plugged.with(|p| p.as_ref().is_some_and(Vec::is_empty));
    view! {
        <div class="field plugged">
            <div class="plugged-head">
                <span class="label">{t!(i18n, welcome_device)}</span>
                <button
                    type="button"
                    class="plugged-hand"
                    class:gone=move || app.get()
                    on:click=move |_| {
                        by_hand.update(|h| *h = !*h);
                        address.set(String::new());
                        place.picked_name.set(None);
                    }
                >
                    {move || if by_hand.get() {
                        t!(i18n, plugged_pick).into_any()
                    } else {
                        t!(i18n, plugged_by_hand).into_any()
                    }}
                </button>
            </div>
            <div class="plugged-list" role="radiogroup" class:gone=by_hand>
                {rows}
                <p class="plugged-none" class:gone=move || !none()>
                    {t!(i18n, plugged_none)}
                </p>
                <p class="plugged-none" class:gone=move || plugged.with(Option::is_some)>
                    {t!(i18n, plugged_looking)}
                </p>
            </div>
            <Show when=move || by_hand.get()>
                <input
                    class="mono"
                    required
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    placeholder="/dev/ttyACM0"
                    prop:value=move || address.get()
                    on:input=move |ev| address.set(event_target_value(&ev))
                />
                <span class="hint">{t!(i18n, welcome_device_hint)}</span>
            </Show>
        </div>
    }
}

/// What a radio heard nearby is called: its name, or its address.
fn nearby_name(radio: &NearbyRadio) -> String {
    radio
        .name
        .clone()
        .unwrap_or_else(|| radio.address.trim_start_matches("ble://").to_owned())
}

/// The radios heard nearby by Bluetooth, looked for as it is shown and
/// again as asked, to pick one from: its firmware picked with it, if it
/// says.
#[component]
fn NearbyPicker(place: Place) -> impl IntoView {
    let i18n = use_i18n();
    let address = place.address;
    let nearby = RwSignal::new(None::<Vec<NearbyRadio>>);
    let allowed = RwSignal::new(true);
    let looking = RwSignal::new(false);
    // Looked for again and again while Android asks for leave, so that the
    // radios show as soon as it is given.
    fn look(
        looking: RwSignal<bool>,
        allowed: RwSignal<bool>,
        nearby: RwSignal<Option<Vec<NearbyRadio>>>,
    ) {
        if looking.get_untracked() {
            return;
        }
        looking.set(true);
        spawn_local(async move {
            if let Ok(heard) = daemon::bluetooth().await {
                allowed.try_set(heard.allowed);
                nearby.try_set(Some(heard.radios));
                if !heard.allowed {
                    set_timeout(
                        move || {
                            if !looking.is_disposed() {
                                look(looking, allowed, nearby);
                            }
                        },
                        ASKED_AGAIN,
                    );
                }
            }
            looking.try_set(false);
        });
    }
    let look = move || look(looking, allowed, nearby);
    look();
    let pick = move |radio: NearbyRadio| {
        address.set(radio.address.clone());
        place.picked_name.set(Some(nearby_name(&radio)));
        if let Some(kind) = radio.kind {
            place.kind.set(kind);
        }
    };
    let rows = move || {
        let mut heard = nearby.get().unwrap_or_default();
        // The one it has already, out of reach or connected and so silent:
        // first, by its address.
        let now = address.get();
        if now.starts_with("ble://") && !heard.iter().any(|r| r.address == now) {
            heard.insert(
                0,
                NearbyRadio {
                    address: now,
                    name: None,
                    rssi: None,
                    kind: None,
                },
            );
        }
        heard
            .into_iter()
            .map(|radio| {
                let this = radio.address.clone();
                let on = {
                    let this = this.clone();
                    move || address.with(|a| *a == this)
                };
                let name = nearby_name(&radio);
                let detail = [
                    radio.kind.map(|kind| {
                        match kind {
                            RadioKind::Meshtastic => "Meshtastic",
                            RadioKind::Meshcore => "MeshCore",
                        }
                        .to_owned()
                    }),
                    radio.rssi.map(|rssi| format!("{rssi} dBm")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                view! {
                    <button
                        type="button"
                        role="radio"
                        class="plugged-radio"
                        class:on=on.clone()
                        aria-checked=move || on().to_string()
                        on:click=move |_| pick(radio.clone())
                    >
                        <span class="icon" inner_html=BLUETOOTH></span>
                        <span class="what">
                            <span class="title">{name}</span>
                            <span class="detail">{detail}</span>
                            <span class="detail mono">
                                {this.trim_start_matches("ble://").to_owned()}
                            </span>
                        </span>
                    </button>
                }
            })
            .collect_view()
    };
    let none = move || {
        !looking.get()
            && allowed.get()
            && nearby.with(|n| n.as_ref().is_some_and(Vec::is_empty))
            && !address.with(|a| a.starts_with("ble://"))
    };
    view! {
        <div class="field plugged">
            <div class="plugged-head">
                <span class="label">{t!(i18n, nearby_title)}</span>
                <button
                    type="button"
                    class="plugged-hand"
                    disabled=move || looking.get()
                    on:click=move |_| look()
                >
                    {t!(i18n, nearby_again)}
                </button>
            </div>
            <div class="plugged-list" role="radiogroup">
                {rows}
                <p class="plugged-none" class:gone=move || !looking.get()>
                    {t!(i18n, nearby_looking)}
                </p>
                <p class="plugged-none" class:gone=move || allowed.get()>
                    {t!(i18n, nearby_asked)}
                </p>
                <p class="plugged-none" class:gone=move || !none()>
                    {t!(i18n, nearby_none)}
                </p>
            </div>
        </div>
    }
}
