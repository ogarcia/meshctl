// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the selected radio hears, packet by packet, as it hears it: the
//! newest on top, each one new glowing once as it comes, older ones read as
//! the list is scrolled down. Filters say which show; it can be paused, and
//! it holds still while it is read away from its top, what comes waiting
//! above until it goes back.

use crate::app::Live;
use crate::chat::{node_name, short_name, title};
use crate::daemon;
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::preferences::{Choice, pills};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{BROADCAST, Packet, PacketFilter, PacketKind, PacketReach, Peer};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

const PAUSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/player-pause.svg");
const PLAY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/player-play.svg");
const UP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/arrow-up.svg");

/// The most packets kept in the page: older ones go as new ones come.
pub const KEPT: usize = 1000;

/// How far from the top the list is being read, and held still.
const AWAY: i32 = 24;

/// How near its end older packets are read.
const NEAR_END: i32 = 600;

/// The page's scroller, which the list scrolls in.
fn scroller() -> Option<web_sys::Element> {
    document().query_selector("main").ok().flatten()
}

#[component]
pub fn PacketsView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();

    // Opened at its top: what came while it was not shown goes on, and none
    // of what there is now is new.
    live.packets_away.set(false);
    if !live.packets_paused.get_untracked() {
        live.release_packets();
    }
    live.packets_mark.set(
        live.packets
            .with_untracked(|p| p.first().map(|p| p.id))
            .unwrap_or_else(|| live.packets_mark.get_untracked()),
    );

    // Older ones, a page at a time, while there are.
    let busy = RwSignal::new(false);
    let older = move || {
        if busy.get_untracked() || live.packets_end.get_untracked() {
            return;
        }
        let (Some(radio), Some(last)) = (
            live.selected.get_untracked(),
            live.packets.with_untracked(|p| p.last().map(|p| p.id)),
        ) else {
            return;
        };
        let filter = live.packets_filter();
        busy.set(true);
        spawn_local(async move {
            if let Ok(page) = daemon::packets(radio, Some(last), &filter).await
                && live.selected.get_untracked() == Some(radio)
                && live.packets_filter() == filter
            {
                if page.len() < daemon::PACKETS_PAGE {
                    live.packets_end.set(true);
                }
                live.packets.update(|packets| packets.extend(page));
            }
            busy.set(false);
        });
    };

    // Back at the top, what came goes on, unless paused.
    let to_top = move || {
        if let Some(main) = scroller() {
            main.set_scroll_top(0);
        }
        live.packets_away.set(false);
        live.release_packets();
    };

    // As it is scrolled: away from the top or back, and near the end. The
    // listener goes with the view.
    let on_scroll = move || {
        let Some(main) = scroller() else { return };
        let top = main.scroll_top();
        let away = top > AWAY;
        if away != live.packets_away.get_untracked() {
            live.packets_away.set(away);
            if !away && !live.packets_paused.get_untracked() {
                live.release_packets();
            }
        }
        if main.scroll_height() - top - main.client_height() < NEAR_END {
            older();
        }
    };
    let listener = StoredValue::new_local(None::<(web_sys::Element, Closure<dyn FnMut()>)>);
    if let Some(main) = scroller() {
        let call = Closure::<dyn FnMut()>::new(on_scroll);
        if main
            .add_event_listener_with_callback("scroll", call.as_ref().unchecked_ref())
            .is_ok()
        {
            listener.set_value(Some((main, call)));
        }
    }
    on_cleanup(move || {
        listener.update_value(|l| {
            if let Some((main, call)) = l.take() {
                let _ = main
                    .remove_event_listener_with_callback("scroll", call.as_ref().unchecked_ref());
            }
        });
    });
    // Short of filling the page, older ones too, until it does or there are
    // no more.
    Effect::new(move |_| {
        live.packets.with(Vec::len);
        live.packets_end.track();
        request_animation_frame(on_scroll);
    });

    // Filtered anew: read again from the top. Only as the filters change, not
    // as they are set to what they were, as a field left does.
    Effect::new(move |last: Option<(PacketFilter, bool)>| {
        let now = (
            live.packet_filter.get(),
            !live.preferences.with(|p| p.packets_with_own),
        );
        if last.is_some_and(|last| last != now) {
            if let Some(main) = scroller() {
                main.set_scroll_top(0);
            }
            live.packets_away.set(false);
            spawn_local(async move {
                let _ = live.reload_packets().await;
            });
        }
        now
    });

    // Filtered by hand: the radio's own left out, as by default, is not.
    let filtered = move || live.packet_filter.with(|f| *f != PacketFilter::default());
    let held = move || live.packets_held.with(Vec::len);
    provide_context(Shown {
        open: RwSignal::new(false),
        packet: RwSignal::new(None),
    });

    // A radio out of reach that heard nothing yet: why, in the middle, and
    // not "nothing heard yet" under the filters, as if it were listening.
    let bare = Memo::new(move |_| {
        let offline = crate::offline::offline(live)?;
        (live.packets.with(Vec::is_empty) && !filtered()).then_some(offline)
    });

    view! {
        <Show when=move || bare.get().is_some()>
            {move || {
                bare.get().map(|(why, radio)| view! { <crate::offline::OfflineEmpty why radio /> })
            }}
        </Show>
        <div class="live" class:gone=move || bare.get().is_some()>
            <crate::offline::OfflineBanner />
            <PacketTools />
            <section class="packets">
                <Show when=move || held() != 0>
                    <button class="new-packets" on:click=move |_| to_top()>
                        <span class="icon" inner_html=UP></span>
                        {move || t!(i18n, packets_new, count = held())}
                    </button>
                </Show>
                <Show
                    when=move || live.packets.with(|p| !p.is_empty())
                    fallback=move || {
                        view! {
                            <p class="dim nothing">
                                {move || {
                                    if filtered() {
                                        t!(i18n, packets_none).into_any()
                                    } else {
                                        t!(i18n, packets_empty).into_any()
                                    }
                                }}
                            </p>
                        }
                    }
                >
                    <ol class="packet-list">
                        <For each=move || live.packets.get() key=|p| p.id let:packet>
                            {
                                let fresh = packet.id > live.packets_mark.get_untracked();
                                view! { <PacketRow packet fresh /> }
                            }
                        </For>
                    </ol>
                    <Show when=move || live.packets_end.get()>
                        <p class="dim end">{t!(i18n, packets_end)}</p>
                    </Show>
                </Show>
            </section>
        </div>
        // Out of the view, which holds its size: a drawer in it would be held
        // in it too.
        <PacketDrawer />
    }
}

/// The filters over the list, and its pause.
#[component]
fn PacketTools() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let filter = live.packet_filter;

    // A kind by its name, and its name in the query of the list.
    let kind_label = move |kind: PacketKind| match kind {
        PacketKind::Text => t_string!(i18n, packets_kind_text),
        PacketKind::Position => t_string!(i18n, packets_kind_position),
        PacketKind::NodeInfo => t_string!(i18n, packets_kind_node_info),
        PacketKind::Routing => t_string!(i18n, packets_kind_routing),
        PacketKind::Telemetry => t_string!(i18n, packets_kind_telemetry),
        PacketKind::Traceroute => t_string!(i18n, packets_kind_traceroute),
        PacketKind::Neighbors => t_string!(i18n, packets_kind_neighbors),
        PacketKind::Encrypted => t_string!(i18n, packets_kind_encrypted),
        PacketKind::Other => t_string!(i18n, packets_kind_other),
    };
    let kind_value = |kind: PacketKind| {
        serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let pick_kind = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        let kind = PacketKind::ALL
            .into_iter()
            .find(|k| kind_value(*k) == value);
        filter.update(|f| f.kind = kind);
    };

    // The channels the radio has in use, by their slots.
    let channels = move || {
        live.channels.with(|channels| {
            channels
                .iter()
                .filter(|c| c.role != "DISABLED")
                .map(|c| c.index)
                .collect::<Vec<_>>()
        })
    };
    let pick_channel = move |ev: leptos::ev::Event| {
        let channel = event_target_value(&ev).parse().ok();
        filter.update(|f| f.channel = channel);
    };

    // How they came, as the nodes list says it.
    let reaches: Vec<Choice> = [
        None,
        Some(PacketReach::Direct),
        Some(PacketReach::Mesh),
        Some(PacketReach::Mqtt),
    ]
    .into_iter()
    .map(|reach| {
        let label = move || match reach {
            None => t_string!(i18n, nodes_all).to_owned(),
            Some(PacketReach::Direct) => t_string!(i18n, nodes_direct).to_owned(),
            Some(PacketReach::Mesh) => t_string!(i18n, nodes_mesh).to_owned(),
            Some(PacketReach::Mqtt) => "MQTT".to_owned(),
        };
        (
            Signal::derive(move || filter.with(|f| f.reach) == reach),
            Box::new(move || filter.update(|f| f.reach = reach)) as Box<dyn Fn() + Send + Sync>,
            Signal::derive(label),
        )
    })
    .collect();

    // A node by its name as the list offers it, or by its id written out.
    // The radio's own in the list or not, kept in the preferences.
    let with_own = move || {
        let mut preferences = live.preferences.get_untracked();
        preferences.packets_with_own = !preferences.packets_with_own;
        live.prefer(preferences);
    };
    let node_label = move |num: u32| format!("{} (!{num:08x})", node_name(live, num));
    let pick_node = move |ev: leptos::ev::Event| {
        let text = event_target_value(&ev);
        let text = text.trim();
        let node = if text.is_empty() {
            None
        } else if let Some(hex) = text.strip_prefix('!')
            && let Ok(num) = u32::from_str_radix(hex, 16)
        {
            Some(num)
        } else {
            live.nodes
                .with_untracked(|nodes| nodes.keys().copied().find(|&num| node_label(num) == text))
        };
        // Only once it names one, or is cleared: not as it is written.
        if node.is_some() || text.is_empty() {
            filter.update(|f| f.node = node);
        }
    };

    view! {
        <div class="packet-tools">
            <select
                class="kind"
                aria-label=move || t_string!(i18n, packets_kind)
                prop:value=move || filter.with(|f| f.kind.map(kind_value).unwrap_or_default())
                on:change=pick_kind
            >
                <option value="">{t!(i18n, packets_kind_all)}</option>
                {PacketKind::ALL
                    .into_iter()
                    .map(|kind| {
                        view! { <option value=kind_value(kind)>{move || kind_label(kind)}</option> }
                    })
                    .collect_view()}
            </select>
            <select
                class="channel"
                aria-label=move || t_string!(i18n, packets_channel)
                prop:value=move || filter.with(|f| f.channel.map(|c| c.to_string()).unwrap_or_default())
                on:change=pick_channel
            >
                <option value="">{t!(i18n, packets_channel_all)}</option>
                <For each=channels key=|slot| *slot let:slot>
                    <option value=slot.to_string()>
                        {move || title(live, Peer::Channel { channel: slot })}
                    </option>
                </For>
            </select>
            {pills(reaches)}
            // The node to follow, as wide as the switch beside it leaves.
            <div class="search">
                <input
                    type="search"
                    class="node-filter"
                    list="packet-nodes"
                    placeholder=move || t_string!(i18n, packets_node)
                    prop:value=move || filter.with(|f| f.node).map(node_label).unwrap_or_default()
                    on:change=pick_node
                    on:search=pick_node
                />
                <datalist id="packet-nodes">
                    <For each=move || live.nodes.with(|n| n.keys().copied().collect::<Vec<_>>()) key=|num| *num let:num>
                        <option value=move || node_label(num)></option>
                    </For>
                </datalist>
                // The radio's own, left out unless asked for: a switch, off as it
                // starts, and kept in the preferences.
                <div class="own">
                    <crate::radios::Switch
                        on=Signal::derive(move || live.preferences.with(|p| p.packets_with_own))
                        label=Signal::derive(move || t_string!(i18n, packets_with_own).to_owned())
                        flip=move |_| with_own()
                    />
                    <span class="label" on:click=move |_| with_own()>
                        {t!(i18n, packets_with_own)}
                    </span>
                </div>
            </div>
        </div>
    }
}

/// The live view's pause, in its heading: going on, what came while it was
/// paused goes on top, unless it is being read further down.
#[component]
pub fn PauseButton() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let paused = live.packets_paused;
    let pause = move |_| {
        let now = !paused.get_untracked();
        paused.set(now);
        if !now && !live.packets_away.get_untracked() {
            live.release_packets();
        }
    };
    let label = move || {
        if paused.get() {
            t_string!(i18n, packets_resume)
        } else {
            t_string!(i18n, packets_pause)
        }
    };
    view! {
        <button
            type="button"
            class="with-icon pause"
            class:on=move || paused.get()
            aria-pressed=move || paused.get().to_string()
            title=label
            on:click=pause
        >
            <span class="icon" inner_html=move || if paused.get() { PLAY } else { PAUSE }></span>
            <span class="label">{label}</span>
        </button>
    }
}

/// The packet open in its drawer, whole, as a row opens it.
#[derive(Clone, Copy)]
struct Shown {
    open: RwSignal<bool>,
    packet: RwSignal<Option<Packet>>,
}

/// How a packet came: sent by the radio itself, or heard, by air or by
/// MQTT.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Tx,
    Lora,
    Mqtt,
}

impl Source {
    fn of(live: Live, packet: &Packet) -> Self {
        let own = live
            .status
            .with_untracked(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
        if own == Some(packet.from) {
            Self::Tx
        } else if packet.via_mqtt {
            Self::Mqtt
        } else {
            Self::Lora
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Tx => "source tx",
            Self::Lora => "source lora",
            Self::Mqtt => "source mqtt",
        }
    }
}

/// A kind of packet's class, for its colour.
fn kind_class(kind: PacketKind) -> &'static str {
    match kind {
        PacketKind::Text => "kind-tag text",
        PacketKind::Position => "kind-tag position",
        PacketKind::NodeInfo => "kind-tag node-info",
        PacketKind::Routing => "kind-tag routing",
        PacketKind::Telemetry => "kind-tag telemetry",
        PacketKind::Traceroute => "kind-tag traceroute",
        PacketKind::Neighbors => "kind-tag neighbors",
        PacketKind::Encrypted => "kind-tag encrypted",
        PacketKind::Other => "kind-tag other",
    }
}

/// A node by its long name, or its id when it has none: the short one
/// besides would take the room the route has.
fn who(live: Live, num: u32) -> String {
    node_name(live, num)
}

/// The node that relayed a packet last, by the last byte of its number, as
/// the firmware tells: by its short name when only one known node ends so,
/// or only one heard directly does; by the byte otherwise.
pub fn relay_name(live: Live, relay: u32) -> String {
    live.nodes.with(|nodes| {
        let ending: Vec<_> = nodes.values().filter(|n| n.num & 0xff == relay).collect();
        let direct: Vec<_> = ending.iter().filter(|n| n.hops_away == Some(0)).collect();
        let one = match (ending.as_slice(), direct.as_slice()) {
            ([only], _) => Some(only.num),
            (_, [only]) => Some(only.num),
            _ => None,
        };
        one.map_or_else(
            || format!("··{relay:02x}"),
            |num| {
                let short = short_name(live, num);
                if short.is_empty() {
                    format!("!{num:08x}")
                } else {
                    short
                }
            },
        )
    })
}

#[component]
fn PacketRow(packet: Packet, fresh: bool) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let shown = expect_context::<Shown>();
    let from = packet.from;
    let to = packet.to;
    let channel = packet.channel;
    let kind = PacketKind::of(packet.port.as_deref());
    let source = Source::of(live, &packet);
    let port = packet.port.clone().map(|p| short_port(&p));
    let summary = {
        let packet = packet.clone();
        move || summary(&packet, i18n.get_locale())
    };
    let (hops_away, hop_start, relay) = (packet.hops_away, packet.hop_start, packet.relay_node);
    // How strong: its SNR, and its RSSI apart, which a list too narrow for
    // it in every row leaves out.
    let snr = packet.snr.map(|snr| format!("{snr:.1} dB"));
    let rssi = packet.rssi.map(|rssi| {
        if snr.is_some() {
            format!(" · {rssi} dBm")
        } else {
            format!("{rssi} dBm")
        }
    });
    // Heard by air: in how many hops, and by whom last when it was not
    // straight from its sender.
    let hops = move || match (source, hops_away) {
        (Source::Lora, Some(0)) => t!(i18n, packets_direct).into_any(),
        (Source::Lora, Some(hops)) => {
            let count = if hops == 1 {
                t!(i18n, packets_hop).into_any()
            } else {
                t!(i18n, packets_hops, count = hops).into_any()
            };
            // The relay's byte, when the firmware gives it; a question mark
            // when it does not, as the node that relayed it did not say.
            let relay = format!(
                " · {}",
                relay.map_or_else(|| "?".to_owned(), |relay| relay_name(live, relay))
            );
            view! { {count} {relay} }.into_any()
        }
        _ => ().into_any(),
    };
    // Of how many it could take, as the firmware counts.
    let hops_title = move || match hops_away {
        Some(hops) if source == Source::Lora => {
            let mut title = format!("{hops}/{hop_start}");
            if hops > 0 && relay.is_none() {
                title.push_str(" · ");
                title.push_str(t_string!(i18n, packets_relay_unknown));
            }
            title
        }
        _ => String::new(),
    };
    let source_label = move || match source {
        Source::Tx => t_string!(i18n, packets_tx),
        Source::Lora => t_string!(i18n, packets_rx_lora),
        Source::Mqtt => t_string!(i18n, packets_rx_mqtt),
    };
    let source_title = move || match source {
        Source::Tx => t_string!(i18n, packets_tx_title),
        Source::Lora => t_string!(i18n, packets_rx_lora_title),
        Source::Mqtt => t_string!(i18n, packets_rx_mqtt_title),
    };
    let open = {
        let packet = packet.clone();
        move |_| {
            shown.packet.set(Some(packet.clone()));
            shown.open.set(true);
        }
    };

    view! {
        <li class="packet" class:arrived=fresh>
            <button class="packet-line" on:click=open>
                <span class="time mono">{clock(packet.at)}</span>
                <span class="kind">
                    <span class=kind_class(kind)>
                        {match port {
                            Some(port) => port.into_any(),
                            None => t!(i18n, packets_encrypted).into_any(),
                        }}
                    </span>
                </span>
                <span class="source-cell">
                    <span class=source.class() title=source_title>{source_label}</span>
                </span>
                <span class="route">
                    <span class="from">{move || who(live, from)}</span>
                    <span class="arrow">"→"</span>
                    <span class="to">
                        {move || {
                            if to == BROADCAST {
                                title(live, Peer::Channel { channel })
                            } else {
                                who(live, to)
                            }
                        }}
                    </span>
                </span>
                <span class="summary" class:dim=kind == PacketKind::Encrypted>{summary}</span>
                <span class="hops dim" title=hops_title>{hops}</span>
                <span class="signal mono dim">
                    {snr}
                    <span class="rssi">{rssi}</span>
                </span>
            </button>
        </li>
    }
}

/// The packet open, whole: what is known of it, and itself as text, to
/// copy.
#[component]
fn PacketDrawer() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let shown = expect_context::<Shown>();
    let title = Signal::derive(move || {
        shown.packet.with(|p| {
            p.as_ref().map_or_else(String::new, |p| {
                p.port
                    .as_deref()
                    .map_or_else(|| t_string!(i18n, packets_encrypted).to_owned(), short_port)
            })
        })
    });
    let text = move || {
        shown
            .packet
            .with(|p| {
                p.as_ref()
                    .and_then(|p| serde_json::to_string_pretty(p).ok())
            })
            .unwrap_or_default()
    };
    let foot = ViewFn::from(move || {
        view! {
            <button class="with-icon" on:click=move |_| crate::clipboard::copy(&text())>
                {t!(i18n, packets_copy)}
            </button>
        }
    });
    view! {
        <crate::node::Drawer open=shown.open title foot>
            {move || {
                shown
                    .packet
                    .get()
                    .map(|packet| {
                        let source = Source::of(live, &packet);
                        let (from, to, channel) = (packet.from, packet.to, packet.channel);
                        let fact = |label: String, value: AnyView| {
                            view! {
                                <div class="fact">
                                    <dt>{label}</dt>
                                    <dd>{value}</dd>
                                </div>
                            }
                        };
                        let text_fact = |label: String, value: String| fact(label, value.into_any());
                        let mut facts = vec![
                            text_fact(t_string!(i18n, packets_at).to_owned(), when(packet.at)),
                            text_fact(
                                t_string!(i18n, packets_from).to_owned(),
                                format!("{} (!{from:08x})", who(live, from)),
                            ),
                            text_fact(
                                t_string!(i18n, packets_to).to_owned(),
                                if to == BROADCAST {
                                    t_string!(i18n, packets_everyone).to_owned()
                                } else {
                                    format!("{} (!{to:08x})", who(live, to))
                                },
                            ),
                            text_fact(
                                t_string!(i18n, packets_channel).to_owned(),
                                format!("{channel} · {}", title_of(live, channel)),
                            ),
                            text_fact(
                                t_string!(i18n, packets_source).to_owned(),
                                match source {
                                    Source::Tx => t_string!(i18n, packets_tx_title),
                                    Source::Lora => t_string!(i18n, packets_rx_lora_title),
                                    Source::Mqtt => t_string!(i18n, packets_rx_mqtt_title),
                                }
                                .to_owned(),
                            ),
                            text_fact(
                                t_string!(i18n, packets_id).to_owned(),
                                format!("{} (0x{:08x})", packet.packet_id, packet.packet_id),
                            ),
                        ];
                        if let Some(hops) = packet.hops_away {
                            let start = packet.hop_start;
                            facts.push(fact(
                                t_string!(i18n, packets_hops_label).to_owned(),
                                t!(i18n, packets_hops_of, hops = hops, start = start).into_any(),
                            ));
                        }
                        if packet.hops_away.is_some_and(|h| h > 0) {
                            facts.push(text_fact(
                                t_string!(i18n, packets_relay).to_owned(),
                                packet.relay_node.map_or_else(
                                    || t_string!(i18n, packets_relay_unknown).to_owned(),
                                    |relay| format!("{} (··{relay:02x})", relay_name(live, relay)),
                                ),
                            ));
                        }
                        if let Some(snr) = packet.snr {
                            facts.push(text_fact("SNR".to_owned(), format!("{snr:.2} dB")));
                        }
                        if let Some(rssi) = packet.rssi {
                            facts.push(text_fact("RSSI".to_owned(), format!("{rssi} dBm")));
                        }
                        let mut marks: Vec<AnyView> = Vec::new();
                        if packet.pki {
                            marks.push(t!(i18n, packets_pki).into_any());
                        }
                        if packet.want_response {
                            marks.push(t!(i18n, packets_want_response).into_any());
                        }
                        if let Some(id) = packet.request_id {
                            marks.push(t!(i18n, packets_answer_to, id = id).into_any());
                        }
                        if let Some(id) = packet.reply_id {
                            marks.push(t!(i18n, packets_reply_to, id = id).into_any());
                        }
                        if !marks.is_empty() {
                            let marks = marks
                                .into_iter()
                                .map(|mark| view! { <span class="mark-item">{mark}</span> })
                                .collect_view();
                            facts.push(fact(t_string!(i18n, packets_marks).to_owned(), marks.into_any()));
                        }
                        view! {
                            <div class="packet-detail">
                                <dl class="packet-facts">{facts}</dl>
                                <pre class="packet-text mono">{text()}</pre>
                            </div>
                        }
                    })
            }}
        </crate::node::Drawer>
    }
}

/// A channel's name by its slot.
fn title_of(live: Live, channel: u32) -> String {
    title(live, Peer::Channel { channel })
}

/// The day and the time, in the browser's own zone.
fn when(at: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let date = js_sys::Date::new_0();
    date.set_time(at as f64 * 1000.0);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

/// A port without its `_APP`, as people say it.
fn short_port(port: &str) -> String {
    port.strip_suffix("_APP").unwrap_or(port).replace('_', " ")
}

/// The time of day, in the browser's own zone.
fn clock(at: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let date = js_sys::Date::new_0();
    date.set_time(at as f64 * 1000.0);
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

/// What a packet says, in a line.
pub fn summary(packet: &Packet, locale: Locale) -> String {
    let Some(payload) = &packet.payload else {
        return String::new();
    };
    let port = packet.port.as_deref().unwrap_or_default();
    let line = match port {
        _ if payload.is_string() => payload.as_str().map(str::to_owned),
        "POSITION_APP" => position(payload),
        "NODEINFO_APP" => {
            let name = payload["long_name"].as_str().unwrap_or_default();
            let model = payload["hw_model"].as_str().unwrap_or_default();
            Some(format!("{name} · {model}"))
        }
        "TELEMETRY_APP" => telemetry(payload, locale),
        "ROUTING_APP" => payload["error_reason"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| Some("NONE".into())),
        "TRACEROUTE_APP" => {
            let relays = payload["route"].as_array().map_or(0, Vec::len);
            Some(format!("{relays} ⇢"))
        }
        "NEIGHBORINFO_APP" => {
            let neighbors = payload["neighbors"].as_array().map_or(0, Vec::len);
            Some(format!("{neighbors} ↔"))
        }
        _ => None,
    };
    line.unwrap_or_else(|| {
        let mut json = payload.to_string();
        if json.len() > 120 {
            let cut = (0..=117)
                .rev()
                .find(|&i| json.is_char_boundary(i))
                .unwrap_or(0);
            json.truncate(cut);
            json.push('…');
        }
        json
    })
}

fn position(payload: &Value) -> Option<String> {
    let degrees = |key: &str| payload[key].as_f64().map(|v| v * 1e-7);
    let (latitude, longitude) = (degrees("latitude_i")?, degrees("longitude_i")?);
    let altitude = payload["altitude"]
        .as_i64()
        .map_or_else(String::new, |a| format!(" · {a} m"));
    Some(format!("{latitude:.5}, {longitude:.5}{altitude}"))
}

/// The measures a telemetry line reads, by group, with their units: the
/// rest of them, in its drawer.
const MEASURES: &[(&str, &str, &str)] = &[
    ("device_metrics", "battery_level", "%"),
    ("device_metrics", "voltage", "V"),
    ("device_metrics", "channel_utilization", "% ch"),
    ("device_metrics", "air_util_tx", "% tx"),
    ("environment_metrics", "temperature", "°C"),
    ("environment_metrics", "relative_humidity", "% RH"),
    ("environment_metrics", "barometric_pressure", "hPa"),
    ("environment_metrics", "gas_resistance", "MΩ"),
    ("environment_metrics", "iaq", "IAQ"),
    ("environment_metrics", "lux", "lx"),
    ("environment_metrics", "voltage", "V"),
    ("environment_metrics", "current", "mA"),
    ("environment_metrics", "wind_speed", "m/s"),
    ("environment_metrics", "wind_direction", "°"),
    ("environment_metrics", "rainfall_1h", "mm/h"),
    ("environment_metrics", "soil_moisture", "% soil"),
    ("environment_metrics", "weight", "kg"),
    ("environment_metrics", "distance", "mm"),
    ("environment_metrics", "radiation", "µR/h"),
    ("power_metrics", "ch1_voltage", "V"),
    ("power_metrics", "ch1_current", "mA"),
    ("power_metrics", "ch2_voltage", "V"),
    ("power_metrics", "ch2_current", "mA"),
    ("power_metrics", "ch3_voltage", "V"),
    ("power_metrics", "ch3_current", "mA"),
    ("air_quality_metrics", "pm25_standard", "µg/m³ PM2.5"),
    ("air_quality_metrics", "pm10_standard", "µg/m³ PM10"),
    ("air_quality_metrics", "co2", "ppm CO₂"),
    ("health_metrics", "heart_bpm", "bpm"),
    ("health_metrics", "spO2", "% SpO₂"),
    ("health_metrics", "temperature", "°C"),
    ("local_stats", "noise_floor", "dBm"),
    ("host_metrics", "load1", "load"),
];

fn telemetry(payload: &Value, locale: Locale) -> Option<String> {
    let number = |value: f64, unit: &str| {
        if value.fract() == 0.0 {
            format!("{value:.0} {unit}")
        } else {
            format!("{value:.1} {unit}")
        }
    };
    let mut parts: Vec<String> = Vec::new();
    // The radio's own stats: how many of the nodes it knows are online.
    let stats = &payload["local_stats"];
    if let (Some(online), Some(total)) = (
        stats["num_online_nodes"].as_u64(),
        stats["num_total_nodes"].as_u64(),
    ) {
        let nodes = match locale {
            Locale::es => "nodos",
            Locale::en => "nodes",
        };
        parts.push(format!("{online}/{total} {nodes}"));
    }
    parts.extend(
        MEASURES
            .iter()
            .filter_map(|(group, key, unit)| Some(number(payload[group][key].as_f64()?, unit))),
    );
    // None of those: whatever it has, by its names, rather than nothing.
    if parts.is_empty() {
        parts = payload
            .as_object()?
            .values()
            .filter_map(Value::as_object)
            .flat_map(|group| group.iter())
            .filter_map(|(key, value)| {
                Some(format!(
                    "{} {}",
                    key.replace('_', " "),
                    number(value.as_f64()?, "").trim_end()
                ))
            })
            .take(4)
            .collect();
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(port: &str, payload: Value) -> Packet {
        Packet {
            id: 1,
            packet_id: 1,
            from: 1,
            to: BROADCAST,
            channel: 0,
            port: Some(port.into()),
            payload: Some(payload),
            request_id: None,
            reply_id: None,
            want_response: false,
            pki: false,
            at: 0,
            snr: None,
            rssi: None,
            hops_away: None,
            hop_limit: 0,
            hop_start: 0,
            relay_node: None,
            via_mqtt: false,
        }
    }

    #[test]
    fn packets_read_in_a_line() {
        assert_eq!(
            summary(&packet("TEXT_MESSAGE_APP", "hola".into()), Locale::es),
            "hola"
        );
        let position = serde_json::json!({"latitude_i": 433_000_000, "longitude_i": -84_000_000});
        assert_eq!(
            summary(&packet("POSITION_APP", position), Locale::es),
            "43.30000, -8.40000"
        );
        let telemetry =
            serde_json::json!({"device_metrics": {"battery_level": 101, "voltage": 4.2}});
        assert_eq!(
            summary(&packet("TELEMETRY_APP", telemetry), Locale::es),
            "101 % · 4.2 V"
        );
        // A current sensor, as a node's environment.
        let current =
            serde_json::json!({"environment_metrics": {"current": 22.4, "voltage": 5.168}});
        assert_eq!(
            summary(&packet("TELEMETRY_APP", current), Locale::es),
            "5.2 V · 22.4 mA"
        );
        let stats = serde_json::json!({"local_stats": {"num_online_nodes": 37, "num_total_nodes": 80, "noise_floor": -105}});
        assert_eq!(
            summary(&packet("TELEMETRY_APP", stats), Locale::en),
            "37/80 nodes · -105 dBm"
        );
        // Measures it does not know, by their names, not as JSON.
        let unknown = serde_json::json!({"traffic_management_stats": {"packets_seen": 12}});
        assert_eq!(
            summary(&packet("TELEMETRY_APP", unknown), Locale::es),
            "packets seen 12"
        );
        assert_eq!(short_port("TEXT_MESSAGE_APP"), "TEXT MESSAGE");
    }
}
