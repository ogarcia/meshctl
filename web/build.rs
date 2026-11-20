// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

//! Turns the catalogs in `locales/` into the `i18n` module, so that using a key
//! the default language does not declare is a compile error, and one another
//! language lacks is a warning.

use leptos_i18n_build::{Config, TranslationsInfos};
use std::path::PathBuf;

/// The language the interface is written in, and falls back to.
const DEFAULT: &str = "en";

/// Every other language there is a catalog for.
const OTHERS: &[&str] = &["es"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=build.rs");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("no OUT_DIR")?).join("i18n");

    let config = OTHERS
        .iter()
        .try_fold(Config::new(DEFAULT)?, |config, code| {
            config.add_locale(code)
        })?;

    let translations = TranslationsInfos::parse(config)?;
    translations.emit_diagnostics();
    translations.rerun_if_locales_changed();
    translations.generate_i18n_module(out)?;

    Ok(())
}
