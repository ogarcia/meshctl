#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>
#
# Builds the release binary the Flatpak's manifest packs, as CI does: the
# interface first, with trunk, which the daemon within embeds; then the
# desktop app. Then:
#
#   flatpak-builder --user --install --force-clean build-dir desktop/flatpak/me.ogarcia.meshctl.yml
#
# Needs Rust with the wasm32-unknown-unknown target, trunk, and WebKitGTK's
# development files (webkit2gtk-4.1) to link the desktop app.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

echo ">> The interface (trunk)"
(cd web && trunk build --release)

echo ">> The desktop app (release)"
cargo build --release -p meshctl-desktop

echo ">> Done: target/release/meshctl-desktop"
echo "   Now: flatpak-builder --user --install --force-clean build-dir desktop/flatpak/me.ogarcia.meshctl.yml"
