// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Many nodes removed at once, from the list of nodes, rather than waiting
//! for meshctl to clean up on its own: of those the list shows, as searched
//! and filtered, the ones not heard for a while, never a favourite. Each way
//! of a node's own drawer, one node after another, as far as it got shown,
//! and stopped at will.

use crate::app::Live;
use crate::daemon::{self, Failure, Forget};
use crate::i18n::{t, t_string, use_i18n};
use crate::node::Drawer;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::ErrorCode;

/// How long unheard a node has to be, as offered, in seconds, and in short.
const AGES: [(u32, &str); 5] = [
    (28_800, "8 h"),
    (86_400, "24 h"),
    (259_200, "3 d"),
    (604_800, "7 d"),
    (2_592_000, "30 d"),
];

/// What a sweep came to: how many went, and how many the radio would not
/// let go, or did not answer for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Swept {
    done: usize,
    failed: usize,
    stopped: bool,
}

/// The list's drawer for removing many nodes at once: open with `open`, of
/// the nodes `shown`.
#[component]
pub fn Sweep(open: RwSignal<bool>, shown: Memo<Vec<u32>>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let age = RwSignal::new(AGES[4].0);
    // While going: how many are done, of how many; and whether to stop.
    let going = RwSignal::new(None::<(usize, usize)>);
    let stopping = RwSignal::new(false);
    let swept = RwSignal::new(None::<Swept>);
    // Closed as the list goes: not open again by itself when it is back.
    on_cleanup(move || open.set(false));
    // Those the list shows and are old enough, never a favourite: by their
    // numbers, and whether the radio has each.
    let picked = Memo::new(move |_| {
        let since = live.now.get().saturating_sub(age.get());
        shown.with(|shown| {
            live.nodes.with(|nodes| {
                shown
                    .iter()
                    .filter_map(|num| nodes.get(num))
                    .filter(|node| !node.favorite)
                    .filter(|node| node.last_heard.is_none_or(|at| at < since))
                    .map(|node| (node.num, node.on_radio != Some(false)))
                    .collect::<Vec<_>>()
            })
        })
    });
    let on_radio = Memo::new(move |_| picked.with(|p| p.iter().filter(|(_, on)| *on).count()));
    // Another age, or the drawer opened again, says nothing of the last
    // sweep; the list changing, as nodes go, does not.
    Effect::new(move |_| {
        age.track();
        open.track();
        swept.set(None);
    });
    let run = move |how: Forget| {
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        // Out of the radio, or read again from it, only what it has.
        let nums: Vec<u32> = picked.with_untracked(|p| {
            p.iter()
                .filter(|(_, on)| how == Forget::Everything || *on)
                .map(|(num, _)| *num)
                .collect()
        });
        let total = nums.len();
        stopping.set(false);
        swept.set(None);
        going.set(Some((0, total)));
        spawn_local(async move {
            let (mut done, mut failed) = (0, 0);
            for num in nums {
                if stopping.get_untracked() {
                    break;
                }
                match daemon::remove_node(radio, num, how).await {
                    // Gone already, as it would be: as good as done.
                    Ok(()) | Err(Failure::Rejected(ErrorCode::NotFound)) => done += 1,
                    // No radio to ask: none of the rest would go either.
                    Err(Failure::Rejected(ErrorCode::RadioNotConnected)) => {
                        failed += total - done - failed;
                        break;
                    }
                    Err(_) => failed += 1,
                }
                let _ = going.try_set(Some((done + failed, total)));
            }
            let _ = going.try_set(None);
            let _ = swept.try_set(Some(Swept {
                done,
                failed,
                stopped: stopping.try_get_untracked().unwrap_or(false) && done + failed < total,
            }));
        });
    };
    let title = Signal::derive(move || t_string!(i18n, sweep_title).to_owned());
    let ages = move || {
        AGES.iter()
            .map(|&(seconds, short)| {
                (
                    Signal::derive(move || age.get() == seconds),
                    Box::new(move || age.set(seconds)) as Box<dyn Fn() + Send + Sync>,
                    Signal::derive(move || short.to_owned()),
                )
            })
            .collect::<Vec<_>>()
    };
    view! {
        <Drawer open title>
            <div class="removal sweep">
                <div class="sweep-pick">
                    <h3>{t!(i18n, sweep_unheard)}</h3>
                    <div class:off=move || going.get().is_some()>
                        {move || crate::preferences::pills(ages())}
                    </div>
                    <p class="dim">
                        {move || {
                            t!(
                                i18n,
                                sweep_count,
                                count = picked.with(Vec::len),
                                total = shown.with(Vec::len)
                            )
                        }}
                    </p>
                </div>
                {move || match going.get() {
                    Some((so_far, total)) => {
                        view! {
                            <div class="sweep-going">
                                <progress max=total value=so_far></progress>
                                <span>{t!(i18n, sweep_going, done = so_far, total = total)}</span>
                                <button
                                    type="button"
                                    disabled=move || stopping.get()
                                    on:click=move |_| stopping.set(true)
                                >
                                    {t!(i18n, sweep_stop)}
                                </button>
                            </div>
                        }
                            .into_any()
                    }
                    None => {
                        view! {
                            {move || {
                                swept
                                    .get()
                                    .map(|s| {
                                        let said = if s.failed > 0 {
                                            t_string!(i18n, sweep_done_failed)
                                                .replace("{done}", &s.done.to_string())
                                                .replace("{failed}", &s.failed.to_string())
                                        } else if s.stopped {
                                            t_string!(i18n, sweep_done_stopped)
                                                .replace("{done}", &s.done.to_string())
                                        } else {
                                            t_string!(i18n, sweep_done)
                                                .replace("{done}", &s.done.to_string())
                                        };
                                        let failed = s.failed > 0;
                                        view! {
                                            <p class="sweep-result" class:problem=failed>
                                                {said}
                                            </p>
                                        }
                                    })
                            }}
                            <Way how=Forget::Radio count=on_radio run />
                            <Way how=Forget::Meshctl count=on_radio run />
                            <Way
                                how=Forget::Everything
                                count=Memo::new(move |_| picked.with(Vec::len))
                                run
                            />
                        }
                            .into_any()
                    }
                }}
            </div>
        </Drawer>
    }
}

/// One way to remove them: what it does, to how many, and doing it, once
/// more said sure.
#[component]
fn Way(
    how: Forget,
    count: Memo<usize>,
    run: impl Fn(Forget) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let sure = RwSignal::new(false);
    let (title, text) = match how {
        Forget::Radio => (
            t!(i18n, remove_radio).into_any(),
            t!(i18n, sweep_radio_text).into_any(),
        ),
        Forget::Meshctl => (
            t!(i18n, remove_meshctl).into_any(),
            t!(i18n, sweep_meshctl_text).into_any(),
        ),
        Forget::Everything => (
            t!(i18n, remove_all).into_any(),
            t!(i18n, sweep_all_text).into_any(),
        ),
    };
    let button = move || {
        let n = count.get();
        match how {
            Forget::Radio => t_string!(i18n, sweep_radio_button),
            Forget::Meshctl => t_string!(i18n, sweep_meshctl_button),
            Forget::Everything => t_string!(i18n, sweep_all_button),
        }
        .replace("{count}", &n.to_string())
    };
    view! {
        <section class="way">
            <h3>{title}</h3>
            <p class="dim">{text}</p>
            <Show
                when=move || sure.get()
                fallback=move || {
                    view! {
                        <button
                            class="danger-outline"
                            disabled=move || count.get() == 0
                            on:click=move |_| sure.set(true)
                        >
                            {button}
                        </button>
                    }
                }
            >
                <div class="forget">
                    <span class="question">{t!(i18n, remove_sure)}</span>
                    <button
                        class="danger"
                        on:click=move |_| {
                            sure.set(false);
                            run(how);
                        }
                    >
                        {t!(i18n, forget_yes)}
                    </button>
                    <button on:click=move |_| sure.set(false)>{t!(i18n, data_cancel)}</button>
                </div>
            </Show>
        </section>
    }
}
