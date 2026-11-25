#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>
#
# Draws the app's icons from their sources in assets/icons: the PNGs the
# desktop app and its packages take, and the Windows .ico. Up to 32 px from
# app-small.svg, the gear of six teeth that still reads that small; from 48
# px on, from app.svg. And the web app's, in web/pwa: the same, and one to
# be cut to any shape (maskable), from app-maskable.svg. And Android's
# launcher icon for the versions without layered ones.
#
# Needs rsvg-convert (librsvg) and magick (ImageMagick 7).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICONS="$ROOT/assets/icons"
OUT="$ICONS/png"
mkdir -p "$OUT"

for size in 16 24 32 48 64 128 256 512; do
  if [ "$size" -le 32 ]; then
    source="$ICONS/app-small.svg"
  else
    source="$ICONS/app.svg"
  fi
  rsvg-convert -w "$size" -h "$size" -o "$OUT/$size.png" "$source"
done

magick "$OUT/16.png" "$OUT/24.png" "$OUT/32.png" "$OUT/48.png" \
  "$OUT/64.png" "$OUT/256.png" "$ICONS/meshctl.ico"

PWA="$ROOT/web/pwa"
mkdir -p "$PWA"
rsvg-convert -w 192 -h 192 -o "$PWA/icon-192.png" "$ICONS/app.svg"
rsvg-convert -w 512 -h 512 -o "$PWA/icon-512.png" "$ICONS/app.svg"
rsvg-convert -w 512 -h 512 -o "$PWA/icon-maskable-512.png" "$ICONS/app-maskable.svg"
rsvg-convert -w 180 -h 180 -o "$PWA/apple-touch-icon.png" "$ICONS/app-maskable.svg"

# Android's launcher icon, for Android 7, which takes no layers: from
# Android 8 on, mipmap-anydpi-v26 draws it from vectors of its own.
RES="$ROOT/desktop/gen/android/app/src/main/res"
for density in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:144 xxxhdpi:192; do
  rsvg-convert -w "${density#*:}" -h "${density#*:}" \
    -o "$RES/mipmap-${density%:*}/ic_launcher.png" "$ICONS/app.svg"
done

echo "Drawn: $OUT/*.png, $ICONS/meshctl.ico, $PWA/*.png and Android's"
