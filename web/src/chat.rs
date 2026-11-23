// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The chat: the selected radio's conversations, the open one, and writing.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use crate::mesh::ago;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    BROADCAST, ErrorCode, LinkState, Message, MessageStatus, NewMessage, Peer, RadioKind,
};
use wasm_bindgen::JsCast;

/// The longest text the daemon takes, in bytes.
/// What sends a message, as an icon.
const SMILE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/mood-smile.svg");
const SEND: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/send.svg");

const MAX_TEXT_BYTES: usize = 200;

/// From how many bytes a message is near what it takes, and its field says
/// so.
const NEAR_TEXT_BYTES: usize = MAX_TEXT_BYTES - 20;

/// A text split where it nears what a message takes and where it goes
/// past, in bytes, each at a character: what is well within, what is near
/// the end, and what does not fit.
fn past_limit(text: &str) -> (String, String, String) {
    let at = |limit: usize| {
        text.char_indices()
            .find(|(at, c)| at + c.len_utf8() > limit)
            .map_or(text.len(), |(at, _)| at)
    };
    let (near, over) = (at(NEAR_TEXT_BYTES), at(MAX_TEXT_BYTES));
    (
        text[..near].to_owned(),
        text[near..over].to_owned(),
        text[over..].to_owned(),
    )
}

/// Who a message was exchanged with.
pub fn peer_of(message: &Message) -> Peer {
    if message.to == BROADCAST {
        Peer::Channel {
            channel: message.channel,
        }
    } else if message.outgoing {
        Peer::Node { node: message.to }
    } else {
        Peer::Node { node: message.from }
    }
}

/// A modem preset's name as the firmware gives it to an unnamed channel, in
/// any slot: `LONG_FAST` is LongFast.
pub fn preset_name(preset: &str) -> String {
    match preset {
        "LONG_MODERATE" => return "LongMod".to_owned(),
        // One the firmware has no name for.
        "VERY_LONG_SLOW" => return "Invalid".to_owned(),
        _ => {}
    }
    preset
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect()
}

/// What a conversation is called.
pub fn title(live: Live, peer: Peer) -> String {
    match peer {
        Peer::Channel { channel } => {
            let named = live.channels.with(|channels| {
                channels
                    .iter()
                    .find(|c| c.index == channel)
                    .map(|c| c.name.clone())
                    .filter(|name| !name.is_empty())
            });
            // An unnamed one, in any slot, goes by its modem preset, as the
            // radio names it, or by Custom with none; by its slot while
            // neither is known.
            let name = named
                .or_else(|| {
                    if live.selected_kind() != Some(RadioKind::Meshtastic) {
                        return None;
                    }
                    live.status.with(|s| {
                        let local = s.as_ref()?.local.as_ref()?;
                        Some(
                            local
                                .modem_preset
                                .as_deref()
                                .map_or_else(|| "Custom".to_owned(), preset_name),
                        )
                    })
                })
                .unwrap_or_else(|| {
                    let i18n = use_i18n();
                    format!("{} {channel}", t_string!(i18n, channel_numbered))
                });
            format!("# {name}")
        }
        Peer::Node { node } => node_name(live, node),
    }
}

pub fn node_name(live: Live, num: u32) -> String {
    live.nodes.with(|nodes| {
        nodes
            .get(&num)
            .and_then(|n| n.long_name.clone())
            .unwrap_or_else(|| format!("!{num:08x}"))
    })
}

pub fn short_name(live: Live, num: u32) -> String {
    live.nodes.with(|nodes| {
        nodes
            .get(&num)
            .and_then(|n| n.short_name.clone())
            .unwrap_or_else(|| format!("{:04x}", num & 0xffff))
    })
}

/// The open conversation. Which ones there are is in the sidebar.
#[component]
pub fn ChatView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let list = NodeRef::<html::Div>::new();

    // Keep the newest message in view as they come.
    Effect::new(move |_| {
        live.thread.track();
        if let Some(list) = list.get() {
            list.set_scroll_top(list.scroll_height());
        }
    });

    view! {
        <div class="thread">
            {move || match live.open.get() {
                None => view! { <p class="empty dim">{t!(i18n, chat_pick)}</p> }.into_any(),
                Some(peer) => view! {
                    <div class="messages" node_ref=list>
                        <For
                            each=move || live.thread.get()
                            key=|m| (m.id, m.status)
                            let:message
                        >
                            <Bubble message peer />
                        </For>
                    </div>
                    <Composer peer />
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn Bubble(message: Message, peer: Peer) -> impl IntoView {
    let live = expect_context::<Live>();
    let at = u32::try_from(message.at).unwrap_or_default();
    let mine = message.outgoing;
    // In a channel, who said it; in a direct conversation it is obvious.
    let author = (!mine && matches!(peer, Peer::Channel { .. })).then(|| {
        // A sender known only by the name the message gives, as on MeshCore
        // channels, is shown by that name.
        let known = message.from != 0;
        let name = match (&message.sender_name, known) {
            (Some(name), false) => name.clone(),
            _ => node_name(live, message.from),
        };
        let short = known
            .then(|| short_name(live, message.from))
            .filter(|s| !s.is_empty());
        view! {
            <span class="author">
                {short.map(|s| view! { <span class="short">{s}</span> " " })}
                {name}
            </span>
        }
    });
    let reception = (!mine)
        .then(|| {
            let mut parts = Vec::new();
            if message.via_mqtt {
                parts.push("MQTT".to_owned());
            } else if let Some(hops) = message.hops_away {
                parts.push(format!("{hops} ⇢"));
            }
            if let Some(snr) = message.snr.filter(|_| message.hops_away == Some(0)) {
                parts.push(format!("{snr:.1} dB"));
            }
            parts.join(" · ")
        })
        .filter(|meta| !meta.is_empty());
    let (mark, mark_class) = match message.status {
        MessageStatus::Received => ("", ""),
        MessageStatus::Sending => ("…", "pending"),
        MessageStatus::Sent => ("✓", "sent"),
        MessageStatus::Delivered => ("✓✓", "delivered"),
        MessageStatus::Failed => ("⚠", "failed"),
    };
    let error = message.error.clone().unwrap_or_default();

    view! {
        <div class=if mine { "bubble mine" } else { "bubble" }>
            {author}
            <p>{message.text.clone()}</p>
            <span class="meta dim">
                {move || ago(live.now.get(), at)}
                {reception.map(|r| format!(" · {r}"))}
                " "
                <span class=format!("mark {mark_class}") title=error>{mark}</span>
            </span>
        </div>
    }
}

#[component]
fn Composer(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let text = RwSignal::new(String::new());
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    let bytes = move || text.with(String::len);
    let over = move || bytes() > MAX_TEXT_BYTES;
    let connected = move || {
        live.status
            .with(|s| s.as_ref().is_some_and(|s| s.link == LinkState::Ready))
    };
    let can_send = move || {
        connected()
            && !busy.get()
            && !text.with(|t| t.trim().is_empty())
            && bytes() <= MAX_TEXT_BYTES
    };

    // As tall as what is written, wrapped even where it has no spaces, up to
    // a few lines; scrolled within past them. Again as it is emptied.
    let field = NodeRef::<html::Textarea>::new();
    let backdrop = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        text.track();
        field.track();
        // Measured once the field holds the text, which it takes after this.
        request_animation_frame(move || {
            let Some(field) = field.get_untracked() else {
                return;
            };
            let style = web_sys::HtmlElement::style(&field);
            let _ = style.set_property("height", "auto");
            let border = field.offset_height() - field.client_height();
            let _ = style.set_property("height", &format!("{}px", field.scroll_height() + border));
        });
    });
    // By touch the keyboard's return writes a new line, as in the phones'
    // apps, and the button sends; with a keyboard, return sends, and with
    // Ctrl or Shift writes a new line.
    let touch = web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches());
    let keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() != "Enter" || ev.is_composing() || touch {
            return;
        }
        ev.prevent_default();
        if ev.ctrl_key() || ev.shift_key() || ev.meta_key() {
            // As if typed: where the caret is, undone as typing is.
            let _ = document().dyn_into::<web_sys::HtmlDocument>().map(|d| {
                d.exec_command_with_show_ui_and_value(
                    "insertText",
                    false,
                    "
",
                )
            });
        } else if let Some(form) = field.get().and_then(|f| f.form()) {
            let _ = form.request_submit();
        }
    };

    // The emojis, over the bar: each one picked goes where the caret is,
    // and they stay open for more, as in the phones' apps. Escape, a click
    // anywhere else or sending close them.
    let picking = RwSignal::new(false);
    let closing = window_event_listener(leptos::ev::click, move |_| picking.set(false));
    let escaping = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" {
            picking.set(false);
        }
    });
    on_cleanup(move || {
        closing.remove();
        escaping.remove();
    });
    let insert = move |emoji: String| {
        let Some(field) = field.get_untracked() else {
            return;
        };
        let value = field.value();
        // Where the caret is, in the UTF-16 units the field counts in.
        let at = |units: Option<u32>| {
            let units = units.unwrap_or(u32::MAX) as usize;
            let mut counted = 0;
            value
                .char_indices()
                .find(|(_, c)| {
                    counted += c.len_utf16();
                    counted > units
                })
                .map_or(value.len(), |(at, _)| at)
        };
        let (start, end) = (
            at(field.selection_start().ok().flatten()),
            at(field.selection_end().ok().flatten()),
        );
        let written = format!("{}{emoji}{}", &value[..start], &value[end..]);
        let caret =
            u32::try_from(value[..start].encode_utf16().count() + emoji.encode_utf16().count())
                .unwrap_or_default();
        field.set_value(&written);
        text.set(written);
        let _ = field.set_selection_range(caret, caret);
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(radio) = live.selected.get_untracked() else {
            return;
        };
        if !can_send() {
            return;
        }
        let (channel, node) = match peer {
            Peer::Channel { channel } => (Some(channel), None),
            Peer::Node { node } => (None, Some(node)),
        };
        let message = NewMessage {
            text: text.get_untracked(),
            channel,
            node,
            reply_id: None,
        };
        busy.set(true);
        spawn_local(async move {
            match daemon::send_message(radio, &message).await {
                Ok(sent) => {
                    text.set(String::new());
                    picking.set(false);
                    error.set(None);
                    live.thread.update(|thread| {
                        if !thread.iter().any(|m| m.id == sent.id) {
                            thread.push(sent);
                        }
                    });
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };

    let problem = move || {
        if !connected() {
            return Some(t!(i18n, chat_not_connected).into_any());
        }
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::RadioNotConnected) => {
                t!(i18n, chat_not_connected).into_any()
            }
            Failure::Rejected(ErrorCode::InvalidMessage) => t!(i18n, chat_invalid).into_any(),
            _ => t!(i18n, error_failed).into_any(),
        })
    };

    // A node that tells nobody reads what is sent to it: said over where it
    // is written, and the button says it goes all the same.
    let unmessagable = move || match peer {
        Peer::Node { node } => live
            .nodes
            .with(|nodes| nodes.get(&node).is_some_and(|n| n.unmessagable)),
        Peer::Channel { .. } => false,
    };
    let send_label = move || {
        if unmessagable() {
            t_string!(i18n, chat_send_anyway)
        } else {
            t_string!(i18n, chat_send)
        }
    };

    view! {
        {move || {
            unmessagable()
                .then(|| {
                    view! {
                        <p class="unmessagable">
                            <span class="icon" inner_html=crate::mesh::MESSAGE_OFF></span>
                            {t!(i18n, chat_unmessagable)}
                        </p>
                    }
                })
        }}
        <form class="composer" on:submit=submit>
            // Behind what is written, the same text unseen but for what
            // goes past what a message takes, in red: a field cannot colour
            // a part of its own text.
            <div
                class="field"
                class:over=over
            >
                <div class="backdrop" node_ref=backdrop aria-hidden="true">
                    {move || {
                        let (fits, near, over) = text.with(|t| past_limit(t));
                        view! {
                            {fits}
                            <mark class="near">{near}</mark>
                            <mark class="over">{over}</mark>
                            // A last new line takes a line too.
                            " "
                        }
                    }}
                </div>
                <textarea
                    node_ref=field
                    rows="1"
                    placeholder=move || t_string!(i18n, chat_write)
                    prop:value=move || text.get()
                    on:input=move |ev| text.set(event_target_value(&ev))
                    on:keydown=keydown
                    on:scroll=move |_| {
                        if let (Some(field), Some(backdrop)) = (field.get_untracked(), backdrop.get_untracked()) {
                            backdrop.set_scroll_top(field.scroll_top());
                        }
                    }
                ></textarea>
                <button
                    type="button"
                    class="icon-button emoji-toggle"
                    class:on=move || picking.get()
                    aria-expanded=move || picking.get().to_string()
                    title=move || t_string!(i18n, chat_emoji)
                    aria-label=move || t_string!(i18n, chat_emoji)
                    inner_html=SMILE
                    on:click=move |ev| {
                        ev.stop_propagation();
                        picking.update(|p| *p = !*p);
                    }
                ></button>
            </div>
            <Show when=move || picking.get()>
                <div class="emoji-popover" on:click=|ev| ev.stop_propagation()>
                    <crate::emoji::EmojiPicker picked=Signal::stored(None::<String>) pick=Callback::new(insert) />
                </div>
            </Show>
            <span class=move || {
                if bytes() > MAX_TEXT_BYTES { "count over" } else { "count dim" }
            }>{move || format!("{}/{MAX_TEXT_BYTES}", bytes())}</span>
            // Its icon, and its word where there is room: sending anyway, to a
            // node that takes no messages, plain and not in the accent.
            <button
                type="submit"
                class="send"
                class:primary=move || !unmessagable()
                disabled=move || !can_send()
                title=send_label
                aria-label=send_label
            >
                <span class="icon" inner_html=SEND></span>
                <span class="label">{send_label}</span>
            </button>
            <p class="error">{problem}</p>
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TEXT_BYTES, NEAR_TEXT_BYTES, past_limit, preset_name};

    #[test]
    fn what_nears_and_goes_past_a_message_splits_at_a_character() {
        let fits = "a".repeat(NEAR_TEXT_BYTES);
        assert_eq!(
            past_limit(&fits),
            (fits.clone(), String::new(), String::new())
        );
        let near = "a".repeat(MAX_TEXT_BYTES);
        assert_eq!(
            past_limit(&near),
            (
                fits.clone(),
                "a".repeat(MAX_TEXT_BYTES - NEAR_TEXT_BYTES),
                String::new()
            )
        );
        let over = format!("{}ñb", "a".repeat(MAX_TEXT_BYTES - 1));
        // The ñ takes two bytes, one past: it goes, whole.
        assert_eq!(
            past_limit(&over),
            (
                fits,
                "a".repeat(MAX_TEXT_BYTES - NEAR_TEXT_BYTES - 1),
                "ñb".to_owned()
            )
        );
    }

    #[test]
    fn presets_read_as_people_write_them() {
        assert_eq!(preset_name("LONG_FAST"), "LongFast");
        assert_eq!(preset_name("NARROW_FAST"), "NarrowFast");
        assert_eq!(preset_name("LONG_MODERATE"), "LongMod");
    }
}
