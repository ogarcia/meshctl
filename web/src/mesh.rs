// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! What the selected radio knows: itself, and the nodes it hears.

use crate::app::Live;
use crate::i18n::{Locale, t, t_string, use_i18n};
use leptos::prelude::*;
use leptos_i18n::I18nContext;
use meshctl_api::{Node, NodesColumn as Column, NodesSort, Peer, RadioKind};

/// Whether the selected radio is a Meshtastic one, whose nodes say how they
/// are powered and may come through MQTT: MeshCore contacts say nothing of
/// their battery, and MeshCore has no MQTT.
fn is_meshtastic(live: Live) -> bool {
    live.selected_kind() != Some(RadioKind::Meshcore)
}

#[component]
pub fn MeshView() -> impl IntoView {
    let live = expect_context::<Live>();
    // A radio out of reach with no nodes known: why, in the middle, and not
    // an empty list under a strip saying it.
    let bare = Memo::new(move |_| {
        let offline = crate::offline::offline(live)?;
        let local = live
            .status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
        let any = live
            .nodes
            .with(|nodes| nodes.keys().any(|num| Some(*num) != local));
        (!any).then_some(offline)
    });
    move || match bare.get() {
        Some((why, radio)) => view! { <crate::offline::OfflineEmpty why radio /> }.into_any(),
        None => view! {
            <crate::offline::OfflineBanner />
            <NodeList />
        }
        .into_any(),
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
    /// MeshCore's not among the radio's contacts: heard advertising and not
    /// added, or had forget; to be added from their pages.
    Unkept,
}

/// How a node sorts by a column, in its most useful order: names and
/// hardware alphabetically, the most recently heard first, the nearest
/// first, the best powered first. Those that do not say, last.
fn order(column: Column, a: &Node, b: &Node, local: Option<u32>) -> std::cmp::Ordering {
    use std::cmp::Reverse;
    let name = |n: &Node| {
        n.long_name
            .clone()
            .unwrap_or_else(|| n.id.clone())
            .to_lowercase()
    };
    // Nearest first: its own radio, the directly heard by signal, then by
    // hops, MQTT and the unknown last.
    let reach = |n: &Node| {
        let rank = if Some(n.num) == local {
            0
        } else if n.via_mqtt {
            100
        } else {
            n.hops_away.map_or(200, |h| h + 1)
        };
        let signal = if rank == 1 {
            n.snr.unwrap_or(f32::MIN)
        } else {
            0.0
        };
        (rank, Reverse((signal * 100.0) as i64))
    };
    // External first, then the fullest battery, the unknown last.
    let power = |n: &Node| match n.battery_level {
        Some(level) if level > 100 => (0, 0),
        Some(level) => (1, 100 - level),
        None => (2, 0),
    };
    let hardware = |n: &Node| n.hw_model.clone().unwrap_or_default().to_lowercase();
    match column {
        Column::Name => name(a).cmp(&name(b)),
        Column::Heard => b.last_heard.cmp(&a.last_heard),
        Column::Reach => reach(a).cmp(&reach(b)),
        Column::Power => power(a).cmp(&power(b)),
        Column::Hardware => hardware(a).cmp(&hardware(b)),
    }
}

/// Whether a node has nothing to sort by in a column: those go last both
/// ways.
fn unknown(column: Column, node: &Node, local: Option<u32>) -> bool {
    match column {
        Column::Name => false,
        Column::Heard => node.last_heard.is_none(),
        Column::Reach => Some(node.num) != local && !node.via_mqtt && node.hops_away.is_none(),
        Column::Power => node.battery_level.is_none(),
        Column::Hardware => node.hw_model.is_none(),
    }
}

/// The class of a node's avatar: the colours of the map's legend, or its
/// radio's own.
pub fn avatar_class(node: &Node, local: bool) -> String {
    format!(
        "node-avatar {}",
        if local { "local" } else { distance(node) }
    )
}

/// How a node is reached, in words: its own radio, directly, through so many
/// hops, or through MQTT; a MeshCore contact with no route, as having none;
/// none when it is not known.
pub fn reach_words(i18n: I18nContext<Locale>, node: &Node, local: bool) -> Option<String> {
    if local {
        return Some(t_string!(i18n, map_local).to_owned());
    }
    if node.via_mqtt {
        return Some("MQTT".to_owned());
    }
    let Some(hops) = node.hops_away else {
        let meshcore = use_context::<Live>().is_some_and(|live| !is_meshtastic(live));
        return meshcore.then(|| t_string!(i18n, nodes_no_route).to_owned());
    };
    Some(match hops {
        0 => t_string!(i18n, map_direct).to_owned(),
        1 => t_string!(i18n, map_one_hop).to_owned(),
        hops => t_string!(i18n, map_hops).replace("{n}", &hops.to_string()),
    })
}

/// The colour of a node's pin, as the map draws it: the radio's own, or by
/// how far it is (those through MQTT as the farthest).
pub fn pin(node: &Node, local: bool) -> &'static str {
    if local {
        return "--mc-pin-local";
    }
    match distance(node) {
        "direct" => "--mc-pin-direct",
        "near" => "--mc-pin-near",
        "mid" => "--mc-pin-mid",
        _ => "--mc-pin-far",
    }
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
    // By a column, the right way up or not; or, with none, favourites first
    // and then the most recently heard. Kept in the preferences, so that it
    // is the same after a reload and in every browser.
    let sort = Memo::new(move |_| {
        live.preferences
            .with(|p| p.nodes_sort.map(|s| (s.column, !s.reversed)))
            .filter(|(column, _)| *column != Column::Power || is_meshtastic(live))
    });

    // The nodes shown, by their numbers, in their order: only that, so that
    // what follows moves rows rather than drawing them again.
    let shown = Memo::new(move |_| {
        let needle = filter.get().to_lowercase();
        let reach = reach.get();
        let local = live
            .status
            .with(|s| s.as_ref()?.local.as_ref().map(|l| l.num));
        // What the radio hears, not the radio: its own node has its own page.
        let mut nodes: Vec<Node> = live.nodes.with(|nodes| {
            nodes
                .values()
                .filter(|node| Some(node.num) != local)
                .filter(|node| needle.is_empty() || matches(node, &needle))
                .filter(|node| match reach {
                    Reach::All => true,
                    Reach::Direct => !node.via_mqtt && node.hops_away == Some(0),
                    Reach::Mesh => !node.via_mqtt && node.hops_away.is_some_and(|h| h > 0),
                    Reach::Unkept => node.on_radio == Some(false),
                    Reach::Mqtt => node.via_mqtt,
                })
                .cloned()
                .collect()
        });
        match sort.get() {
            Some((column, upright)) => nodes.sort_by(|a, b| {
                let by = order(column, a, b, local);
                unknown(column, a, local)
                    .cmp(&unknown(column, b, local))
                    .then(if upright { by } else { by.reverse() })
                    .then(a.num.cmp(&b.num))
            }),
            None => nodes.sort_by(|a, b| {
                b.favorite
                    .cmp(&a.favorite)
                    .then(b.last_heard.cmp(&a.last_heard))
                    .then(a.num.cmp(&b.num))
            }),
        }
        nodes.into_iter().map(|node| node.num).collect::<Vec<u32>>()
    });
    // How many are drawn: a page at first, and another as the end of the
    // list comes into view; a page again as what is shown changes.
    let limit = RwSignal::new(PAGE);
    Effect::new(move |_| {
        filter.track();
        reach.track();
        sort.track();
        live.selected.track();
        limit.set(PAGE);
    });
    let drawn = move || shown.with(|all| all.iter().take(limit.get()).copied().collect::<Vec<_>>());
    let more = move || shown.with(Vec::len) > limit.get();
    // MeshCore radios have no MQTT, but contacts with no route: each its
    // own filter, and all instead of the other's.
    let mqtt = Memo::new(move |_| is_meshtastic(live));
    Effect::new(move |_| {
        // Each radio's own: Meshtastic's how far, MeshCore's what it keeps.
        let theirs = |r: Reach| matches!(r, Reach::Mesh | Reach::Mqtt);
        let wrong = if mqtt.get() {
            reach.get_untracked() == Reach::Unkept
        } else {
            theirs(reach.get_untracked())
        };
        if wrong {
            reach.set(Reach::All);
        }
    });
    let reaches = move || {
        let shown: &[Reach] = if mqtt.get() {
            &[Reach::All, Reach::Direct, Reach::Mesh, Reach::Mqtt]
        } else {
            &[Reach::All, Reach::Direct, Reach::Unkept]
        };
        shown
            .iter()
            .copied()
            .map(move |this| {
                let label = move || match this {
                    Reach::All => t_string!(i18n, nodes_all).to_string(),
                    Reach::Direct => t_string!(i18n, nodes_direct).to_string(),
                    Reach::Mesh => t_string!(i18n, nodes_mesh).to_string(),
                    Reach::Unkept => t_string!(i18n, nodes_unkept).to_string(),
                    Reach::Mqtt => "MQTT".to_owned(),
                };
                (
                    Signal::derive(move || reach.get() == this),
                    Box::new(move || reach.set(this)) as Box<dyn Fn() + Send + Sync>,
                    Signal::derive(label),
                )
            })
            .collect::<Vec<_>>()
    };

    // A heading that sorts by its column: its most useful way, then the
    // other, then as by default.
    let header = move |column: Column, label: AnyView| {
        let next = move || {
            let mut preferences = live.preferences.get_untracked();
            preferences.nodes_sort = match sort.get_untracked() {
                Some((c, true)) if c == column => Some(NodesSort {
                    column,
                    reversed: true,
                }),
                Some((c, false)) if c == column => None,
                _ => Some(NodesSort {
                    column,
                    reversed: false,
                }),
            };
            live.prefer(preferences);
        };
        let arrow = move || match sort.get() {
            Some((c, true)) if c == column => Some(SORTED),
            Some((c, false)) if c == column => Some(SORTED_BACK),
            _ => None,
        };
        view! {
            <th aria-sort=move || match sort.get() {
                Some((c, true)) if c == column => "ascending",
                Some((c, false)) if c == column => "descending",
                _ => "none",
            }>
                <button type="button" class="sort" class:on=move || arrow().is_some() on:click=move |_| next()>
                    {label}
                    <span class="arrow" inner_html=move || arrow().unwrap_or_default()></span>
                </button>
            </th>
        }
    };

    view! {
        <section class="node-list">
            <div class="node-tools">
                {move || crate::preferences::pills(reaches())}
                <div class="node-search">
                    <input
                        type="search"
                        placeholder=move || t_string!(i18n, search)
                        prop:value=move || filter.get()
                        on:input=move |ev| filter.set(event_target_value(&ev))
                    />
                    <SortMenu sort />
                </div>
            </div>
            <table class="nodes-table">
                <thead>
                    <tr>
                        {header(Column::Name, t!(i18n, nodes_name).into_any())}
                        {header(Column::Heard, t!(i18n, nodes_heard).into_any())}
                        {header(Column::Reach, t!(i18n, nodes_reach).into_any())}
                        {move || is_meshtastic(live).then(|| header(Column::Power, t!(i18n, nodes_battery).into_any()))}
                        {header(Column::Hardware, t!(i18n, nodes_hardware).into_any())}
                        <th></th>
                    </tr>
                </thead>
                <tbody>
                    <For each=drawn key=|num| *num let:num>
                        <NodeRow num now=live.now />
                    </For>
                    {move || more().then(|| view! { <MoreRows limit total=Signal::derive(move || shown.with(Vec::len)) /> })}
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

/// The sorting the table's headings give, for when the list has no headings
/// (in two lines, short of room): a button beside the search and, under it,
/// the columns. The one in use, again, the other way up.
#[component]
fn SortMenu(sort: Memo<Option<(Column, bool)>>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let open = RwSignal::new(false);
    let closing = window_event_listener(leptos::ev::click, move |_| open.set(false));
    on_cleanup(move || closing.remove());
    let choose = move |column: Option<Column>| {
        open.set(false);
        let mut preferences = live.preferences.get_untracked();
        preferences.nodes_sort = column.map(|column| NodesSort {
            column,
            reversed: matches!(sort.get_untracked(), Some((c, true)) if c == column),
        });
        live.prefer(preferences);
    };
    let item = move |column: Option<Column>, label: Signal<String>| {
        let on = move || match (sort.get(), column) {
            (None, None) => true,
            (Some((c, _)), Some(column)) => c == column,
            _ => false,
        };
        let arrow = move || match sort.get() {
            Some((c, upright)) if Some(c) == column => {
                if upright {
                    SORTED
                } else {
                    SORTED_BACK
                }
            }
            _ if on() => CHECK,
            _ => "",
        };
        view! {
            <button
                role="menuitemradio"
                aria-checked=move || on().to_string()
                class:on=on
                on:click=move |_| choose(column)
            >
                <span class="label">{label}</span>
                <span class="icon" inner_html=arrow></span>
            </button>
        }
    };
    let columns = [
        (
            None,
            Signal::derive(move || t_string!(i18n, nodes_sort_default).to_owned()),
        ),
        (
            Some(Column::Name),
            Signal::derive(move || t_string!(i18n, nodes_name).to_owned()),
        ),
        (
            Some(Column::Heard),
            Signal::derive(move || t_string!(i18n, nodes_heard).to_owned()),
        ),
        (
            Some(Column::Reach),
            Signal::derive(move || t_string!(i18n, nodes_reach).to_owned()),
        ),
        (
            Some(Column::Power),
            Signal::derive(move || t_string!(i18n, nodes_battery).to_owned()),
        ),
        (
            Some(Column::Hardware),
            Signal::derive(move || t_string!(i18n, nodes_hardware).to_owned()),
        ),
    ];
    view! {
        <span class="sort-menu">
            <button
                type="button"
                class="icon-button"
                class:on=move || open.get()
                class:sorted=move || sort.get().is_some()
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                title=move || t_string!(i18n, nodes_sort)
                aria-label=move || t_string!(i18n, nodes_sort)
                inner_html=ARROWS_SORT
                on:click=move |ev| {
                    ev.stop_propagation();
                    open.update(|o| *o = !*o);
                }
            ></button>
            <Show when=move || open.get()>
                <div class="heading-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                    {columns
                        .into_iter()
                        .filter(|(column, _)| *column != Some(Column::Power) || is_meshtastic(live))
                        .map(|(column, label)| item(column, label))
                        .collect_view()}
                </div>
            </Show>
        </span>
    }
}

const ARROWS_SORT: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/arrows-sort.svg");
const CHECK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/check.svg");
const SORTED: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
const SORTED_BACK: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-up.svg");
const MESSAGE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/message.svg");
pub const STAR: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/star.svg");
pub const MESSAGE_OFF: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/message-off.svg");
pub const PIN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/map-pin.svg");
const DATABASE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/database.svg");
pub const PLUG: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plug.svg");
pub const BATTERY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/battery-2.svg");

/// How many rows of nodes are drawn at a time.
const PAGE: usize = 100;

/// The end of the rows drawn, while there are more: as it comes into view,
/// or near, another page is drawn.
#[component]
fn MoreRows(limit: RwSignal<usize>, total: Signal<usize>) -> impl IntoView {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let i18n = use_i18n();
    let end = NodeRef::<leptos::html::Td>::new();
    let watch =
        StoredValue::new_local(None::<(web_sys::IntersectionObserver, Closure<dyn FnMut()>)>);
    Effect::new(move |_| {
        let Some(element) = end.get() else { return };
        if watch.with_value(Option::is_some) {
            return;
        }
        let seen = element.clone();
        let call = Closure::<dyn FnMut()>::new(move || {
            let near = web_sys::window()
                .and_then(|w| w.inner_height().ok())
                .and_then(|h| h.as_f64())
                .is_some_and(|height| seen.get_bounding_client_rect().top() < height + 600.0);
            if near {
                limit.update(|l| *l += PAGE);
            }
        });
        let options = web_sys::IntersectionObserverInit::new();
        options.set_root_margin("600px");
        if let Ok(observer) =
            web_sys::IntersectionObserver::new_with_options(call.as_ref().unchecked_ref(), &options)
        {
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
    view! {
        <tr class="more-rows">
            <td colspan="6" node_ref=end>
                {move || {
                    t_string!(i18n, nodes_more)
                        .replace("{shown}", &limit.get().min(total.get()).to_string())
                        .replace("{total}", &total.get().to_string())
                }}
            </td>
        </tr>
    }
}

/// A node's row, by its number: drawn again only as that node changes.
#[component]
fn NodeRow(num: u32, now: RwSignal<u32>) -> impl IntoView {
    let live = expect_context::<Live>();
    let node = Memo::new(move |_| live.nodes.with(|nodes| nodes.get(&num).cloned()));
    move || node.get().map(|node| view! { <NodeLine node now /> })
}

#[component]
fn NodeLine(node: Node, now: RwSignal<u32>) -> impl IntoView {
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
    let mut class = avatar_class(&node, local);
    if crate::badge::is_emoji(&short) {
        class.push_str(" emoji");
    }
    // Heard through others, the SNR is the last relay's, not this node's.
    let reach = {
        let node = node.clone();
        move || {
            let words = reach_words(i18n, &node, local).unwrap_or_default();
            match node.snr {
                Some(snr) if !local && !node.via_mqtt && node.hops_away == Some(0) => {
                    format!("{words} · {snr:.1} dB")
                }
                _ => words,
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
    let unmessagable = node.unmessagable;
    // Not in the radio's list, as it said when last read in full: meshctl
    // alone keeps it.
    let kept = node.on_radio == Some(false);
    // On MeshCore, such a node is added to the radio from its row too.
    let keepable = kept && untrack(|| !is_meshtastic(live));
    let keeping = RwSignal::new(false);
    let keep = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        keeping.set(true);
        leptos::task::spawn_local(async move {
            let _ = crate::daemon::keep_contact(radio, num).await;
            let _ = keeping.try_set(false);
        });
    };
    let keep_label = move || t_string!(i18n, node_keep);

    view! {
        <tr class="node-row" on:click=move |_| live.open_node(num)>
            <td>
                <span class="who">
                    <span class=class>{crate::badge::fit(&short)}</span>
                    <span class="names">
                        <span class="name">
                            <span class="text">{name}</span>
                            {favorite.then(|| view! { <span class="mark star" inner_html=STAR></span> })}
                            {unmessagable.then(|| view! { <span class="mark" title=move || t_string!(i18n, node_unmessagable) inner_html=MESSAGE_OFF></span> })}
                            {kept.then(|| view! { <span class="mark kept" title=move || t_string!(i18n, node_kept) aria-label=move || t_string!(i18n, node_kept) inner_html=DATABASE></span> })}
                            // Where it is, always the last, to find it in the
                            // same place: to the map, and not to its page as
                            // the rest of the row.
                            {placed
                                .then(|| {
                                    view! {
                                        <button
                                            type="button"
                                            class="mark to-map"
                                            title=move || t_string!(i18n, node_on_map)
                                            aria-label=move || t_string!(i18n, node_on_map)
                                            inner_html=PIN
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                live.show_on_map(num);
                                            }
                                        ></button>
                                    }
                                })}
                        </span>
                        <span class="mono dim">{node.id.clone()}</span>
                    </span>
                </span>
            </td>
            <td class="heard dim">{move || heard.map(|t| ago(now.get(), t)).unwrap_or_default()}</td>
            <td class="reach">{reach}</td>
            // Read once: every row is drawn again for another radio.
            {untrack(|| is_meshtastic(live)).then(|| view! { <td class="power-cell">{power}</td> })}
            <td class="hardware">
                <span class="names">
                    <span>{node.hw_model.as_deref().map(model)}</span>
                    <span class="dim small">
                        {move || role.as_deref().map(|r| crate::names::role(i18n.get_locale(), r))}
                    </span>
                </span>
            </td>
            <td class="actions">
                {keepable
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="icon-button keep"
                                disabled=move || keeping.get()
                                title=keep_label
                                aria-label=keep_label
                                on:click=keep
                                inner_html=crate::links::ANTENNA_PLUS
                            ></button>
                        }
                    })}
                // Written to only once the radio keeps it, as MeshCore sends
                // only to its contacts.
                {(!keepable)
                    .then(|| {
                        view! {
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
                        }
                    })}
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

/// How long something has been awaited, five seconds at a time, as the
/// clock goes: in seconds, and past the minute in minutes and seconds, so
/// that it is seen going up to its timeout.
pub fn waited(now: u32, then: u32) -> String {
    let seconds = now.saturating_sub(then) / 5 * 5;
    if seconds < 60 {
        format!("{seconds} s")
    } else {
        format!("{} min {:02} s", seconds / 60, seconds % 60)
    }
}

/// The time, by the second, for what is awaited: so that it shows going five
/// seconds after it was asked, and every five after that, on the dot, and
/// not when the page's own clock, which goes at its own pace, ticks. None
/// for what is not awaited, which goes by the page's clock. It stops with
/// whatever made it.
pub fn awaiting_clock(awaited: bool) -> Option<RwSignal<u32>> {
    awaited.then(|| {
        let clock = RwSignal::new(crate::app::now());
        if let Ok(ticking) = set_interval_with_handle(
            move || clock.set(crate::app::now()),
            std::time::Duration::from_secs(1),
        ) {
            on_cleanup(move || ticking.clear());
        }
        clock
    })
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
    use super::{model, waited};

    #[test]
    fn what_is_awaited_goes_five_seconds_at_a_time() {
        assert_eq!(waited(1004, 1000), "0 s");
        assert_eq!(waited(1007, 1000), "5 s");
        assert_eq!(waited(1059, 1000), "55 s");
        assert_eq!(waited(1060, 1000), "1 min 00 s");
        assert_eq!(waited(1088, 1000), "1 min 25 s");
        assert_eq!(waited(1120, 1000), "2 min 00 s");
    }

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
