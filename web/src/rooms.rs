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
                    type="password"
                    maxlength="15"
                    autocomplete="off"
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
