// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! The radios: their list, in the rail's order, and the editor of each one,
//! its button included.

use crate::app::{LinkLabel, Live, link_class};
use crate::badge::{self, Face, PALETTE, initials};
use crate::daemon::{self, Failure};
use crate::emoji::EmojiPicker;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{
    Badge, BadgeContent, BadgeIcon, ErrorCode, Radio, RadioId, RadioKind, RadioSettings,
};

const GRIP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/grip-vertical.svg");
const UP: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-up.svg");
const DOWN: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/chevron-down.svg");
const EYE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye.svg");
const CLOSE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/x.svg");
const EYE_OFF: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye-off.svg");

/// What the radios' editor is open on, if anything, and in which of its
/// sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    Closed,
    Adding,
    Radio(RadioId, Section),
}

/// The editor's two sections: what the radio is and how meshctl reaches it,
/// and how its button looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Connection,
    Button,
}

/// What a radio is set to, as it is changed.
fn settings_of(radio: &Radio) -> RadioSettings {
    RadioSettings {
        name: radio.name.clone(),
        kind: radio.kind,
        address: radio.address.clone(),
        enabled: radio.enabled,
        shown: radio.shown,
        badge: radio.badge.clone(),
    }
}

fn kind_name(kind: RadioKind) -> &'static str {
    match kind {
        RadioKind::Meshtastic => "Meshtastic",
        RadioKind::Meshcore => "MeshCore",
    }
}

/// Changes one thing of a radio at once, from its row.
fn quick(live: Live, radio: &Radio, change: impl FnOnce(&mut RadioSettings)) {
    let id = radio.id;
    let mut settings = settings_of(radio);
    change(&mut settings);
    spawn_local(async move {
        if daemon::save_radio(Some(id), &settings).await.is_ok() {
            let _ = live.refresh_radios().await;
        }
    });
}

/// Puts the radio at `from` where the one at `to` is, here at once and then
/// in the daemon.
fn reorder(live: Live, from: usize, to: usize) {
    let Some(mut radios) = live.radios.get_untracked() else {
        return;
    };
    if from == to || from >= radios.len() || to >= radios.len() {
        return;
    }
    let moved = radios.remove(from);
    radios.insert(to, moved);
    let order: Vec<RadioId> = radios.iter().map(|r| r.id).collect();
    live.radios.set(Some(radios));
    spawn_local(async move {
        let _ = daemon::order_radios(&order).await;
        let _ = live.refresh_radios().await;
    });
}

#[component]
pub fn RadiosView() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let radios = move || {
        let radios = live.radios.get().unwrap_or_default();
        let count = radios.len();
        radios
            .into_iter()
            .enumerate()
            .map(|(at, radio)| (at, count, radio))
            .collect::<Vec<_>>()
    };
    // The row being dragged, by its place.
    let dragging = RwSignal::new(None::<usize>);

    view! {
        <section class="radios" aria-label=move || t_string!(i18n, radios_title)>
            <For
                each=radios
                key=|(at, count, radio)| (*at, *count, radio.clone())
                let:((at, count, radio))
            >
                <RadioRow radio at count dragging />
            </For>
            <p class="hint dim">{t!(i18n, radios_hint)}</p>
        </section>
        {move || {
            // Over the list, on a radio picked or on one to add.
            match live.radio_editing.get() {
                Editing::Closed => ().into_any(),
                Editing::Adding => view! { <RadioEditor id=None modal=true /> }.into_any(),
                Editing::Radio(id, section) => {
                    view! { <RadioEditor id=Some(id) modal=true section /> }.into_any()
                }
            }
        }}
    }
}

#[component]
fn RadioRow(
    radio: Radio,
    at: usize,
    count: usize,
    dragging: RwSignal<Option<usize>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let id = radio.id;
    let link = radio.link;
    let selected = move || matches!(live.radio_editing.get(), Editing::Radio(at, _) if at == id);
    let over = RwSignal::new(false);

    let flip = {
        let radio = radio.clone();
        move |_| quick(live, &radio, |s| s.shown = !s.shown)
    };
    let toggle = {
        let radio = radio.clone();
        move |_| quick(live, &radio, |s| s.enabled = !s.enabled)
    };
    let eye_label = move || {
        if radio.shown {
            t_string!(i18n, radio_hide)
        } else {
            t_string!(i18n, radio_show)
        }
    };
    let detail = format!("{} · {}", kind_name(radio.kind), radio.address);
    let name = radio.name.clone();
    let badge = radio.badge.clone();

    view! {
        <div
            class="radio-row"
            class:selected=selected
            class:hidden=!radio.shown
            class:dragging=move || dragging.get() == Some(at)
            class:over=move || over.get()
            draggable="true"
            on:dragstart=move |ev| {
                if let Some(data) = ev.data_transfer() {
                    data.set_effect_allowed("move");
                    // Firefox starts no drag without data.
                    let _ = data.set_data("text/plain", &at.to_string());
                }
                dragging.set(Some(at));
            }
            on:dragover=move |ev| {
                ev.prevent_default();
                over.set(dragging.get_untracked().is_some_and(|from| from != at));
            }
            on:dragleave=move |_| over.set(false)
            on:drop=move |ev| {
                ev.prevent_default();
                over.set(false);
                if let Some(from) = dragging.get_untracked() {
                    reorder(live, from, at);
                }
                dragging.set(None);
            }
            on:dragend=move |_| dragging.set(None)
        >
            <span class="grip" title=move || t_string!(i18n, radio_drag) inner_html=GRIP></span>
            <span class="moves">
                <button
                    type="button"
                    disabled={at == 0}
                    title=move || t_string!(i18n, radio_up)
                    aria-label=move || t_string!(i18n, radio_up)
                    on:click=move |_| reorder(live, at, at.saturating_sub(1))
                    inner_html=UP
                ></button>
                <button
                    type="button"
                    disabled={at + 1 >= count}
                    title=move || t_string!(i18n, radio_down)
                    aria-label=move || t_string!(i18n, radio_down)
                    on:click=move |_| reorder(live, at, at + 1)
                    inner_html=DOWN
                ></button>
            </span>
            // Its button opens how it looks; the rest of it, the rest.
            <button
                type="button"
                class="face-button"
                title=move || t_string!(i18n, radio_button_edit)
                aria-label=move || t_string!(i18n, radio_button_edit)
                on:click=move |_| live.radio_editing.set(Editing::Radio(id, Section::Button))
            >
                <span class=badge::class(&badge) style=badge::style(&badge)>
                    <Face name=name.clone() badge=badge.clone() />
                </span>
            </button>
            <button
                type="button"
                class="pick"
                on:click=move |_| live.radio_editing.set(Editing::Radio(id, Section::Connection))
            >
                <span class="who">
                    <span class="name">
                        <b>{name.clone()}</b>
                        {(!radio.shown)
                            .then(|| view! { <span class="tag">{t!(i18n, radio_hidden)}</span> })}
                        <span class=format!("badge {}", link_class(link))>
                            <LinkLabel link=Signal::stored(link) />
                        </span>
                    </span>
                    <span class="mono dim">{detail}</span>
                </span>
            </button>
            <button
                type="button"
                class="eye"
                class:off=!radio.shown
                aria-pressed=radio.shown.to_string()
                title=eye_label
                aria-label=eye_label
                on:click=flip
                inner_html=if radio.shown { EYE } else { EYE_OFF }
            ></button>
            <Switch
                on=Signal::stored(radio.enabled)
                label=Signal::derive(move || t_string!(i18n, radio_enabled).to_string())
                flip=toggle
            />
        </div>
    }
}

/// On or off, for one thing.
#[component]
fn Switch(
    on: Signal<bool>,
    label: Signal<String>,
    flip: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            type="button"
            role="switch"
            class="switch"
            class:on=move || on.get()
            aria-checked=move || on.get().to_string()
            title=move || label.get()
            aria-label=move || label.get()
            on:click=flip
        >
            <span></span>
        </button>
    }
}

/// What a button shows, as it is picked: each kind of content keeps what was
/// last picked of it, to go back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Initials,
    Emoji,
    Icon,
}

#[derive(Debug, Clone, PartialEq)]
struct Look {
    mode: Mode,
    /// One or two characters; none for the initials.
    text: String,
    emoji: String,
    icon: BadgeIcon,
}

/// A radio's settings as the editor changes them.
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    name: String,
    kind: RadioKind,
    address: String,
    enabled: bool,
    shown: bool,
    color: Option<String>,
    look: Look,
}

impl Draft {
    fn new(settings: &RadioSettings) -> Self {
        let mut look = Look {
            mode: Mode::Initials,
            text: String::new(),
            emoji: "📡".into(),
            icon: match settings.kind {
                RadioKind::Meshtastic => BadgeIcon::Meshtastic,
                RadioKind::Meshcore => BadgeIcon::Meshcore,
            },
        };
        match &settings.badge.content {
            BadgeContent::Initials => {}
            BadgeContent::Text { text } => text.clone_into(&mut look.text),
            BadgeContent::Emoji { emoji } => {
                look.mode = Mode::Emoji;
                emoji.clone_into(&mut look.emoji);
            }
            BadgeContent::Icon { icon } => {
                look.mode = Mode::Icon;
                look.icon = *icon;
            }
        }
        Self {
            name: settings.name.clone(),
            kind: settings.kind,
            address: settings.address.clone(),
            enabled: settings.enabled,
            shown: settings.shown,
            // A colour of the palette, as it was once kept, by its name.
            color: settings.badge.color.as_deref().map(|color| {
                badge::palette_name(color).map_or_else(|| color.to_owned(), str::to_owned)
            }),
            look,
        }
    }

    fn fresh() -> Self {
        Self::new(&RadioSettings {
            name: String::new(),
            kind: RadioKind::Meshtastic,
            address: String::new(),
            enabled: true,
            shown: true,
            badge: Badge::default(),
        })
    }

    fn badge(&self) -> Badge {
        let content = match self.look.mode {
            Mode::Initials if self.look.text.trim().is_empty() => BadgeContent::Initials,
            Mode::Initials => BadgeContent::Text {
                text: self.look.text.trim().to_owned(),
            },
            Mode::Emoji => BadgeContent::Emoji {
                emoji: self.look.emoji.clone(),
            },
            Mode::Icon => BadgeContent::Icon {
                icon: self.look.icon,
            },
        };
        Badge {
            color: self.color.clone(),
            content,
        }
    }

    fn settings(&self) -> RadioSettings {
        RadioSettings {
            name: self.name.clone(),
            kind: self.kind,
            address: self.address.clone(),
            enabled: self.enabled,
            shown: self.shown,
            badge: self.badge(),
        }
    }

    /// Takes what changed elsewhere, from `old` to `new`, and keeps what was
    /// changed here.
    fn merge(&mut self, old: &Self, new: &Self) {
        macro_rules! take {
            ($($field:ident),*) => {
                $(if self.$field == old.$field {
                    self.$field.clone_from(&new.$field);
                })*
            };
        }
        take!(name, kind, address, enabled, shown, color);
        if self.badge() == old.badge() {
            self.look.clone_from(&new.look);
        }
    }
}

/// Changes a radio, or adds one when given none. As a `modal`, over the list,
/// it is closed once done with, saved or not; when not, it is the only thing
/// on its page, which is left by adding the radio.
#[component]
pub fn RadioEditor(
    id: Option<RadioId>,
    #[prop(optional)] modal: bool,
    #[prop(optional)] section: Section,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    // What the daemon has of it, which may change while it is edited.
    let stored = Memo::new(move |_| {
        let id = id?;
        live.radios.with(|radios| {
            radios
                .iter()
                .flatten()
                .find(|r| r.id == id)
                .map(settings_of)
        })
    });
    let start = stored
        .get_untracked()
        .map_or_else(Draft::fresh, |s| Draft::new(&s));
    let base = StoredValue::new(start.clone());
    let draft = RwSignal::new(start);
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    let section = RwSignal::new(section);
    let close = move || live.radio_editing.set(Editing::Closed);
    // Escape closes it, as its ✕ and Cancel do.
    if modal {
        let escape = window_event_listener(leptos::ev::keydown, move |ev| {
            if ev.key() == "Escape" {
                close();
            }
        });
        on_cleanup(move || escape.remove());
    }
    // A radio to add starts by its name.
    let name_input = NodeRef::<leptos::html::Input>::new();
    if id.is_none() && modal {
        Effect::new(move |_| {
            if let Some(input) = name_input.get() {
                let _ = input.focus();
            }
        });
    }

    Effect::new(move |_| {
        let Some(settings) = stored.get() else {
            return;
        };
        let new = Draft::new(&settings);
        let old = base.get_value();
        if new != old {
            draft.update(|draft| draft.merge(&old, &new));
            base.set_value(new);
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let settings = draft.with_untracked(Draft::settings);
        busy.set(true);
        spawn_local(async move {
            match daemon::save_radio(id, &settings).await {
                Ok(radio) => {
                    error.set(None);
                    let saved = Draft::new(&settings_of(&radio));
                    base.set_value(saved.clone());
                    draft.set(saved);
                    let _ = live.refresh_radios().await;
                    if modal {
                        close();
                    } else {
                        live.select(radio.id);
                    }
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };
    let remove = move |_| {
        let Some(id) = id else {
            return;
        };
        let sure = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(t_string!(i18n, radio_remove_confirm))
                    .ok()
            })
            .unwrap_or(false);
        if sure {
            spawn_local(async move {
                if daemon::remove_radio(id).await.is_ok() {
                    close();
                    let _ = live.refresh_radios().await;
                }
            });
        }
    };

    let message = move || {
        error.get().map(|failure| match failure {
            Failure::Rejected(ErrorCode::InvalidName) => t!(i18n, error_invalid_name).into_any(),
            Failure::Rejected(ErrorCode::InvalidAddress) => {
                t!(i18n, error_invalid_address).into_any()
            }
            Failure::Rejected(ErrorCode::InvalidBadge) => t!(i18n, error_invalid_badge).into_any(),
            Failure::Rejected(ErrorCode::UnsupportedKind) => {
                t!(i18n, error_unsupported_kind).into_any()
            }
            _ => t!(i18n, error_failed).into_any(),
        })
    };
    let title = move || match id {
        Some(_) => {
            let name = stored.get().map(|s| s.name).unwrap_or_default();
            t!(i18n, radio_edit_title, name = name).into_any()
        }
        None => t!(i18n, radio_add_title).into_any(),
    };
    let kinds = [RadioKind::Meshtastic, RadioKind::Meshcore].map(|kind| {
        let on = move || draft.with(|d| d.kind == kind);
        view! {
            <button
                type="button"
                role="radio"
                class:on=on
                aria-checked=move || on().to_string()
                on:click=move |_| draft.update(|d| d.kind = kind)
            >
                {kind_name(kind)}
            </button>
        }
    });
    let name = Signal::derive(move || draft.with(|d| d.name.clone()));
    let look = Signal::derive(move || draft.with(Draft::badge));
    let detail = move || {
        draft.with(|d| match d.address.trim() {
            "" => kind_name(d.kind).to_owned(),
            address => format!("{} · {address}", kind_name(d.kind)),
        })
    };
    let tabs = [Section::Connection, Section::Button].map(|tab| {
        let on = move || section.get() == tab;
        let label = move || match tab {
            Section::Connection => t!(i18n, radio_connection).into_any(),
            Section::Button => t!(i18n, radio_button).into_any(),
        };
        view! {
            <button
                type="button"
                role="tab"
                class:on=on
                aria-selected=move || on().to_string()
                on:click=move |_| section.set(tab)
            >
                {label}
            </button>
        }
    });
    let at = move || match section.get() {
        Section::Connection => 0,
        Section::Button => 1,
    };

    let form = view! {
        <form
            class="card radio-editor"
            class:modal=modal
            role=modal.then_some("dialog")
            aria-modal=modal.then_some("true")
            on:submit=submit
        >
            <header class="editor-head">
                <span
                    class=move || look.with(badge::class)
                    style=move || look.with(badge::style)
                >
                    <Face name badge=look />
                </span>
                <span class="who">
                    <h2>{title}</h2>
                    <span class="mono dim">{detail}</span>
                </span>
                {modal
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="close"
                                title=move || t_string!(i18n, radio_cancel)
                                aria-label=move || t_string!(i18n, radio_cancel)
                                on:click=move |_| close()
                                inner_html=CLOSE
                            ></button>
                        }
                    })}
            </header>
            <div class="segmented" role="tablist" style=move || format!("--at: {}; --n: 2", at())>
                {tabs.into_iter().collect_view()}
            </div>

            <div class="editor-body">
                <div class="fields" class:gone=move || section.get() != Section::Connection>
                    <label>
                        <span class="dim">{t!(i18n, radio_name)}</span>
                        <input
                            required
                            maxlength="64"
                            node_ref=name_input
                            placeholder=move || t_string!(i18n, radio_name_hint)
                            prop:value=move || draft.with(|d| d.name.clone())
                            on:input=move |ev| {
                                let name = event_target_value(&ev);
                                draft.update(|d| d.name = name);
                            }
                        />
                    </label>
                    <div class="field">
                        <span class="dim">{t!(i18n, radio_kind)}</span>
                        <div
                            class="segmented"
                            role="radiogroup"
                            style=move || {
                                let at = draft.with(|d| usize::from(d.kind == RadioKind::Meshcore));
                                format!("--n: 2; --at: {at}")
                            }
                        >
                            {kinds.into_iter().collect_view()}
                        </div>
                    </div>
                    <label>
                        <span class="dim">{t!(i18n, radio_address)}</span>
                        <input
                            required
                            class="mono"
                            placeholder="192.168.1.10 · /dev/ttyACM0"
                            prop:value=move || draft.with(|d| d.address.clone())
                            on:input=move |ev| {
                                let address = event_target_value(&ev);
                                draft.update(|d| d.address = address);
                            }
                        />
                        <span class="hint dim">{t!(i18n, radio_address_hint)}</span>
                    </label>
                    <div class="toggle">
                        <Switch
                            on=Signal::derive(move || draft.with(|d| d.enabled))
                            label=Signal::derive(move || t_string!(i18n, radio_enabled).to_string())
                            flip=move |_| draft.update(|d| d.enabled = !d.enabled)
                        />
                        <span>
                            {t!(i18n, radio_enabled)}
                            <span class="dim">{t!(i18n, radio_enabled_hint)}</span>
                        </span>
                    </div>
                    <div class="toggle">
                        <Switch
                            on=Signal::derive(move || draft.with(|d| d.shown))
                            label=Signal::derive(move || t_string!(i18n, radio_shown).to_string())
                            flip=move |_| draft.update(|d| d.shown = !d.shown)
                        />
                        <span>
                            {t!(i18n, radio_shown)}
                            <span class="dim">{t!(i18n, radio_shown_hint)}</span>
                        </span>
                    </div>
                </div>
                <div class="button-look" class:gone=move || section.get() != Section::Button>
                    <ColorPicker draft />
                    <ContentPicker draft />
                </div>
            </div>

            <p class="error">{message}</p>
            <div class="buttons">
                {id
                    .is_some()
                    .then(|| {
                        view! {
                            <button type="button" class="danger" on:click=remove>
                                {t!(i18n, radio_remove)}
                            </button>
                        }
                    })}
                {modal
                    .then(|| {
                        view! {
                            <button type="button" on:click=move |_| close()>
                                {t!(i18n, radio_cancel)}
                            </button>
                        }
                    })}
                <button type="submit" class="primary" disabled=move || busy.get()>
                    {move || match id {
                        Some(_) => t!(i18n, radio_save).into_any(),
                        None => t!(i18n, radio_add).into_any(),
                    }}
                </button>
            </div>
        </form>
    };

    if modal {
        // A click off it does not close it: it is too easy to make by mistake,
        // and what was changed would be lost.
        view! { <div class="modal-back">{form}</div> }.into_any()
    } else {
        form.into_any()
    }
}

/// The button's colour: the rail's own, one of the palette's, or any other.
#[component]
fn ColorPicker(draft: RwSignal<Draft>) -> impl IntoView {
    let i18n = use_i18n();
    let color = move || draft.with(|d| d.color.clone());
    let swatches = PALETTE.map(|name| {
        let on = move || color().as_deref() == Some(name);
        let label = move || badge::color_name(name);
        view! {
            <button
                type="button"
                role="radio"
                class="swatch split"
                class:on=on
                aria-checked=move || on().to_string()
                title=label
                aria-label=label
                style=format!(
                    "--swatch: var(--mc-badge-{name}-on); --swatch-rest: var(--mc-badge-{name})"
                )
                on:click=move |_| draft.update(|d| d.color = Some(name.to_owned()))
            ></button>
        }
    });
    let none = move || color().is_none();
    // Any other: the one it has when it is none of the palette's.
    let other = move || color().filter(|c| badge::palette_name(c).is_none());
    view! {
        <div class="field colors">
            <span class="dim">{t!(i18n, radio_color)}</span>
            <div class="swatches" role="radiogroup">
                <button
                    type="button"
                    role="radio"
                    class="swatch none"
                    class:on=none
                    aria-checked=move || none().to_string()
                    title=move || t_string!(i18n, radio_color_none)
                    aria-label=move || t_string!(i18n, radio_color_none)
                    on:click=move |_| draft.update(|d| d.color = None)
                ></button>
                {swatches.into_iter().collect_view()}
                // One more swatch: the colours' wheel, or the one picked there.
                <label
                    class="swatch other"
                    class:on=move || other().is_some()
                    class:split=move || other().is_some()
                    style=move || {
                        other()
                            .map(|c| {
                                format!(
                                    "--swatch: {c}; \
                                     --swatch-rest: color-mix(in srgb, {c} 50%, var(--mc-rail))"
                                )
                            })
                            .unwrap_or_default()
                    }
                    title=move || t_string!(i18n, radio_color_other)
                >
                    <input
                        type="color"
                        aria-label=move || t_string!(i18n, radio_color_other)
                        prop:value=move || other().unwrap_or_else(|| "#f0a63a".to_owned())
                        on:input=move |ev| {
                            let picked = event_target_value(&ev);
                            draft.update(|d| d.color = Some(picked));
                        }
                    />
                </label>
            </div>
        </div>
    }
}

/// What the button shows: initials or characters, an emoji, or an icon.
#[component]
fn ContentPicker(draft: RwSignal<Draft>) -> impl IntoView {
    let i18n = use_i18n();
    let mode = move || draft.with(|d| d.look.mode);
    let tabs = [Mode::Initials, Mode::Emoji, Mode::Icon].map(|tab| {
        let on = move || mode() == tab;
        let label = move || match tab {
            Mode::Initials => t!(i18n, radio_look_initials).into_any(),
            Mode::Emoji => t!(i18n, radio_look_emoji).into_any(),
            Mode::Icon => t!(i18n, radio_look_icon).into_any(),
        };
        view! {
            <button
                type="button"
                role="tab"
                class:on=on
                aria-selected=move || on().to_string()
                on:click=move |_| draft.update(|d| d.look.mode = tab)
            >
                {label}
            </button>
        }
    });
    let icons = BadgeIcon::ALL.map(|icon| {
        let on = move || draft.with(|d| d.look.mode == Mode::Icon && d.look.icon == icon);
        let name = badge::icon_name(icon);
        view! {
            <button
                type="button"
                class:on=on
                title=name.clone()
                aria-label=name
                on:click=move |_| {
                    draft.update(|d| {
                        d.look.icon = icon;
                        d.look.mode = Mode::Icon;
                    });
                }
                inner_html=badge::svg(icon)
            ></button>
        }
    });
    let icons = icons.into_iter().collect_view();
    let picked = Signal::derive(move || {
        draft.with(|d| (d.look.mode == Mode::Emoji).then(|| d.look.emoji.clone()))
    });
    let pick = Callback::new(move |emoji: String| {
        draft.update(|d| {
            d.look.emoji = emoji;
            d.look.mode = Mode::Emoji;
        });
    });

    // Where the pill stands, under the option picked.
    let at = move || match mode() {
        Mode::Initials => 0,
        Mode::Emoji => 1,
        Mode::Icon => 2,
    };

    // One panel, the same size for the three: picking another changes what is
    // in it, and nothing around it moves.
    view! {
        <div class="field contents">
            <span class="dim">{t!(i18n, radio_look)}</span>
            <div class="segmented" role="tablist" style=move || format!("--at: {}", at())>
                {tabs.into_iter().collect_view()}
            </div>
            <div class="look-panel">
            <div class="look-text" class:gone=move || mode() != Mode::Initials>
                <input
                    maxlength="2"
                    placeholder=move || draft.with(|d| initials(&d.name))
                    prop:value=move || draft.with(|d| d.look.text.clone())
                    on:input=move |ev| {
                        let text = event_target_value(&ev);
                        draft.update(|d| d.look.text = text);
                    }
                />
                <span class="dim">{t!(i18n, radio_look_text_hint)}</span>
            </div>
            <div class="look-emoji" class:gone=move || mode() != Mode::Emoji>
                <EmojiPicker picked pick start=7 />
            </div>
            <div class="icon-grid" class:gone=move || mode() != Mode::Icon>
                {icons}
            </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> RadioSettings {
        RadioSettings {
            name: "Casa".into(),
            kind: RadioKind::Meshtastic,
            address: "192.168.1.10".into(),
            enabled: true,
            shown: true,
            badge: Badge::default(),
        }
    }

    #[test]
    fn a_draft_gives_back_what_it_was_given() {
        let mut settings = home();
        for content in [
            BadgeContent::Initials,
            BadgeContent::Text { text: "TM".into() },
            BadgeContent::Emoji {
                emoji: "🧪".into()
            },
            BadgeContent::Icon {
                icon: BadgeIcon::Tent,
            },
        ] {
            settings.badge = Badge {
                color: Some("teal".into()),
                content,
            };
            assert_eq!(Draft::new(&settings).settings(), settings);
        }
        // A colour of the palette as it was once kept goes by its name.
        settings.badge.color = Some("#38D9A9".into());
        assert_eq!(Draft::new(&settings).color.as_deref(), Some("teal"));
        settings.badge.color = Some("#123456".into());
        assert_eq!(Draft::new(&settings).color.as_deref(), Some("#123456"));
        // Characters left blank are the initials.
        let mut draft = Draft::new(&home());
        draft.look.text = " ".into();
        assert_eq!(draft.badge().content, BadgeContent::Initials);
    }

    #[test]
    fn a_draft_takes_what_changed_elsewhere_and_keeps_its_own() {
        let old = Draft::new(&home());
        let mut draft = old.clone();
        draft.name = "Casa de campo".into();
        // Hidden from its row, while its name was being changed here.
        let mut elsewhere = home();
        elsewhere.shown = false;
        elsewhere.name = "Otra".into();
        let new = Draft::new(&elsewhere);
        draft.merge(&old, &new);
        assert!(!draft.shown);
        assert_eq!(draft.name, "Casa de campo");
    }
}
