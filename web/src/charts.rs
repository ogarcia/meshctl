// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! A node's telemetry over a while, charted: one chart for each measure,
//! or for two of one unit, drawn as SVG, with what each point says on
//! hovering it.
//!
//! The while is chosen among a few, only those there is history for: up to
//! the first that holds all there is. It is kept in the preferences.

use crate::app::Live;
use crate::daemon;
use crate::i18n::{Locale, t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use meshctl_api::{NodeTelemetry, TelemetryPoint};

/// The whiles there are to choose, in seconds: from a day, as nodes send
/// their telemetry once an hour by default, and routers twice a day.
const WINDOWS: [u32; 6] = [86_400, 172_800, 259_200, 604_800, 1_209_600, 2_592_000];

/// The while charted when none was chosen: a day.
const DEFAULT_WINDOW: u32 = 86_400;

/// How a while is named, short.
fn window_label(i18n: I18nContext<Locale>, window: u32) -> String {
    match window {
        86_400 => "24 h".into(),
        172_800 => "48 h".into(),
        259_200 => "3 d".into(),
        604_800 => "7 d".into(),
        1_209_600 => t_string!(i18n, window_two_weeks).to_owned(),
        _ => t_string!(i18n, window_month).to_owned(),
    }
}

/// The whiles worth offering: up to the first that holds all the history
/// there is, as a longer one would show the same.
fn offered(oldest: Option<i64>, now: i64) -> Vec<u32> {
    let Some(oldest) = oldest else {
        return Vec::new();
    };
    let age = now - oldest;
    let mut windows = Vec::new();
    for window in WINDOWS {
        windows.push(window);
        if i64::from(window) >= age {
            break;
        }
    }
    windows
}

/// A chart to draw: its title, its unit and decimals, its lines, each with
/// its name and its measure, and what its scale is held to.
struct Spec {
    group: Group,
    title: String,
    unit: &'static str,
    decimals: usize,
    lines: Vec<(String, &'static str)>,
    floor: Option<f64>,
    ceiling: Option<f64>,
}

/// What a chart is of, as the node's page groups it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Device,
    Signal,
    Environment,
    Power,
}

impl Group {
    const ALL: [Self; 4] = [Self::Device, Self::Signal, Self::Environment, Self::Power];

    fn title(self, i18n: I18nContext<Locale>) -> String {
        match self {
            Self::Device => t_string!(i18n, node_device),
            Self::Signal => t_string!(i18n, node_signal),
            Self::Environment => t_string!(i18n, node_environment),
            Self::Power => t_string!(i18n, node_power),
        }
        .to_owned()
    }
}

/// The charts there are for a node's series, in the order they are shown:
/// its device's, the signal it is heard at, its environment's and its
/// power's. Each only when it has some point.
fn specs(
    i18n: I18nContext<Locale>,
    telemetry: &NodeTelemetry,
) -> Vec<(Spec, Vec<Vec<TelemetryPoint>>)> {
    let points = |metric: &str| {
        telemetry
            .series
            .iter()
            .find(|s| s.metric == metric)
            .map(|s| s.points.clone())
            .filter(|p| !p.is_empty())
    };
    let mut specs = vec![
        Spec {
            group: Group::Device,
            title: t_string!(i18n, node_battery).to_owned(),
            unit: " %",
            decimals: 0,
            lines: vec![(String::new(), "device.battery_level")],
            floor: Some(0.0),
            ceiling: Some(100.0),
        },
        Spec {
            group: Group::Device,
            title: t_string!(i18n, node_voltage).to_owned(),
            unit: " V",
            decimals: 2,
            lines: vec![(String::new(), "device.voltage")],
            floor: None,
            ceiling: None,
        },
        Spec {
            group: Group::Device,
            title: t_string!(i18n, chart_airtime).to_owned(),
            unit: " %",
            decimals: 1,
            lines: vec![
                (
                    t_string!(i18n, node_channel_use).to_owned(),
                    "device.channel_utilization",
                ),
                (
                    t_string!(i18n, node_air_tx).to_owned(),
                    "device.air_util_tx",
                ),
            ],
            floor: Some(0.0),
            ceiling: None,
        },
        Spec {
            group: Group::Signal,
            title: "SNR".into(),
            unit: " dB",
            decimals: 1,
            lines: vec![(String::new(), "signal.snr")],
            floor: None,
            ceiling: None,
        },
        Spec {
            group: Group::Signal,
            title: "RSSI".into(),
            unit: " dBm",
            decimals: 0,
            lines: vec![(String::new(), "signal.rssi")],
            floor: None,
            ceiling: None,
        },
    ];
    // Every environment value there is, as the node's section names and
    // orders them.
    for key in crate::node::ENVIRONMENT_FIELDS {
        let metric = format!("environment.{key}");
        if points(&metric).is_some()
            && let Some((title, unit, decimals)) = crate::node::environment_field(i18n, key)
        {
            specs.push(Spec {
                group: Group::Environment,
                title,
                unit,
                decimals,
                lines: vec![(String::new(), leak(&metric))],
                floor: None,
                ceiling: None,
            });
        }
    }
    // Each power channel's voltage, and its current, apart: two units.
    for channel in 1..=8 {
        for (field, unit, decimals, word) in [
            ("voltage", " V", 2, t_string!(i18n, env_voltage)),
            ("current", " mA", 0, t_string!(i18n, env_current)),
        ] {
            let metric = format!("power.ch{channel}_{field}");
            if points(&metric).is_some() {
                let title = format!(
                    "{} · {word}",
                    t_string!(i18n, power_channel).replace("{n}", &channel.to_string())
                );
                specs.push(Spec {
                    group: Group::Power,
                    title,
                    unit,
                    decimals,
                    lines: vec![(String::new(), leak(&metric))],
                    floor: None,
                    ceiling: None,
                });
            }
        }
    }
    specs
        .into_iter()
        .filter_map(|spec| {
            let lines: Vec<Vec<TelemetryPoint>> = spec
                .lines
                .iter()
                .map(|(_, metric)| points(metric).unwrap_or_default())
                .collect();
            lines.iter().any(|l| !l.is_empty()).then_some((spec, lines))
        })
        .collect()
}

/// A measure's name, for as long as the page runs: few, and the same ones.
fn leak(metric: &str) -> &'static str {
    Box::leak(metric.to_owned().into_boxed_str())
}

/// A node's telemetry charts, under their choice of while: none when the
/// node has no history at all.
#[component]
pub fn Telemetry(num: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let data = RwSignal::new(None::<NodeTelemetry>);
    let chosen = move || {
        live.preferences
            .with(|p| p.telemetry_window)
            .unwrap_or(DEFAULT_WINDOW)
    };
    // The while charted: the one chosen, or the longest there is history
    // for when that is shorter.
    let window = Memo::new(move |_| {
        let offered = data.with(|d| {
            d.as_ref()
                .map(|d| offered(d.oldest, i64::from(live.now.get())))
                .unwrap_or_default()
        });
        let chosen = chosen();
        if offered.contains(&chosen) || offered.is_empty() {
            chosen
        } else {
            offered.last().copied().unwrap_or(chosen)
        }
    });
    // Read again as the while changes, and as the node tells something new,
    // which its latest of each kind follows.
    Effect::new(move |_| {
        let window = window.get();
        live.latest.track();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Ok(telemetry) = daemon::node_telemetry(radio, num, window).await
                && live.node.get_untracked() == Some(num)
            {
                data.set(Some(telemetry));
            }
        });
    });
    let choose = move |window: u32| {
        let mut preferences = live.preferences.get_untracked();
        preferences.telemetry_window = Some(window);
        live.prefer(preferences);
    };
    // The whiles worth offering, which change seldom: the choice of them
    // stays as it is while another is chosen, so that its pill slides.
    let windows = Memo::new(move |_| {
        data.with(|d| {
            d.as_ref()
                .map(|d| offered(d.oldest, i64::from(live.now.get())))
                .unwrap_or_default()
        })
    });
    let pill = move || {
        let windows = windows.get();
        let at = windows.iter().position(|&w| w == window.get()).unwrap_or(0);
        format!("--n: {}; --at: {at}", windows.len())
    };
    let pills = move || {
        windows
            .get()
            .into_iter()
            .map(|w| {
                view! {
                    <button
                        type="button"
                        role="radio"
                        class:on=move || window.get() == w
                        aria-checked=move || (window.get() == w).to_string()
                        on:click=move |_| choose(w)
                    >
                        {window_label(i18n, w)}
                    </button>
                }
            })
            .collect_view()
    };
    // The charts, in groups as the page's sections: a group with none, left
    // out.
    let body = move || {
        let telemetry = data.get()?;
        let end = i64::from(live.now.get_untracked());
        let start = end - i64::from(window.get());
        let mut charts = specs(i18n, &telemetry);
        if charts.is_empty() {
            return Some(view! { <p class="nothing">{t!(i18n, chart_nothing)}</p> }.into_any());
        }
        Some(
            Group::ALL
                .into_iter()
                .filter_map(|group| {
                    let (of, rest): (Vec<_>, Vec<_>) =
                        charts.drain(..).partition(|(spec, _)| spec.group == group);
                    charts = rest;
                    (!of.is_empty()).then(|| {
                        let of = of
                            .into_iter()
                            .map(|(spec, lines)| view! { <Chart spec lines start end /> })
                            .collect_view();
                        view! {
                            <div class="chart-group">
                                <h3>{group.title(i18n)}</h3>
                                <div class="charts">{of}</div>
                            </div>
                        }
                    })
                })
                .collect_view()
                .into_any(),
        )
    };
    move || {
        (!windows.with(Vec::is_empty)).then(|| {
            view! {
                <section class="node-section telemetry">
                    <header>
                        <h2>{t!(i18n, chart_title)}</h2>
                        <div class="segmented windows" role="radiogroup" style=pill>
                            {pills}
                        </div>
                    </header>
                    {body}
                </section>
            }
        })
    }
}

/// Where a chart's plot is drawn, in the SVG's own units: stretched to the
/// plot's size, the lines keep their width.
const W: f64 = 1000.0;
const H: f64 = 100.0;

/// One chart: its title and latest value, its scale, its lines, and what
/// the points under the pointer say.
#[component]
fn Chart(spec: Spec, lines: Vec<Vec<TelemetryPoint>>, start: i64, end: i64) -> impl IntoView {
    let Spec {
        group: _,
        title,
        unit,
        decimals,
        lines: names,
        floor,
        ceiling,
    } = spec;
    let format = move |value: f64| format!("{value:.decimals$}{unit}");
    let (lo, hi) = scale(&lines, floor, ceiling);
    let span = (end - start).max(1) as f64;
    let x = move |at: i64| (at - start) as f64 / span * W;
    let y = move |value: f64| H - (value - lo) / (hi - lo) * H;
    let paths: Vec<String> = lines
        .iter()
        .map(|points| path(points, x, y, (end - start) / 50))
        .collect();
    let latest = lines
        .first()
        .and_then(|points| points.last())
        .map(|p| format(p.value));
    let hover = RwSignal::new(None::<f64>);
    let plot = NodeRef::<leptos::html::Div>::new();
    let on_move = move |ev: leptos::ev::PointerEvent| {
        if let Some(plot) = plot.get_untracked() {
            let rect = plot.get_bounding_client_rect();
            let fraction = (f64::from(ev.client_x()) - rect.left()) / rect.width();
            hover.set(Some(fraction.clamp(0.0, 1.0)));
        }
    };
    let lines_for_hover = lines.clone();
    let names_for_hover = names.clone();
    // The points nearest the pointer's time, one for each line.
    let under = move || {
        let fraction = hover.get()?;
        let at = start + (fraction * span) as i64;
        let first = nearest(lines_for_hover.first()?, at)?;
        let values: Vec<(String, Option<f64>, usize)> = lines_for_hover
            .iter()
            .enumerate()
            .map(|(index, points)| {
                (
                    names_for_hover[index].0.clone(),
                    nearest(points, first.at).map(|p| p.value),
                    index,
                )
            })
            .collect();
        Some((first.at, values))
    };
    let legend = (names.len() > 1).then(|| {
        names
            .iter()
            .enumerate()
            .map(|(index, (name, _))| {
                view! {
                    <span class="legend-key">
                        <i class=format!("swatch line-{index}")></i>
                        {name.clone()}
                    </span>
                }
            })
            .collect_view()
    });
    let mid = (lo + hi) / 2.0;
    view! {
        <figure class="chart" role="img" aria-label=title.clone()>
            <figcaption>
                <span class="title">{title.clone()}</span>
                <span class="latest">{latest}</span>
            </figcaption>
            <div class="frame">
                <div class="scale">
                    <span>{format(hi)}</span>
                    <span>{format(mid)}</span>
                    <span>{format(lo)}</span>
                </div>
                <div
                    class="plot"
                    node_ref=plot
                    on:pointermove=on_move
                    on:pointerleave=move |_| hover.set(None)
                >
                    <svg viewBox=format!("0 0 {W} {H}") preserveAspectRatio="none" aria-hidden="true">
                        <line class="grid" x1="0" x2=W y1="0" y2="0"></line>
                        <line class="grid" x1="0" x2=W y1=H / 2.0 y2=H / 2.0></line>
                        <line class="grid" x1="0" x2=W y1=H y2=H></line>
                        {paths
                            .into_iter()
                            .enumerate()
                            .map(|(index, d)| {
                                view! { <path class=format!("line line-{index}") d=d></path> }
                            })
                            .collect_view()}
                    </svg>
                    {move || {
                        under()
                            .map(|(at, values)| {
                                let left = x(at) / W * 100.0;
                                let side = if left > 60.0 { "left" } else { "right" };
                                let rows = values
                                    .into_iter()
                                    .map(|(name, value, index)| {
                                        view! {
                                            <span class="row">
                                                <i class=format!("swatch line-{index}")></i>
                                                {(!name.is_empty()).then(|| format!("{name}: "))}
                                                <strong>{value.map(format)}</strong>
                                            </span>
                                        }
                                    })
                                    .collect_view();
                                view! {
                                    <i class="cursor" style=format!("left: {left:.2}%")></i>
                                    <div class=format!("tip {side}") style=format!("left: {left:.2}%")>
                                        <span class="when">{moment(at, (end - start).min(259_200))}</span>
                                        {rows}
                                    </div>
                                }
                            })
                    }}
                </div>
                <div class="times">
                    <span>{moment(start, end - start)}</span>
                    <span>{moment(start + (end - start) / 2, end - start)}</span>
                    <span>{moment(end, end - start)}</span>
                </div>
            </div>
            {legend.map(|legend| view! { <div class="legend">{legend}</div> })}
        </figure>
    }
}

/// The scale a chart's values go on: what the measure is held to, or else
/// their least and most, with some room around.
fn scale(lines: &[Vec<TelemetryPoint>], floor: Option<f64>, ceiling: Option<f64>) -> (f64, f64) {
    let values = lines.iter().flatten().map(|p| p.value);
    let (min, max) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    if !min.is_finite() {
        return (0.0, 1.0);
    }
    let room = ((max - min) * 0.15).max(if max.abs() > 10.0 { 1.0 } else { 0.1 });
    let lo = floor.map_or(min - room, |floor| floor.min(min));
    let hi = ceiling.map_or(max + room, |ceiling| ceiling.max(max));
    if hi > lo {
        (lo, hi)
    } else {
        (lo - 1.0, hi + 1.0)
    }
}

/// A line through points, broken where they are further apart than a
/// stretch: a while with nothing heard shows as a gap.
fn path(
    points: &[TelemetryPoint],
    x: impl Fn(i64) -> f64,
    y: impl Fn(f64) -> f64,
    gap: i64,
) -> String {
    // Further apart than what they usually are, too.
    let mut steps: Vec<i64> = points.windows(2).map(|w| w[1].at - w[0].at).collect();
    steps.sort_unstable();
    let usual = steps.get(steps.len() / 2).copied().unwrap_or(0);
    let gap = gap.max(usual * 3);
    let mut d = String::new();
    let mut last: Option<i64> = None;
    for point in points {
        let (px, py) = (x(point.at), y(point.value));
        match last {
            Some(at) if point.at - at <= gap => d.push_str(&format!("L{px:.1} {py:.2}")),
            // A new stretch: a point alone shows as a dot, by the line's caps.
            _ => d.push_str(&format!("M{px:.1} {py:.2}l0 0")),
        }
        last = Some(point.at);
    }
    d
}

/// The point nearest a time, of points in order.
fn nearest(points: &[TelemetryPoint], at: i64) -> Option<TelemetryPoint> {
    let index = points.partition_point(|p| p.at < at);
    let before = index.checked_sub(1).and_then(|i| points.get(i));
    let after = points.get(index);
    match (before, after) {
        (Some(b), Some(a)) => Some(if at - b.at <= a.at - at { *b } else { *a }),
        (Some(p), None) | (None, Some(p)) => Some(*p),
        (None, None) => None,
    }
}

/// A moment, in this browser's time: its hour within a day's while, its day
/// and hour within a few days', its day beyond.
fn moment(at: i64, window: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64 * 1000.0));
    let (day, month, hour, minute) = (
        date.get_date(),
        date.get_month() + 1,
        date.get_hours(),
        date.get_minutes(),
    );
    if window <= 86_400 {
        format!("{hour:02}:{minute:02}")
    } else if window <= 259_200 {
        format!("{day:02}/{month:02} {hour:02}:{minute:02}")
    } else {
        format!("{day:02}/{month:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whiles_are_offered_up_to_the_first_that_holds_all() {
        assert!(offered(None, 1000).is_empty());
        // Less than a day: a day, the shortest there is.
        assert_eq!(offered(Some(1000 - 600), 1000), [86_400]);
        // Two days and a half: up to three.
        assert_eq!(
            offered(Some(1_000_000 - 60 * 3600), 1_000_000),
            [86_400, 172_800, 259_200]
        );
        assert_eq!(offered(Some(0), 100_000_000).len(), WINDOWS.len());
    }

    #[test]
    fn a_line_breaks_where_nothing_was_heard() {
        let point = |at, value| TelemetryPoint { at, value };
        let points = [
            point(0, 0.0),
            point(10, 1.0),
            point(20, 2.0),
            point(500, 3.0),
        ];
        let d = path(&points, |at| at as f64, |v| v, 50);
        assert_eq!(d, "M0.0 0.00l0 0L10.0 1.00L20.0 2.00M500.0 3.00l0 0");
    }

    #[test]
    fn the_nearest_point_is_found() {
        let point = |at| TelemetryPoint { at, value: 0.0 };
        let points = [point(0), point(10), point(30)];
        assert_eq!(nearest(&points, 4).unwrap().at, 0);
        assert_eq!(nearest(&points, 21).unwrap().at, 30);
        assert_eq!(nearest(&points, 99).unwrap().at, 30);
        assert!(nearest(&[], 5).is_none());
    }
}
