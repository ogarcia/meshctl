// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The chat: the selected radio's conversations, the open one, and writing.

use crate::app::Live;
use crate::daemon::{self, Failure};
use crate::i18n::{Locale, t, t_string, use_i18n};
use crate::mesh::ago;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::I18nContext;
use meshctl_api::{
    BROADCAST, ErrorCode, LinkState, Message, MessageId, MessageStatus, NewMessage, Peer, RadioKind,
};
use wasm_bindgen::JsCast;

/// What sends a message, as an icon.
const SEND: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/send.svg");
const SMILE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/mood-smile.svg");
const REPLY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/arrow-back-up.svg");
const PLUS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/plus.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const TRASH: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/trash.svg");
const COPY: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/copy.svg");
/// How far a message went.
const CLOCK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/clock.svg");
const CHECK: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/check.svg");
const CHECKS: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/checks.svg");
const ALERT: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/alert-triangle.svg");

/// The longest text the daemon takes, in bytes.
const MAX_TEXT_BYTES: usize = 200;

/// How many emojis a reaction offers at once, before the rest.
const REACTIONS_OFFERED: usize = 7;

/// How many reactions a message shows, the most given, before the rest.
const REACTIONS_SHOWN: usize = 4;

/// How many of the emojis most used the chat offers first: four rows.
const FREQUENT_SHOWN: usize = 36;

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

/// What is being done with the open conversation's messages: which one is
/// answered, which one has its reactions open (beside it, with a mouse),
/// which one has its menu open (by touch).
#[derive(Clone, Copy)]
struct Thread {
    replying: RwSignal<Option<u32>>,
    reacting: RwSignal<Option<u32>>,
    menu: RwSignal<Option<i64>>,
    /// Which one shows all its reactions, until a click anywhere else.
    every: RwSignal<Option<i64>>,
}

/// Whether the screen is touched rather than pointed at with a mouse.
fn by_touch() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Reacts to a message, once with each emoji: counted among the most used,
/// and gone out as a message that answers it.
fn react_to(live: Live, peer: Peer, packet: u32, emoji: String) {
    // One of each: the same again says nothing new.
    let given = live.thread.with_untracked(|t| {
        t.iter()
            .any(|m| m.outgoing && m.emoji && m.reply_id == Some(packet) && m.text == emoji)
    });
    if given {
        return;
    }
    spawn_local(async move {
        if let Ok(emojis) = daemon::use_emoji(&emoji).await {
            live.emojis.set(Some(emojis));
        }
        let _ = send(live, peer, emoji, Some(packet), true).await;
    });
}

/// Answers a message: said over the field, and into it, but not by touch,
/// where the keyboard would cover what is answered.
fn reply_to(thread: Thread, packet: u32) {
    thread.replying.set(Some(packet));
    if !by_touch()
        && let Some(field) = document()
            .query_selector(".composer textarea")
            .ok()
            .flatten()
            .and_then(|f| f.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = field.focus();
    }
}

/// Forgets a message, once sure: only here, as what went out on the mesh
/// cannot be taken back.
fn forget(live: Live, i18n: I18nContext<Locale>, id: MessageId) {
    let sure = web_sys::window()
        .and_then(|w| {
            w.confirm_with_message(t_string!(i18n, chat_delete_confirm))
                .ok()
        })
        .unwrap_or(false);
    let Some(radio) = live.selected.get_untracked() else {
        return;
    };
    if sure {
        spawn_local(async move {
            if daemon::forget_message(radio, id).await.is_ok() {
                live.thread.update(|t| t.retain(|m| m.id != id));
            }
        });
    }
}

/// Whether the radio has replies and reactions: Meshtastic does, MeshCore
/// does not.
fn answerable(live: Live) -> bool {
    live.selected_kind() == Some(RadioKind::Meshtastic)
}

/// Sends a message, and puts it in the open conversation as sent.
async fn send(
    live: Live,
    peer: Peer,
    text: String,
    reply_id: Option<u32>,
    emoji: bool,
) -> Result<(), Failure> {
    let radio = live.selected.get_untracked().ok_or(Failure::Unreachable)?;
    let (channel, node) = match peer {
        Peer::Channel { channel } => (Some(channel), None),
        Peer::Node { node } => (None, Some(node)),
    };
    let message = NewMessage {
        text,
        channel,
        node,
        reply_id,
        emoji,
    };
    let sent = daemon::send_message(radio, &message).await?;
    live.thread.update(|thread| {
        if !thread.iter().any(|m| m.id == sent.id) {
            thread.push(sent);
        }
    });
    Ok(())
}

/// Who wrote a message, as the conversation shows it.
fn author_of(live: Live, message: &Message) -> String {
    if message.outgoing {
        return live.status.with(|s| {
            s.as_ref()
                .and_then(|s| s.local.as_ref())
                .map_or_else(String::new, |l| node_name(live, l.num))
        });
    }
    match (&message.sender_name, message.from) {
        (Some(name), 0) => name.clone(),
        _ => node_name(live, message.from),
    }
}

/// The open conversation. Which ones there are is in the sidebar.
#[component]
pub fn ChatView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let list = NodeRef::<html::Div>::new();
    provide_context(Thread {
        replying: RwSignal::new(None),
        reacting: RwSignal::new(None),
        menu: RwSignal::new(None),
        every: RwSignal::new(None),
    });
    // A click anywhere else closes the reactions; another conversation
    // starts with nothing answered.
    let state = expect_context::<Thread>();
    let closing = window_event_listener(leptos::ev::click, move |_| {
        state.reacting.set(None);
        state.every.set(None);
    });
    on_cleanup(move || closing.remove());
    Effect::new(move |_| {
        live.open.track();
        state.replying.set(None);
        state.reacting.set(None);
        state.menu.set(None);
    });
    // A reaction goes under what it reacts to; one to a message not here,
    // as a message.
    let shown = move || {
        live.thread.with(|thread| {
            thread
                .iter()
                .filter(|m| {
                    !(m.emoji
                        && m.reply_id
                            .is_some_and(|to| thread.iter().any(|t| t.packet_id == to)))
                })
                .cloned()
                .collect::<Vec<_>>()
        })
    };

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
                            each=shown
                            key=|m| (m.id, m.status)
                            let:message
                        >
                            <Bubble message peer />
                        </For>
                    </div>
                    <Composer peer />
                    <MessageMenu peer />
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn Bubble(message: Message, peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
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
    // How far it went, as an icon: waiting, out, received, or not; its words
    // on hovering, and why it failed, when it did.
    let status = message.status;
    let error = message.error.clone();
    let mark = (status != MessageStatus::Received).then(|| {
        let (icon, class) = match status {
            MessageStatus::Sending => (CLOCK, "pending"),
            MessageStatus::Sent => (CHECK, "sent"),
            MessageStatus::Delivered => (CHECKS, "delivered"),
            _ => (ALERT, "failed"),
        };
        let words = move || {
            let words = match status {
                MessageStatus::Sending => t_string!(i18n, chat_sending),
                MessageStatus::Sent => t_string!(i18n, chat_sent),
                MessageStatus::Delivered => t_string!(i18n, chat_delivered),
                _ => t_string!(i18n, chat_failed),
            };
            match &error {
                Some(error) => format!("{words}: {error}"),
                None => words.to_owned(),
            }
        };
        view! {
            <span class=format!("mark {class}") title=words.clone() aria-label=words inner_html=icon></span>
        }
    });

    let thread = expect_context::<Thread>();
    let id = message.id;
    let packet = message.packet_id;
    // Replies and reactions refer to its packet: one it has, on a radio
    // that has them.
    let can_answer = packet != 0 && untrack(move || answerable(live));
    // What it answers, quoted: who, and the start of it.
    let quote = message.reply_id.filter(|_| !message.emoji).map(|to| {
        move || {
            let answered = live
                .thread
                .with(|t| t.iter().find(|m| m.packet_id == to).cloned());
            match answered {
                Some(answered) => view! {
                    <div class="quote">
                        <span class="who">{author_of(live, &answered)}</span>
                        <span class="what">{answered.text.clone()}</span>
                    </div>
                }
                .into_any(),
                None => view! {
                    <div class="quote">
                        <span class="what dim">{t!(i18n, chat_reply_unknown)}</span>
                    </div>
                }
                .into_any(),
            }
        }
    });
    let react = move |emoji: String| {
        thread.reacting.set(None);
        react_to(live, peer, packet, emoji);
    };
    // The reactions to it, each emoji once, the most given first, how many
    // and who on hovering; one more of the same at a touch, unless it is
    // ours already. A few, and the rest behind one more.
    let reactions = move || {
        let mut by: Vec<(String, Vec<String>, bool)> = Vec::new();
        live.thread.with(|t| {
            for m in t
                .iter()
                .filter(|m| packet != 0 && m.emoji && m.reply_id == Some(packet))
            {
                let who = author_of(live, m);
                match by.iter_mut().find(|(emoji, _, _)| *emoji == m.text) {
                    Some(seen) => {
                        seen.1.push(who);
                        seen.2 |= m.outgoing;
                    }
                    None => by.push((m.text.clone(), vec![who], m.outgoing)),
                }
            }
        });
        if by.is_empty() {
            return None;
        }
        by.sort_by_key(|(_, who, _)| std::cmp::Reverse(who.len()));
        let hidden = if thread.every.get() == Some(id) {
            0
        } else {
            by.len().saturating_sub(REACTIONS_SHOWN)
        };
        let shown = by.len() - hidden;
        let chips = by
            .into_iter()
            .take(shown)
            .map(|(emoji, who, ours)| {
                let count = who.len();
                let again = emoji.clone();
                view! {
                    <button
                        type="button"
                        class="reaction"
                        class:ours=ours
                        title=who.join(", ")
                        disabled=ours || !can_answer
                        on:click=move |ev| {
                            ev.stop_propagation();
                            react(again.clone());
                        }
                    >
                        {emoji}
                        {(count > 1).then(|| view! { <span class="count">{count}</span> })}
                    </button>
                }
            })
            .collect_view();
        let more = (hidden > 0).then(|| {
            view! {
                <button
                    type="button"
                    class="reaction more"
                    on:click=move |ev| {
                        ev.stop_propagation();
                        thread.every.set(Some(id));
                    }
                >
                    {format!("+{hidden}")}
                </button>
            }
        });
        Some(view! { <div class="reactions">{chips} {more}</div> })
    };
    // With a mouse, what can be done with it, at its foot on the right, on
    // hovering: answering it, reacting to it, forgetting it. By touch, its
    // menu instead.
    let actions = view! {
        <div class="answer" on:click=|ev| ev.stop_propagation()>
            {can_answer
                .then(|| {
                    view! {
                        <button
                            type="button"
                            class="icon-button"
                            title=move || t_string!(i18n, chat_reply)
                            aria-label=move || t_string!(i18n, chat_reply)
                            inner_html=REPLY
                            on:click=move |_| reply_to(thread, packet)
                        ></button>
                        <button
                            type="button"
                            class="icon-button"
                            class:on=move || thread.reacting.get() == Some(packet)
                            title=move || t_string!(i18n, chat_react)
                            aria-label=move || t_string!(i18n, chat_react)
                            inner_html=SMILE
                            on:click=move |_| {
                                thread.reacting.update(|r| {
                                    *r = if *r == Some(packet) { None } else { Some(packet) };
                                });
                            }
                        ></button>
                    }
                })}
            <button
                type="button"
                class="icon-button"
                title=move || t_string!(i18n, chat_delete)
                aria-label=move || t_string!(i18n, chat_delete)
                inner_html=TRASH
                on:click=move |_| forget(live, i18n, id)
            ></button>
        </div>
    };
    // With reactions, what can be done with it goes up to its top: at its
    // foot it would cover them.
    let reacted = move || {
        packet != 0
            && live
                .thread
                .with(|t| t.iter().any(|m| m.emoji && m.reply_id == Some(packet)))
    };
    let picker = move || {
        (thread.reacting.get() == Some(packet))
            .then(|| view! { <ReactionPicker pick=Callback::new(react) /> })
    };

    view! {
        <div
            class="bubble-row"
            class:mine=mine
            class:reacting=move || thread.reacting.get() == Some(packet) && packet != 0
            class:reacted=reacted
        >
            <div
                class=if mine { "bubble mine" } else { "bubble" }
                class:held=move || thread.menu.get() == Some(id)
                class:reacted=reacted
                on:click=move |ev| {
                    if by_touch() {
                        ev.stop_propagation();
                        thread.menu.set(Some(id));
                        // Another one's reactions, all shown, fold.
                        if thread.every.get_untracked() != Some(id) {
                            thread.every.set(None);
                        }
                    }
                }
            >
                {author}
                {quote}
                <p>{message.text.clone()}</p>
                <span class="meta dim">
                    {move || ago(live.now.get(), at)}
                    {reception.map(|r| format!(" · {r}"))}
                    {mark}
                </span>
                {reactions}
                {actions}
            </div>
            {picker}
        </div>
    }
}

/// By touch, what can be done with a message, from below: reacting to it,
/// answering it, copying it, forgetting it.
#[component]
fn MessageMenu(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let thread = expect_context::<Thread>();
    let close = move || thread.menu.set(None);
    move || {
        let id = thread.menu.get()?;
        let message = live
            .thread
            .with_untracked(|t| t.iter().find(|m| m.id == id).cloned())?;
        let packet = message.packet_id;
        let can_answer = packet != 0 && answerable(live);
        let text = message.text.clone();
        Some(view! {
            <div class="sheet-back" on:click=move |_| close()>
                <div class="message-menu" role="menu" on:click=|ev| ev.stop_propagation()>
                    {can_answer
                        .then(|| {
                            view! {
                                <ReactionPicker pick=Callback::new(move |emoji: String| {
                                    close();
                                    react_to(live, peer, packet, emoji);
                                }) />
                            }
                        })}
                    {can_answer
                        .then(|| {
                            view! {
                                <button role="menuitem" on:click=move |_| {
                                    close();
                                    reply_to(thread, packet);
                                }>
                                    <span class="icon" inner_html=REPLY></span>
                                    {t!(i18n, chat_reply)}
                                </button>
                            }
                        })}
                    <button role="menuitem" on:click=move |_| {
                        close();
                        crate::clipboard::copy(&text);
                    }>
                        <span class="icon" inner_html=COPY></span>
                        {t!(i18n, chat_copy)}
                    </button>
                    <button role="menuitem" class="danger-item" on:click=move |_| {
                        close();
                        forget(live, i18n, id);
                    }>
                        <span class="icon" inner_html=TRASH></span>
                        {t!(i18n, chat_delete)}
                    </button>
                </div>
            </div>
        })
    }
}

/// The emojis to react with: the most used, at a touch, and all of them
/// behind the last button.
#[component]
fn ReactionPicker(pick: Callback<String>) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let all = RwSignal::new(false);
    if live.emojis.get_untracked().is_none() {
        spawn_local(async move {
            if let Ok(emojis) = daemon::emojis().await {
                live.emojis.set(Some(emojis));
            }
        });
    }
    let offered = move || {
        let mut offered = live.emojis.get().unwrap_or_default();
        for common in crate::emoji::COMMON {
            if !offered.iter().any(|e| e == common) {
                offered.push(common.to_owned());
            }
        }
        offered
    };
    view! {
        <div class="reaction-picker" class:all=move || all.get() on:click=|ev| ev.stop_propagation()>
            {move || {
                if all.get() {
                    let frequent = Signal::derive(offered);
                    view! {
                        <crate::emoji::EmojiPicker
                            picked=Signal::stored(None::<String>)
                            pick=pick
                            frequent=frequent
                        />
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="quick">
                            {offered()
                                .into_iter()
                                .filter(|emoji| crate::emoji::draws(emoji))
                                .take(REACTIONS_OFFERED)
                                .map(|emoji| {
                                    let picked = emoji.clone();
                                    view! {
                                        <button type="button" on:click=move |_| pick.run(picked.clone())>
                                            {emoji}
                                        </button>
                                    }
                                })
                                .collect_view()}
                            <button
                                type="button"
                                class="more"
                                title=move || t_string!(i18n, chat_more_emojis)
                                aria-label=move || t_string!(i18n, chat_more_emojis)
                                inner_html=PLUS
                                on:click=move |_| all.set(true)
                            ></button>
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn Composer(peer: Peer) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let thread = expect_context::<Thread>();
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
            // The emojis first, if open; what is answered, if not.
            if picking.get_untracked() {
                picking.set(false);
            } else {
                thread.replying.set(None);
            }
        }
    });
    on_cleanup(move || {
        closing.remove();
        escaping.remove();
    });
    // The most used, as they were when the emojis opened: what is picked
    // does not move under the finger, and comes forward the next time. The
    // common ones after, as many as make four rows.
    let frequent = RwSignal::new(Vec::<String>::new());
    let offer = move || {
        let mut offered = live.emojis.get_untracked().unwrap_or_default();
        for common in crate::emoji::COMMON {
            if offered.len() >= FREQUENT_SHOWN {
                break;
            }
            if !offered.iter().any(|e| e == common) {
                offered.push(common.to_owned());
            }
        }
        offered.truncate(FREQUENT_SHOWN);
        frequent.set(offered);
    };
    Effect::new(move |opened: Option<bool>| {
        let open = picking.get();
        if open && opened != Some(true) {
            offer();
            if live.emojis.get_untracked().is_none() {
                spawn_local(async move {
                    if let Ok(emojis) = daemon::emojis().await {
                        live.emojis.set(Some(emojis));
                        offer();
                    }
                });
            }
        }
        open
    });
    let insert = move |emoji: String| {
        let counted = emoji.clone();
        spawn_local(async move {
            if let Ok(emojis) = daemon::use_emoji(&counted).await {
                live.emojis.set(Some(emojis));
            }
        });
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
        if !can_send() {
            return;
        }
        let reply_id = thread.replying.get_untracked();
        let written = text.get_untracked();
        busy.set(true);
        spawn_local(async move {
            match send(live, peer, written, reply_id, false).await {
                Ok(()) => {
                    text.set(String::new());
                    picking.set(false);
                    error.set(None);
                    thread.replying.set(None);
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };
    // What is answered, over the field: who, the start of it, and a way out.
    let replying = move || {
        let to = thread.replying.get()?;
        let answered = live
            .thread
            .with(|t| t.iter().find(|m| m.packet_id == to).cloned())?;
        let who = author_of(live, &answered);
        Some(view! {
            <div class="replying">
                <span class="icon" inner_html=REPLY></span>
                <div class="quote">
                    <span class="who">
                        {move || t_string!(i18n, chat_replying).replace("{name}", &who)}
                    </span>
                    <span class="what">{answered.text.clone()}</span>
                </div>
                <button
                    type="button"
                    class="icon-button"
                    title=move || t_string!(i18n, chat_reply_cancel)
                    aria-label=move || t_string!(i18n, chat_reply_cancel)
                    inner_html=CLOSE
                    on:click=move |_| thread.replying.set(None)
                ></button>
            </div>
        })
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
            {replying}
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
                    <crate::emoji::EmojiPicker
                        picked=Signal::stored(None::<String>)
                        pick=Callback::new(insert)
                        frequent=Signal::from(frequent)
                    />
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
