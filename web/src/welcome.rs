//! What meshctl shows with no radios yet: the whole screen, without the
//! bars, which would have nothing to hold. Three steps: what meshctl is; the
//! radio, with only what it takes to reach it; and the link coming up, until
//! the radio is ready and the interface opens on it.

use crate::app::{Live, Page};
use crate::daemon::{self, Failure};
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use meshctl_api::{Badge, ErrorCode, LinkState, RadioId, RadioKind, RadioSettings};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Hello,
    Radio,
    Linking,
}

/// How meshctl reaches the radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Network,
    Usb,
}

/// How long the radio has to start answering before the welcome says it
/// does not, while meshctl keeps trying.
const PATIENCE: Duration = Duration::from_secs(25);

/// How long "ready" shows before the interface opens.
const READY_PAUSE: Duration = Duration::from_millis(1200);

/// `welcoming` holds the radio added here while its link comes up: until it
/// is ready, the welcome stays, though there is a radio already.
#[component]
pub fn Welcome(welcoming: RwSignal<Option<RadioId>>) -> impl IntoView {
    let step = RwSignal::new(if welcoming.get_untracked().is_some() {
        Step::Linking
    } else {
        Step::Hello
    });
    let kind = RwSignal::new(RadioKind::Meshtastic);
    let via = RwSignal::new(Via::Network);
    let address = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let form = Form {
        kind,
        via,
        address,
        name,
    };
    view! {
        <div class="welcome-screen">
            // Back, on a phone, up here beside the language: a row less
            // over the form, for a screen short of height.
            <div class="welcome-top">
                {move || (step.get() == Step::Radio).then(|| back(step, "welcome-back up"))}
                <Languages />
            </div>
            <div class="welcome-body">
                {move || match step.get() {
                    Step::Hello => view! { <Hello step /> }.into_any(),
                    Step::Radio => view! { <RadioStep step form welcoming /> }.into_any(),
                    Step::Linking => view! { <Linking step form welcoming /> }.into_any(),
                }}
            </div>
        </div>
    }
}

/// Back from the radio to what meshctl is: over the form, or at the top on
/// a phone (`up`), as the style sheet shows one or the other.
fn back(step: RwSignal<Step>, class: &'static str) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <button type="button" class=class on:click=move |_| step.set(Step::Hello)>
            <span class="icon" inner_html=ARROW_LEFT></span>
            {t!(i18n, welcome_back)}
        </button>
    }
}

/// What the radio step asks, kept while going back and forth.
#[derive(Clone, Copy)]
struct Form {
    kind: RwSignal<RadioKind>,
    via: RwSignal<Via>,
    address: RwSignal<String>,
    name: RwSignal<String>,
}

impl Form {
    /// The address as the daemon takes it: a device's path from its name
    /// alone, as `ttyACM0`.
    fn address(self) -> String {
        let address = self.address.get_untracked().trim().to_owned();
        match self.via.get_untracked() {
            Via::Usb if !address.is_empty() && !address.starts_with('/') => {
                format!("/dev/{address}")
            }
            _ => address,
        }
    }
}

/// The language, before anything else is read: the browser's, unless
/// another is picked, as in the preferences.
#[component]
fn Languages() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let value = move || crate::preferences::language_code(live.preferences.with(|p| p.language));
    let pick = move |ev: leptos::ev::Event| {
        let mut preferences = live.preferences.get_untracked();
        preferences.language = crate::preferences::language_of(&event_target_value(&ev));
        live.prefer(preferences);
    };
    view! {
        <label class="welcome-language">
            <span class="icon" inner_html=LANGUAGE></span>
            <select
                prop:value=value
                on:change=pick
                aria-label=move || t_string!(i18n, prefs_language)
            >
                {crate::preferences::language_options()}
            </select>
        </label>
    }
}

#[component]
fn Hello(step: RwSignal<Step>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <section class="welcome-hello">
            <svg class="welcome-logo" viewBox="0 0 24 24" role="img" aria-label="meshctl">
                <circle cx="5" cy="12" r="2.5"></circle>
                <circle cx="19" cy="5" r="2.5"></circle>
                <circle cx="19" cy="19" r="2.5"></circle>
                <path d="M7.3 11 16.7 6M7.3 13l9.4 5"></path>
            </svg>
            <h1>{t!(i18n, welcome_title)}</h1>
            <p class="welcome-lead">{t!(i18n, welcome_lead)}</p>
            <button class="primary welcome-go" on:click=move |_| step.set(Step::Radio)>
                {t!(i18n, welcome_start)}
                <span class="icon" inner_html=ARROW_RIGHT></span>
            </button>
            <p class="welcome-small">{t!(i18n, welcome_more)}</p>
        </section>
    }
}

/// One of [`cards`]: what it picks, its icon if any, its name and its line.
type Card<T> = (T, Option<&'static str>, Signal<String>, Signal<String>);

/// Two big choices side by side, as cards: a name, a line under it and,
/// maybe, an icon.
fn cards<T: Copy + PartialEq + Send + Sync + 'static>(
    value: RwSignal<T>,
    options: Vec<Card<T>>,
) -> impl IntoView {
    let buttons = options
        .into_iter()
        .map(|(this, icon, title, detail)| {
            let on = move || value.get() == this;
            view! {
                <button
                    type="button"
                    role="radio"
                    class="welcome-card"
                    class:on=on
                    aria-checked=move || on().to_string()
                    on:click=move |_| value.set(this)
                >
                    {icon.map(|icon| view! { <span class="icon" inner_html=icon></span> })}
                    <span class="title">{title}</span>
                    <span class="detail">{detail}</span>
                </button>
            }
        })
        .collect_view();
    view! { <div class="welcome-cards" role="radiogroup">{buttons}</div> }
}

#[component]
fn RadioStep(
    step: RwSignal<Step>,
    form: Form,
    welcoming: RwSignal<Option<RadioId>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let Form {
        kind,
        via,
        address,
        name,
    } = form;
    let error = RwSignal::new(None::<Failure>);
    let busy = RwSignal::new(false);
    // Into the address at once, but not by touch: the keyboard would cover
    // the choices above it before they are read.
    let input = NodeRef::<leptos::html::Input>::new();
    let touch = web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches());
    Effect::new(move |_| {
        if let Some(input) = input.get()
            && !touch
        {
            let _ = input.focus();
        }
    });

    let kinds = cards(
        kind,
        vec![
            (
                RadioKind::Meshtastic,
                None,
                Signal::derive(|| "Meshtastic".to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_meshtastic_detail).to_owned()),
            ),
            (
                RadioKind::Meshcore,
                None,
                Signal::derive(|| "MeshCore".to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_meshcore_detail).to_owned()),
            ),
        ],
    );
    let vias = cards(
        via,
        vec![
            (
                Via::Network,
                Some(WIFI),
                Signal::derive(move || t_string!(i18n, welcome_network).to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_network_detail).to_owned()),
            ),
            (
                Via::Usb,
                Some(USB),
                Signal::derive(move || t_string!(i18n, welcome_usb).to_owned()),
                Signal::derive(move || t_string!(i18n, welcome_usb_detail).to_owned()),
            ),
        ],
    );
    let hint = move || match via.get() {
        Via::Network => {
            let port = match kind.get() {
                RadioKind::Meshtastic => 4403,
                RadioKind::Meshcore => 5000,
            };
            t_string!(i18n, welcome_address_hint).replace("{port}", &port.to_string())
        }
        Via::Usb => t_string!(i18n, welcome_device_hint).to_owned(),
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let target = form.address();
        // With no name, the address for now: the radio's own once it says.
        let given = name.get_untracked().trim().to_owned();
        let settings = RadioSettings {
            name: if given.is_empty() {
                target.clone()
            } else {
                given
            },
            kind: kind.get_untracked(),
            address: target,
            enabled: true,
            shown: true,
            badge: Badge::default(),
        };
        busy.set(true);
        spawn_local(async move {
            // Checked again after a link that did not come up: the same radio.
            match daemon::save_radio(welcoming.get_untracked(), &settings).await {
                Ok(radio) => {
                    error.set(None);
                    welcoming.set(Some(radio.id));
                    let _ = live.refresh_radios().await;
                    live.select(radio.id);
                    step.set(Step::Linking);
                }
                Err(failure) => error.set(Some(failure)),
            }
            busy.set(false);
        });
    };
    let message = move || {
        error.get().map(|failure| {
            let text = match failure {
                Failure::Rejected(ErrorCode::InvalidName) => {
                    t!(i18n, error_invalid_name).into_any()
                }
                Failure::Rejected(ErrorCode::InvalidAddress) => {
                    t!(i18n, error_invalid_address).into_any()
                }
                Failure::Rejected(ErrorCode::UnsupportedKind) => {
                    t!(i18n, error_unsupported_kind).into_any()
                }
                _ => t!(i18n, error_failed).into_any(),
            };
            view! { <p class="error" role="alert">{text}</p> }
        })
    };

    view! {
        <form class="welcome-radio" on:submit=submit>
            {back(step, "welcome-back")}
            <h1>{t!(i18n, welcome_radio_title)}</h1>
            <fieldset>
                <legend>{t!(i18n, welcome_kind)}</legend>
                {kinds}
            </fieldset>
            <fieldset>
                <legend>{t!(i18n, welcome_via)}</legend>
                {vias}
            </fieldset>
            <label class="field">
                <span class="label">
                    {move || match via.get() {
                        Via::Network => t!(i18n, welcome_address).into_any(),
                        Via::Usb => t!(i18n, welcome_device).into_any(),
                    }}
                </span>
                <input
                    node_ref=input
                    class="mono"
                    required
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    placeholder=move || match via.get() {
                        Via::Network => "192.168.1.10",
                        Via::Usb => "/dev/ttyACM0",
                    }
                    prop:value=move || address.get()
                    on:input=move |ev| address.set(event_target_value(&ev))
                />
                <span class="hint">{hint}</span>
            </label>
            <label class="field">
                <span class="label">
                    {t!(i18n, welcome_name)}
                    <span class="optional">" · " {t!(i18n, welcome_name_optional)}</span>
                </span>
                <input
                    maxlength="40"
                    placeholder=move || t_string!(i18n, welcome_name_placeholder)
                    prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                />
                <span class="hint">{t!(i18n, welcome_name_hint)}</span>
            </label>
            {message}
            <button
                type="submit"
                class="primary welcome-go"
                disabled=move || busy.get() || address.with(|a| a.trim().is_empty())
            >
                {t!(i18n, welcome_connect)}
                <span class="icon" inner_html=ARROW_RIGHT></span>
            </button>
        </form>
    }
}

/// Where the link is, of the three things the welcome shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Connecting,
    Syncing,
    Ready,
}

#[component]
fn Linking(
    step: RwSignal<Step>,
    form: Form,
    welcoming: RwSignal<Option<RadioId>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let link = Memo::new(move |_| {
        let id = welcoming.get()?;
        live.radios
            .with(|radios| radios.iter().flatten().find(|r| r.id == id).map(|r| r.link))
    });
    let phase = Memo::new(move |_| match link.get() {
        Some(LinkState::Ready) => Phase::Ready,
        Some(LinkState::Syncing) => Phase::Syncing,
        _ => Phase::Connecting,
    });
    // Short of an answer: the link tried and dropped, or nothing for a
    // while. Another wait starts it all again, and the timers of the waits
    // before it say nothing.
    let failed = RwSignal::new(false);
    let tried = RwSignal::new(false);
    let wait = RwSignal::new(0_u32);
    let start_waiting = move || {
        failed.set(false);
        tried.set(false);
        let this = wait.get_untracked() + 1;
        wait.set(this);
        // By then the welcome may be gone, the radio in: none of this then.
        set_timeout(
            move || {
                if wait.try_get_untracked() == Some(this)
                    && phase.try_get_untracked() == Some(Phase::Connecting)
                {
                    failed.try_set(true);
                }
            },
            PATIENCE,
        );
    };
    start_waiting();
    Effect::new(move |_| match link.get() {
        Some(LinkState::Connecting | LinkState::Syncing) => tried.set(true),
        Some(LinkState::Disconnected) if tried.get_untracked() => failed.set(true),
        Some(LinkState::Ready) => failed.set(false),
        _ => {}
    });

    // Into the interface: on the map if the radio knows where any node is,
    // on the nodes if not. The radio's own name, if none was given.
    let enter = move || {
        // A moment after it was ready: the welcome may be gone by then.
        if form.name.is_disposed() {
            return;
        }
        let Some(id) = welcoming.try_get_untracked().flatten() else {
            return;
        };
        if form.name.get_untracked().trim().is_empty() {
            let own = live.status.with_untracked(|s| {
                let num = s.as_ref()?.local.as_ref()?.num;
                live.nodes.with_untracked(|nodes| {
                    nodes
                        .get(&num)
                        .and_then(|n| n.long_name.clone())
                        .filter(|n| !n.trim().is_empty())
                })
            });
            let radio = live
                .radios
                .with_untracked(|r| r.iter().flatten().find(|r| r.id == id).cloned());
            if let (Some(own), Some(radio)) = (own, radio) {
                let mut settings = crate::radios::settings_of(&radio);
                settings.name = own;
                spawn_local(async move {
                    if daemon::save_radio(Some(id), &settings).await.is_ok() {
                        let _ = live.refresh_radios().await;
                    }
                });
            }
        }
        let placed = live
            .nodes
            .with_untracked(|nodes| nodes.values().any(|n| n.position.is_some()));
        live.go(if placed { Page::Map } else { Page::Nodes });
        welcoming.set(None);
    };
    // Once: a link that drops and comes back while "ready" shows does not
    // open the interface twice.
    let entering = StoredValue::new(false);
    Effect::new(move |_| {
        if phase.get() == Phase::Ready && !entering.get_value() {
            entering.set_value(true);
            set_timeout(enter, READY_PAUSE);
        }
    });

    let nodes = move || {
        let n = live.nodes.with(|nodes| nodes.len());
        (n > 0).then(|| {
            view! {
                <span class="count">
                    {t_string!(i18n, welcome_nodes).replace("{n}", &n.to_string())}
                </span>
            }
        })
    };
    let item = move |this: Phase, label: AnyView, extra: Option<AnyView>| {
        let state = move || match phase.get().cmp(&this) {
            std::cmp::Ordering::Greater => "done",
            std::cmp::Ordering::Equal if this == Phase::Ready => "done",
            std::cmp::Ordering::Equal if failed.get() => "stuck",
            std::cmp::Ordering::Equal => "now",
            std::cmp::Ordering::Less => "later",
        };
        let icon = move || match state() {
            "done" => CIRCLE_CHECK,
            "stuck" => ALERT,
            "now" => LOADER,
            _ => "",
        };
        view! {
            <li class=state>
                <span class="icon" inner_html=icon></span>
                <span class="what">{label} {extra}</span>
            </li>
        }
    };
    let address = StoredValue::new(form.address());

    view! {
        <section class="welcome-linking" aria-live="polite">
            <h1>{t!(i18n, welcome_linking_title)}</h1>
            <p class="welcome-address mono">{address.get_value()}</p>
            <ol class="welcome-steps">
                {item(Phase::Connecting, t!(i18n, welcome_step_connect).into_any(), None)}
                {item(Phase::Syncing, t!(i18n, welcome_step_sync).into_any(), Some(nodes.into_any()))}
                {item(Phase::Ready, t!(i18n, welcome_step_ready).into_any(), None)}
            </ol>
            <Show when=move || failed.get()>
                <div class="welcome-failed" role="alert">
                    <p class="what">{move || t_string!(i18n, welcome_failed).replace("{address}", &address.get_value())}</p>
                    <p class="hint">{t!(i18n, welcome_failed_hint)}</p>
                    <div class="buttons">
                        <button class="primary" on:click=move |_| step.set(Step::Radio)>
                            {t!(i18n, welcome_check)}
                        </button>
                        <button on:click=move |_| start_waiting()>{t!(i18n, welcome_wait)}</button>
                    </div>
                    <button class="link" on:click=move |_| enter()>{t!(i18n, welcome_enter)}</button>
                </div>
            </Show>
        </section>
    }
}

const ARROW_RIGHT: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/arrow-right.svg");
const ARROW_LEFT: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/arrow-left.svg");
const WIFI: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/wifi.svg");
const USB: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/usb.svg");
const CIRCLE_CHECK: &str =
    include_str!("../vendor/tabler-icons-3.51.0/icons/outline/circle-check.svg");
const ALERT: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/alert-triangle.svg");
const LANGUAGE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/language.svg");
const LOADER: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/loader-2.svg");

const EYE: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye.svg");
const EYE_OFF: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/eye-off.svg");

/// What meshctl shows when it asks for its access token: the whole screen,
/// as the welcome, as nothing else can be read without it. The token typed,
/// shown or not, kept in this browser once it goes in; and why the last one
/// did not, when one was given.
#[component]
pub fn TokenScreen() -> impl IntoView {
    let i18n = use_i18n();
    let live = expect_context::<Live>();
    let value = RwSignal::new(String::new());
    let shown = RwSignal::new(false);
    let checking = RwSignal::new(false);
    let unreachable = RwSignal::new(false);
    // Tried here first, so that one refused says so in place: kept, and
    // the interface opened, only once it goes in.
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let token = value.get_untracked().trim().to_owned();
        if token.is_empty() || checking.get_untracked() {
            return;
        }
        checking.set(true);
        unreachable.set(false);
        spawn_local(async move {
            let taken = daemon::check_token(&token).await;
            checking.try_set(false);
            match taken {
                Ok(true) => {
                    daemon::set_token(&token);
                    live.token_refused.set(false);
                    live.daemon.set(crate::app::Daemon::Connecting);
                }
                Ok(false) => live.token_refused.set(true),
                Err(_) => {
                    unreachable.try_set(true);
                }
            }
        });
    };
    view! {
        <div class="welcome-screen">
            <div class="welcome-top"></div>
            <div class="welcome-body">
                <form class="welcome-hello token-screen" on:submit=submit>
                    <svg class="welcome-logo" viewBox="0 0 24 24" role="img" aria-label="meshctl">
                        <circle cx="5" cy="12" r="2.5"></circle>
                        <circle cx="19" cy="5" r="2.5"></circle>
                        <circle cx="19" cy="19" r="2.5"></circle>
                        <path d="M7.3 11 16.7 6M7.3 13l9.4 5"></path>
                    </svg>
                    <h1>{t!(i18n, token_title)}</h1>
                    <p class="welcome-lead">{t!(i18n, token_needed)}</p>
                    <div class="token-field">
                        <input
                            type=move || if shown.get() { "text" } else { "password" }
                            name="token"
                            autocomplete="current-password"
                            autocapitalize="off"
                            spellcheck="false"
                            required=true
                            placeholder=move || t_string!(i18n, token_placeholder)
                            aria-label=move || t_string!(i18n, token_placeholder)
                            prop:value=move || value.get()
                            on:input=move |ev| {
                                value.set(event_target_value(&ev));
                                live.token_refused.set(false);
                                unreachable.set(false);
                            }
                        />
                        <button
                            type="button"
                            class="icon-button"
                            title=move || {
                                if shown.get() { t_string!(i18n, token_hide) } else { t_string!(i18n, token_show) }
                            }
                            aria-label=move || {
                                if shown.get() { t_string!(i18n, token_hide) } else { t_string!(i18n, token_show) }
                            }
                            inner_html=move || if shown.get() { EYE_OFF } else { EYE }
                            on:click=move |_| shown.update(|s| *s = !*s)
                        ></button>
                    </div>
                    {move || {
                        live.token_refused
                            .get()
                            .then(|| view! { <p class="problem" role="alert">{t!(i18n, token_wrong)}</p> })
                    }}
                    {move || {
                        unreachable
                            .get()
                            .then(|| view! { <p class="problem" role="alert">{t!(i18n, token_unreachable)}</p> })
                    }}
                    // Held while it is tried, as it was: it takes a moment.
                    <button type="submit" class="primary welcome-go" disabled=move || checking.get()>
                        {t!(i18n, token_use)}
                        <span class="icon" inner_html=ARROW_RIGHT></span>
                    </button>
                    <p class="welcome-small">{t!(i18n, token_hint)}</p>
                </form>
            </div>
        </div>
    }
}
