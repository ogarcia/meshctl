// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! MeshCore rooms, which send their posts only to those logged in: logging
//! in from the conversation, what the radio was let in as, and logging in
//! again when the posts stop coming.

use crate::app::Live;
use crate::daemon::Failure;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use meshctl_api::{ErrorCode, RoomAccess};

const LOCK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/lock.svg");
const LOGIN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/login-2.svg");
const KEY_OFF: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/key-off.svg");

/// What the selected radio was let in to the open room as, if it logged in
/// since it connected.
pub fn access(live: Live, node: u32) -> Option<RoomAccess> {
    live.room.with(|r| {
        r.as_ref()
            .filter(|r| r.node == node)
            .and_then(|r| r.login.as_ref())
            .and_then(|l| l.access)
    })
}

/// Whether the radio can post in the open room: logged in to more than read.
pub fn can_post(live: Live, node: u32) -> bool {
    access(live, node).is_some_and(|a| a != RoomAccess::ReadOnly)
}

/// What the radio was let in to a room as, in words, for under its name.
#[component]
pub fn AccessWords(node: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    move || {
        let busy = live
            .room
            .with(|r| r.as_ref().is_some_and(|r| r.node == node && r.busy));
        if busy {
            return t_string!(i18n, room_logging_in).to_owned();
        }
        match access(live, node) {
            Some(RoomAccess::Admin) => t_string!(i18n, room_admin),
            Some(RoomAccess::ReadWrite) => t_string!(i18n, room_read_write),
            Some(RoomAccess::ReadOnly) => t_string!(i18n, room_read_only),
            None => t_string!(i18n, room_out),
        }
        .to_owned()
    }
}

/// Logging in again with the password kept, for when the room's posts stop
/// coming: it forgets those it cannot reach, and a guest when it restarts.
#[component]
pub fn LogInAgain(node: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let busy = move || {
        live.room
            .with(|r| r.as_ref().is_some_and(|r| r.node == node && r.busy))
    };
    let label = move || {
        if access(live, node).is_some() {
            t_string!(i18n, room_log_in_again)
        } else {
            t_string!(i18n, room_log_in)
        }
    };
    view! {
        <button
            class="icon-button"
            title=label
            aria-label=label
            disabled=busy
            inner_html=LOGIN
            on:click=move |_| live.log_in_room(node, None)
        ></button>
    }
}

/// Whether a login to a room is under way.
pub fn logging_in(live: Live, node: u32) -> bool {
    live.room
        .with(|r| r.as_ref().is_some_and(|r| r.node == node && r.busy))
}

/// The circumference of the ring that empties as a login is waited for,
/// at its radius of 26.
const RING: f64 = 2.0 * std::f64::consts::PI * 26.0;

/// While a login to a room is waited for, in the middle of the conversation:
/// a ring that empties as the time runs out, and the seconds left; until
/// the daemon says how long, a ring that turns.
#[component]
pub fn LoggingIn(node: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let now = RwSignal::new(js_sys::Date::now());
    let ticking = set_interval_with_handle(
        move || now.set(js_sys::Date::now()),
        std::time::Duration::from_millis(200),
    )
    .ok();
    on_cleanup(move || {
        if let Some(ticking) = ticking {
            ticking.clear();
        }
    });
    let until = move || {
        live.room
            .with(|r| r.as_ref().filter(|r| r.node == node).and_then(|r| r.until))
    };
    let left = move || until().map(|(until, _)| ((until - now.get()) / 1000.0).max(0.0));
    let offset = move || {
        until().map_or(RING * 0.75, |(until, wait)| {
            RING * (1.0 - ((until - now.get()) / wait).clamp(0.0, 1.0))
        })
    };
    view! {
        <div class="logging-in" role="status">
            <div class="ring" class:turning=move || until().is_none()>
                <svg viewBox="0 0 60 60" aria-hidden="true">
                    <circle class="track" cx="30" cy="30" r="26"></circle>
                    <circle
                        class="left"
                        cx="30"
                        cy="30"
                        r="26"
                        stroke-dasharray=RING.to_string()
                        stroke-dashoffset=move || offset().to_string()
                    ></circle>
                </svg>
                <span class="seconds">
                    {move || left().map(|left| format!("{}", left.ceil()))}
                </span>
            </div>
            <p>{t!(i18n, room_logging_in)}</p>
            <p class="dim">
                {move || {
                    if until().is_some() {
                        t_string!(i18n, room_waiting)
                    } else {
                        t_string!(i18n, room_sending)
                    }
                }}
            </p>
        </div>
    }
}

/// Forgetting the password kept for a room: there only while one is.
#[component]
pub fn ForgetPassword(node: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let remembered = move || {
        live.room.with(|r| {
            r.as_ref()
                .filter(|r| r.node == node)
                .and_then(|r| r.login.as_ref())
                .is_some_and(|l| l.remembered)
        })
    };
    move || {
        remembered().then(|| {
            view! {
                <button
                    class="icon-button"
                    title=move || t_string!(i18n, room_forget_password)
                    aria-label=move || t_string!(i18n, room_forget_password)
                    inner_html=KEY_OFF
                    on:click=move |_| {
                        live.forget_room_password(node, t_string!(i18n, room_forget_sure));
                    }
                ></button>
            }
        })
    }
}

/// In place of the composer, while the radio cannot post in the room: why,
/// and a password to log in with.
#[component]
pub fn RoomBar(node: u32) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let password = RwSignal::new(String::new());
    let state = move || live.room.with(|r| r.clone().filter(|r| r.node == node));
    let busy = move || state().is_some_and(|r| r.busy);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let given = password.get_untracked();
        live.log_in_room(node, (!given.is_empty()).then_some(given));
    };
    let why = move || match access(live, node) {
        Some(RoomAccess::ReadOnly) => t_string!(i18n, room_why_read_only),
        _ => t_string!(i18n, room_why_out),
    };
    let error = move || {
        state()?.error.map(|failure| match failure {
            Failure::Rejected(ErrorCode::WrongPassword) => t_string!(i18n, room_wrong_password),
            Failure::Rejected(ErrorCode::RadioDidNotAnswer) => t_string!(i18n, room_no_answer),
            Failure::Rejected(ErrorCode::RadioNotConnected) => {
                t_string!(i18n, chat_not_connected)
            }
            _ => t_string!(i18n, room_failed),
        })
    };
    view! {
        <form class="room-bar" on:submit=submit>
            <p class="why">
                <span class="icon" inner_html=LOCK></span>
                <span>{why}</span>
            </p>
            <div class="fields">
                <input
                    // Text, hidden by its style, not a password field: a room's
                    // password is meshctl's to keep, not the browser's to offer
                    // to save, nor a password manager's.
                    type="text"
                    class="secret"
                    maxlength="15"
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    data-lpignore="true"
                    data-1p-ignore="true"
                    data-bwignore="true"
                    data-form-type="other"
                    placeholder=move || t_string!(i18n, room_password)
                    aria-label=move || t_string!(i18n, room_password)
                    prop:value=move || password.get()
                    on:input=move |ev| password.set(event_target_value(&ev))
                />
                <button type="submit" class="primary" disabled=busy>
                    {move || {
                        if busy() {
                            t!(i18n, room_logging_in).into_any()
                        } else {
                            t!(i18n, room_log_in).into_any()
                        }
                    }}
                </button>
            </div>
            {move || error().map(|e| view! { <p class="error">{e}</p> })}
        </form>
    }
}
