// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use std::process::Command;

fn main() {
    // The migrations are compiled in by `sqlx::migrate!`, which cannot tell
    // Cargo that a changed or new file in the directory is a reason to build
    // again. This does.
    println!("cargo:rerun-if-changed=migrations");

    // Which commit this was built from, short, for /version and the about
    // window: from the environment when it is set, as a build that copies
    // the source without .git can do; from git when the source is a
    // checkout; none otherwise.
    let revision = std::env::var("MESHCTL_GIT_REVISION")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=7", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=MESHCTL_GIT_REVISION={revision}");
    println!("cargo:rerun-if-env-changed=MESHCTL_GIT_REVISION");
    // A new commit moves HEAD, or the branch HEAD points to.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");
}
