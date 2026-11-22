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

/// The longest text the daemon takes, in bytes.
const MAX_TEXT_BYTES: usize = 200;

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
            <input
                placeholder=move || t_string!(i18n, chat_write)
                prop:value=move || text.get()
                on:input=move |ev| text.set(event_target_value(&ev))
            />
            <span class=move || {
                if bytes() > MAX_TEXT_BYTES { "count over" } else { "count dim" }
            }>{move || format!("{}/{MAX_TEXT_BYTES}", bytes())}</span>
            <button
                type="submit"
                class:primary=move || !unmessagable()
                disabled=move || !can_send()
            >
                {move || {
                    if unmessagable() {
                        t_string!(i18n, chat_send_anyway).to_owned()
                    } else {
                        t_string!(i18n, chat_send).to_owned()
                    }
                }}
            </button>
            <p class="error">{problem}</p>
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::preset_name;

    #[test]
    fn presets_read_as_people_write_them() {
        assert_eq!(preset_name("LONG_FAST"), "LongFast");
        assert_eq!(preset_name("NARROW_FAST"), "NarrowFast");
        assert_eq!(preset_name("LONG_MODERATE"), "LongMod");
    }
}
