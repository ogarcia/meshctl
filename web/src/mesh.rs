// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the selected radio knows: itself, and the nodes it hears.

use crate::app::{Daemon, Live};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::{LinkState, Node, Peer};

#[component]
pub fn MeshView() -> impl IntoView {
    view! {
        <OfflineNote />
        <NodeList />
    }
}

/// For a radio that is not connected: why, and how old what it shows is.
#[component]
fn OfflineNote() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let radio = move || {
        let id = live.selected.get()?;
        live.radios
            .with(|radios| radios.as_ref()?.iter().find(|r| r.id == id).cloned())
    };
    move || {
        let radio = radio()?;
        if live.daemon.get() != Daemon::Connected {
            // Not the radio: meshctl itself is out of reach.
            return Some(
                view! { <p class="card note">{t!(i18n, daemon_down_note)}</p> }.into_any(),
            );
        }
        let why = match radio.link {
            LinkState::Ready | LinkState::Syncing => return None,
            LinkState::Disabled => t!(i18n, radio_disabled_note).into_any(),
            LinkState::Connecting | LinkState::Disconnected => {
                // While it first connects there is nothing to explain yet.
                radio.last_connected?;
                t!(i18n, radio_offline_note).into_any()
            }
        };
        let since = radio.last_connected.map(|at| {
            let at = u32::try_from(at).unwrap_or_default();
            view! {
                " "
                {t!(i18n, radio_last_connected, ago = move || ago(live.now.get(), at))}
            }
        });
        Some(view! { <p class="card note">{why} {since}</p> }.into_any())
    }
}

/// The radio itself.
/// Which nodes the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    All,
    /// Heard directly.
    Direct,
    /// Heard through other nodes.
    Mesh,
    Mqtt,
}

/// How far a node is, by the classes the map's legend has.
fn distance(node: &Node) -> &'static str {
    if node.via_mqtt {
        return "mqtt";
    }
    match node.hops_away {
        Some(0) => "direct",
        Some(1..=2) => "near",
        Some(3..=4) => "mid",
        _ => "far",
    }
}

#[component]
fn NodeList() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let filter = RwSignal::new(String::new());
    let reach = RwSignal::new(Reach::All);

    let shown = move || {
        let needle = filter.get().to_lowercase();
        let reach = reach.get();
        let mut nodes: Vec<Node> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter(|node| needle.is_empty() || matches(node, &needle))
                .filter(|node| match reach {
                    Reach::All => true,
                    Reach::Direct => !node.via_mqtt && node.hops_away == Some(0),
                    Reach::Mesh => !node.via_mqtt && node.hops_away.is_some_and(|h| h > 0),
                    Reach::Mqtt => node.via_mqtt,
                })
                .cloned()
                .collect()
        });
        // Favourites first, then the most recently heard.
        nodes.sort_by(|a, b| {
            b.favorite
                .cmp(&a.favorite)
                .then(b.last_heard.cmp(&a.last_heard))
                .then(a.num.cmp(&b.num))
        });
        nodes
    };
    let reaches = [Reach::All, Reach::Direct, Reach::Mesh, Reach::Mqtt]
        .into_iter()
        .map(|this| {
            let label = move || match this {
                Reach::All => t!(i18n, nodes_all).into_any(),
                Reach::Direct => t!(i18n, nodes_direct).into_any(),
                Reach::Mesh => t!(i18n, nodes_mesh).into_any(),
                Reach::Mqtt => "MQTT".into_any(),
            };
            (
                Signal::derive(move || reach.get() == this),
                Box::new(move || reach.set(this)) as Box<dyn Fn() + Send + Sync>,
                label.into_any(),
            )
        })
        .collect::<Vec<_>>();

    view! {
        <section class="node-list">
            <div class="node-tools">
                {crate::preferences::pills(reaches)}
                <input
                    type="search"
                    placeholder=move || t_string!(i18n, search)
                    prop:value=move || filter.get()
                    on:input=move |ev| filter.set(event_target_value(&ev))
                />
            </div>
            <table class="nodes-table">
                <thead>
                    <tr>
                        <th>{t!(i18n, nodes_name)}</th>
                        <th>{t!(i18n, nodes_heard)}</th>
                        <th>{t!(i18n, nodes_reach)}</th>
                        <th>{t!(i18n, nodes_battery)}</th>
                        <th>{t!(i18n, nodes_hardware)}</th>
                        <th></th>
                    </tr>
                </thead>
                <tbody>
                    <For each=shown key=|node| format!("{node:?}") let:node>
                        <NodeRow node now=live.now />
                    </For>
                </tbody>
            </table>
        </section>
    }
}

fn matches(node: &Node, needle: &str) -> bool {
    [
        node.long_name.as_deref(),
        node.short_name.as_deref(),
        Some(&node.id),
    ]
    .into_iter()
    .flatten()
    .any(|text| text.to_lowercase().contains(needle))
}

const MESSAGE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/message.svg");
const STAR: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/star.svg");
const PIN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/map-pin.svg");
const PLUG: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plug.svg");
const BATTERY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/battery-2.svg");

#[component]
fn NodeRow(node: Node, now: RwSignal<u32>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let num = node.num;
    let heard = node.last_heard;
    let name = node.long_name.clone().unwrap_or_else(|| node.id.clone());
    // MeshCore nodes have no short name: their name's first letters.
    let short = node
        .short_name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::badge::initials(&name));
    // The radio's own node, as on the map.
    let local = live
        .status
        .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num))
        == Some(num);
    let class = format!(
        "node-avatar {}",
        if local { "local" } else { distance(&node) }
    );
    // Heard through others, the SNR is the last relay's, not this node's.
    let reach = {
        let (hops, mqtt, snr) = (node.hops_away, node.via_mqtt, node.snr);
        move || {
            if local {
                return t_string!(i18n, map_local).to_owned();
            }
            if mqtt {
                return "MQTT".to_owned();
            }
            match hops {
                Some(0) => match snr {
                    Some(snr) => format!("{} · {snr:.1} dB", t_string!(i18n, map_direct)),
                    None => t_string!(i18n, map_direct).to_owned(),
                },
                Some(1) => t_string!(i18n, map_one_hop).to_owned(),
                Some(hops) => t_string!(i18n, map_hops).replace("{n}", &hops.to_string()),
                None => String::new(),
            }
        }
    };
    let power = node.battery_level.map(|level| {
        if level > 100 {
            view! {
                <span class="power external" title=move || t_string!(i18n, power_external)>
                    <span class="icon" inner_html=PLUG></span>
                    {t!(i18n, power_external)}
                </span>
            }
            .into_any()
        } else {
            view! {
                <span class="power" class:low=level < 20>
                    <span class="icon" inner_html=BATTERY></span>
                    {format!("{level} %")}
                </span>
            }
            .into_any()
        }
    });
    let role = node.role.clone();
    let favorite = node.favorite;
    let placed = node.position.is_some();

    view! {
        <tr class="node-row" on:click=move |_| live.open_node(num)>
            <td>
                <span class="who">
                    <span class=class>{crate::badge::fit(&short)}</span>
                    <span class="names">
                        <span class="name">
                            {name}
                            {favorite.then(|| view! { <span class="mark star" inner_html=STAR></span> })}
                            {placed.then(|| view! { <span class="mark" inner_html=PIN></span> })}
                        </span>
                        <span class="mono dim">{node.id.clone()}</span>
                    </span>
                </span>
            </td>
            <td class="dim">{move || heard.map(|t| ago(now.get(), t)).unwrap_or_default()}</td>
            <td>{reach}</td>
            <td>{power}</td>
            <td>
                <span class="names">
                    <span>{node.hw_model.as_deref().map(model)}</span>
                    <span class="dim small">
                        {move || role.as_deref().map(|r| crate::names::role(i18n.get_locale(), r))}
                    </span>
                </span>
            </td>
            <td class="actions">
                <button
                    type="button"
                    class="icon-button"
                    title=move || t_string!(i18n, node_message)
                    aria-label=move || t_string!(i18n, node_message)
                    on:click=move |ev| {
                        ev.stop_propagation();
                        live.open_chat(Peer::Node { node: num });
                    }
                    inner_html=MESSAGE
                ></button>
            </td>
        </tr>
    }
}

/// A hardware model as people write it, from the protobufs' name for it:
/// `HELTEC_WIRELESS_TRACKER` is Heltec Wireless Tracker. Words with a digit,
/// and short ones, are left as they are: `RAK4631`, `NRF52`, `M1`, `DIY`.
pub fn model(raw: &str) -> String {
    raw.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            if word.len() <= 3 || word.chars().any(|c| c.is_ascii_digit()) {
                word.to_owned()
            } else {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first
                        .to_uppercase()
                        .chain(chars.flat_map(char::to_lowercase))
                        .collect()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a node runs on: its battery's level, and its voltage when given;
/// `external` (in the interface's words) when it is plugged in, which the
/// firmware says with a level over 100.
pub fn power(level: Option<u32>, voltage: Option<f32>, external: &str) -> Option<String> {
    let level = level.map(|level| {
        if level > 100 {
            external.to_owned()
        } else {
            format!("{level} %")
        }
    });
    match (level, voltage) {
        (Some(level), Some(voltage)) => Some(format!("{level} · {voltage:.2} V")),
        (Some(level), None) => Some(level),
        (None, Some(voltage)) => Some(format!("{voltage:.2} V")),
        (None, None) => None,
    }
}

/// How far, in metres, a position sent with so many bits of precision may
/// be from where the node is: half the side of the cell it rounds to, as the
/// apps tell it. None when exact (32) or not sent (0).
pub fn precision_radius(bits: u32) -> Option<u32> {
    if !(1..32).contains(&bits) {
        return None;
    }
    // A degree of latitude is about 111 320 m; coordinates go by 1e-7 degrees.
    let side = f64::from(1_u32 << (32 - bits)) * 1e-7 * 111_320.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((side / 2.0).round() as u32)
}

/// How far off a rounded position may be: in metres up to a kilometre, in
/// kilometres beyond.
pub fn radius(metres: u32) -> String {
    if metres < 1000 {
        format!("± {metres} m")
    } else {
        format!("± {:.1} km", f64::from(metres) / 1000.0)
    }
}

/// How long ago, in the largest unit that fits. The units are the same in
/// every language the interface speaks.
pub fn ago(now: u32, then: u32) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3_600 => format!("{} min", seconds / 60),
        3_600..86_400 => format!("{} h", seconds / 3_600),
        _ => format!("{} d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::model;

    #[test]
    fn models_read_as_people_write_them() {
        assert_eq!(model("HELTEC_WIRELESS_TRACKER"), "Heltec Wireless Tracker");
        assert_eq!(model("RAK4631"), "RAK4631");
        assert_eq!(model("NRF52_PROMICRO_DIY"), "NRF52 Promicro DIY");
        assert_eq!(model("THINKNODE_M1"), "Thinknode M1");
        assert_eq!(model("T_ECHO"), "T Echo");
    }

    #[test]
    fn a_radius_reads_in_the_unit_that_fits() {
        assert_eq!(super::radius(365), "± 365 m");
        assert_eq!(super::radius(2918), "± 2.9 km");
        assert_eq!(super::precision_radius(13), Some(2918));
        assert_eq!(super::precision_radius(32), None);
    }
}
