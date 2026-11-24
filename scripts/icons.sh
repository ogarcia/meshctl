#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>
#
# Draws the app's icons from their sources in assets/icons: the PNGs the
# desktop app and its packages take, and the Windows .ico. Up to 32 px from
# app-small.svg, the gear of six teeth that still reads that small; from 48
# px on, from app.svg.
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

echo "Drawn: $OUT/*.png and $ICONS/meshctl.ico"
