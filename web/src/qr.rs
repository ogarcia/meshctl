// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Reading a QR code: from the camera, live, where the browser lets a page
//! have it (over HTTPS, or on this very machine); or from a photo, taken or
//! picked, anywhere. Read here, in Rust, with no browser API that only some
//! browsers have.

use crate::i18n::{t, t_string, use_i18n};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

/// A photo: Tabler's camera.
const PHOTO: &str = include_str!("../vendor/tabler-icons-3.51.0/icons/outline/camera.svg");

/// How often a frame of the camera is looked at.
const EVERY: Duration = Duration::from_millis(250);

/// The widest a frame of the camera, or a photo, is looked at, in pixels:
/// enough for a QR that fills a fair part of it, and quick to read.
const FRAME_WIDTH: u32 = 640;
const PHOTO_WIDTH: u32 = 1280;

/// The text of the first QR code in an image, by its pixels, four bytes each
/// (red, green, blue and alpha): none when it holds none that reads.
pub fn decode(width: usize, height: usize, rgba: &[u8]) -> Option<String> {
    if rgba.len() < width * height * 4 {
        return None;
    }
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        let at = (y * width + x) * 4;
        let (r, g, b) = (
            u32::from(rgba[at]),
            u32::from(rgba[at + 1]),
            u32::from(rgba[at + 2]),
        );
        u8::try_from((r * 299 + g * 587 + b * 114) / 1000).unwrap_or(u8::MAX)
    });
    image
        .detect_grids()
        .into_iter()
        .find_map(|grid| grid.decode().ok().map(|(_, text)| text))
}

/// What a frame or a photo is drawn from.
enum Source<'a> {
    Video(&'a web_sys::HtmlVideoElement),
    Image(&'a web_sys::HtmlImageElement),
}

/// Draws a source no wider than `widest`, its shape kept, and reads the QR
/// in it, if any.
fn read(source: &Source<'_>, width: u32, height: u32, widest: u32) -> Option<String> {
    if width == 0 || height == 0 {
        return None;
    }
    let scale = f64::from(widest.min(width)) / f64::from(width);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (w, h) = (
        (f64::from(width) * scale).round() as u32,
        (f64::from(height) * scale).round() as u32,
    );
    let canvas: web_sys::HtmlCanvasElement =
        document().create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(w);
    canvas.set_height(h);
    let options = web_sys::ContextAttributes2d::new();
    options.set_will_read_frequently(true);
    let context: web_sys::CanvasRenderingContext2d = canvas
        .get_context_with_context_options("2d", &options)
        .ok()??
        .dyn_into()
        .ok()?;
    let (dw, dh) = (f64::from(w), f64::from(h));
    match source {
        Source::Video(video) => context
            .draw_image_with_html_video_element_and_dw_and_dh(video, 0.0, 0.0, dw, dh)
            .ok()?,
        Source::Image(image) => context
            .draw_image_with_html_image_element_and_dw_and_dh(image, 0.0, 0.0, dw, dh)
            .ok()?,
    }
    let pixels = context.get_image_data(0.0, 0.0, dw, dh).ok()?.data();
    decode(w as usize, h as usize, &pixels)
}

/// How the camera is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Camera {
    /// Asked for, not yet given.
    Starting,
    /// Shown, and looked at.
    On,
    /// None to be had here: no HTTPS, or none at all.
    Unavailable,
    /// Not let have it.
    Denied,
}

/// The camera's stream, to stop it once done.
fn stop_stream(stream: &web_sys::MediaStream) {
    for track in stream.get_tracks().iter() {
        if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
            track.stop();
        }
    }
}

/// Reads a QR code, and hands its text to `found`: from the camera, live,
/// the back one where there are two, when the browser lets the page have
/// it; or from a photo, taken or picked. The camera stops once it is read,
/// or once this goes.
#[component]
pub fn QrScanner(found: Callback<String>) -> impl IntoView {
    let i18n = use_i18n();
    let video = NodeRef::<html::Video>::new();
    let picker = NodeRef::<html::Input>::new();
    let camera = RwSignal::new(Camera::Starting);
    let photo_problem = RwSignal::new(false);
    let stream = StoredValue::new_local(None::<web_sys::MediaStream>);
    let done = StoredValue::new(false);
    let finish = move |text: String| {
        if done.get_value() {
            return;
        }
        done.set_value(true);
        if let Some(stream) = stream.get_value() {
            stop_stream(&stream);
        }
        found.run(text);
    };

    // The camera, where there is one to be had: only in a secure context.
    let media = window()
        .is_secure_context()
        .then(|| window().navigator().media_devices().ok())
        .flatten();
    match media {
        None => camera.set(Camera::Unavailable),
        Some(media) => {
            let constraints = web_sys::MediaStreamConstraints::new();
            let wanted = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&wanted, &"facingMode".into(), &"environment".into());
            constraints.set_video(&wanted);
            constraints.set_audio(&false.into());
            spawn_local(async move {
                let Ok(asked) = media.get_user_media_with_constraints(&constraints) else {
                    camera.try_set(Camera::Unavailable);
                    return;
                };
                match JsFuture::from(asked).await {
                    Ok(given) => {
                        let given: web_sys::MediaStream = given.unchecked_into();
                        // Gone, or read, before it came: let it go at once.
                        let gone = done.try_get_value().unwrap_or(true);
                        let Some(element) = video.try_get_untracked().flatten().filter(|_| !gone)
                        else {
                            stop_stream(&given);
                            return;
                        };
                        element.set_src_object(Some(&given));
                        let _ = element.play();
                        let _ = stream.try_set_value(Some(given));
                        camera.try_set(Camera::On);
                    }
                    Err(_) => {
                        camera.try_set(Camera::Denied);
                    }
                }
            });
        }
    }
    // A frame looked at now and then, while it shows.
    if let Ok(every) = set_interval_with_handle(
        move || {
            if camera.get_untracked() != Camera::On || done.get_value() {
                return;
            }
            let Some(element) = video.get_untracked() else {
                return;
            };
            // Not before it has a frame to show.
            if element.ready_state() < 2 {
                return;
            }
            let (w, h) = (element.video_width(), element.video_height());
            if let Some(text) = read(&Source::Video(&element), w, h, FRAME_WIDTH) {
                finish(text);
            }
        },
        EVERY,
    ) {
        on_cleanup(move || every.clear());
    }
    on_cleanup(move || {
        if let Some(stream) = stream.get_value() {
            stop_stream(&stream);
        }
    });

    // A photo, taken or picked: read once loaded.
    let picked = move |_| {
        photo_problem.set(false);
        let Some(input) = picker.get_untracked() else {
            return;
        };
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            return;
        };
        input.set_value("");
        let Ok(url) = web_sys::Url::create_object_url_with_blob(&file) else {
            return;
        };
        let Ok(image) = web_sys::HtmlImageElement::new() else {
            return;
        };
        let loaded = image.clone();
        let source = url.clone();
        let onload = Closure::once_into_js(move || {
            let _ = web_sys::Url::revoke_object_url(&source);
            let (w, h) = (loaded.natural_width(), loaded.natural_height());
            match read(&Source::Image(&loaded), w, h, PHOTO_WIDTH) {
                Some(text) => finish(text),
                None => photo_problem.set(true),
            }
        });
        image.set_onload(Some(onload.unchecked_ref()));
        image.set_src(&url);
    };

    let pick = move |_| {
        if let Some(input) = picker.get_untracked() {
            input.click();
        }
    };
    let no_camera = move || matches!(camera.get(), Camera::Unavailable | Camera::Denied);
    view! {
        <div class="qr-scanner">
            <input
                node_ref=picker
                style="display: none"
                type="file"
                accept="image/*"
                capture="environment"
                on:change=picked
            />
            <div class="qr-view" class:gone=no_camera>
                <video node_ref=video autoplay=true muted=true playsinline=true></video>
                <span class="qr-frame" aria-hidden="true"></span>
                {move || {
                    (camera.get() == Camera::Starting)
                        .then(|| view! { <span class="qr-starting">{t!(i18n, qr_starting)}</span> })
                }}
            </div>
            // With no camera, its place is the photo's.
            {move || {
                no_camera()
                    .then(|| {
                        view! {
                            <button type="button" class="qr-view qr-pick" on:click=pick>
                                <span class="icon" inner_html=PHOTO></span>
                                <span>{t!(i18n, qr_photo_pick)}</span>
                                <span class="dim">
                                    {move || match camera.get() {
                                        Camera::Denied => t_string!(i18n, qr_denied),
                                        _ => t_string!(i18n, qr_no_camera),
                                    }}
                                </span>
                            </button>
                        }
                    })
            }}
            {move || {
                (!no_camera())
                    .then(|| {
                        view! {
                            <p class="hint dim">
                                {t!(i18n, qr_camera_hint)} ", "
                                <button type="button" class="link" on:click=pick>
                                    {t!(i18n, qr_photo)}
                                </button>
                            </p>
                        }
                    })
            }}
            {move || photo_problem.get().then(|| view! { <p class="error">{t!(i18n, qr_not_found)}</p> })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::decode;

    /// A QR drawn as pixels, each module four pixels square, black on white
    /// with its quiet zone.
    fn drawn(text: &str) -> (usize, usize, Vec<u8>) {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let modules = code.width();
        let colors = code.to_colors();
        let (scale, quiet) = (4, 4);
        let side = (modules + 2 * quiet) * scale;
        let mut rgba = vec![255u8; side * side * 4];
        for y in 0..modules {
            for x in 0..modules {
                if colors[y * modules + x] == qrcode::Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let at =
                                (((y + quiet) * scale + dy) * side + (x + quiet) * scale + dx) * 4;
                            rgba[at..at + 3].copy_from_slice(&[0, 0, 0]);
                        }
                    }
                }
            }
        }
        (side, side, rgba)
    }

    #[test]
    fn a_link_drawn_as_a_qr_reads_back() {
        let link = "meshcore://contact/add?name=Ana&public_key=41484f56";
        let (w, h, rgba) = drawn(link);
        assert_eq!(decode(w, h, &rgba).as_deref(), Some(link));
        // Nothing but white: none.
        assert_eq!(decode(w, h, &vec![255; w * h * 4]), None);
    }
}
