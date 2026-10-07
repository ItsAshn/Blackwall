#!/usr/bin/env sh
# Render a screenshot headlessly (Xvfb + any Vulkan driver, incl. lavapipe).
# usage: scripts/screenshot.sh OUT.png [blackwall args...]
# e.g.   scripts/screenshot.sh docs/images/overview.png --demo --quality high
set -eu
out=$1; shift
bin=${BLACKWALL_BIN:-target/release/blackwall}
WGPU_BACKEND=${WGPU_BACKEND:-vulkan} xvfb-run -a -s "-screen 0 1600x900x24" \
  "$bin" --size 1600x900 --screenshot "$out" "$@"
