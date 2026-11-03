#!/usr/bin/env bash
# Has Argentum's edited rule moved since this crate was checked against it?
#
# Compares each Argentum file the rule depends on with its SHA-256 when this
# crate was proven against Argentum (main at 772b1b28, 2026-11-03). A change
# is not necessarily a change of rule; it means the differential check should
# be run again (see the README), then the hashes below updated.
#
#   argentum-drift.sh            # Argentum's main branch, from GitHub
#   argentum-drift.sh <folder>   # a local Argentum checkout
#
# Names each file that changed or is gone; exits 1 when one changed, 2 when
# one is gone.
set -euo pipefail

PINNED='
f2b3c8d0292d07c827580538e15f39d74208acd0077e57552f5467e3be81f747 src-tauri/engine/src/rm_lens_builtin.rs
f2a84b43887a706dbe3b82a4ca91a06ea36a2eb648fdda56530ccd9b35b7ea61 src-tauri/src/app_settings.rs
2be2e6486cedfb04c2cae8208e8cc86ff8dccc48401e29a42a0b4412709b4af9 src-tauri/src/exif_processing.rs
88bcc41016eeb68af3ead403f39887640c0428c20eff3ec7cad0125295c22492 src-tauri/src/file_management.rs
5bd8f3f6dc8e707f613603afc7a262d52f7cf617e604a1a453a9aefc19ede44e src-tauri/src/formats.rs
1c14d911b2038382571a52b582a66ca7e3e31821a58b099bdeb15cf21a322b4e src-tauri/src/guided_perspective.rs
3d6bbfc991b4b571ab5b161f5045c10f904007325c10791dc92c1c76154e967c src-tauri/src/image_processing.rs
3170347ea157b66519138fc29ed80fdb5e0c420759aecf5c5d2aaa5330cca864 src-tauri/src/mods/black_white.rs
12e662dde869c70697fc69ec3ba72f57e66509e9bad77488e874c46bb4ad6a86 src-tauri/src/mods/clipping.rs
0c0aeddab1f05c4645de11e5d0595a15347974a69329998a1c385745546de031 src-tauri/src/mods/color_grade.rs
f575258e495dc789f0f2d3fca9dba5cc2ce01d7020fd29bd415a326d32e65a87 src-tauri/src/mods/dng_baseline.tsv
9178c2641c78805493f21ff60d4564505991ca01e0ec135d278a8beb5db18f2e src-tauri/src/mods/dng_standard.rs
440655d90c5981840f8851ddefa79a26183dd2ecd68f92dba5654661afb6c4bd src-tauri/src/mods/edit_status.rs
b97edbc09ae1a1a6032202f72486ed2d5d4a2909c35eb853f2443ad73f1ac1f9 src-tauri/src/mods/file_ops.rs
8c0ab494826c4711f68641d371d66f3a73efed24e217923641116ea37df7a7be src-tauri/src/mods/film_lut.rs
3e806d048a0bb3516c477aa8506b62beb23e0fd8983c531772d216e6dbe27b3d src-tauri/src/mods/lens_builtin.rs
a364f03639cd0db6a9ceeab15c1cca95ad385c695504d30005003be55070d75a src-tauri/src/mods/profile_correction.rs
aafec4c70a6746b3aa62703253d18e194dff4f613413f4d600910d2d566d1dd5 src-tauri/src/mods/provider_look.rs
3732de7db9bb0eb5db3b5bb7bfd8c247697de28d4d24318341cc228bf4ef931e src-tauri/src/mods/raw_tone.rs
2373ffa723d1d739bc23b3d7995cd58ce5abd9eb6cc7b54e627209941cacdfc0 src-tauri/src/mods/salvage.rs
75066250fc7c56b7d3090f8d3cfa7657943e684346b1edf38f4c483977b58d26 src-tauri/src/mods/sharpen.rs
40330bed29c22471c748027597942d257b06a81ce7b1cbb24935ce0c26cda104 src-tauri/src/mods/tif_raw.rs
04bd0bb525179bee78c92182c9a819612d61deb703c70ab202231ba7b734fcf8 src-tauri/src/white_balance.rs
b0ed8ca26a6603ede086e97bc5608abf31576628e2b1ddae2bf8b04c2ecaab46 src/argentum/defaultAdjustments.json
3742582e28a20a4a340282414e6e8f94f5fa7c4491f771a2f2a024b9eeeb1823 src/utils/defaultAdjustments.json
'

ROOT="${1:-}"
fetch() {
  if [ -n "$ROOT" ]; then
    cat "$ROOT/$1"
  else
    curl -fsSL --max-time 30 "https://raw.githubusercontent.com/akhater/Argentum/main/$1"
  fi
}

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT
changed=0
gone=0
while read -r pinned file; do
  [ -n "$file" ] || continue
  if ! fetch "$file" > "$TMP" 2> /dev/null; then
    echo "gone: $file"
    gone=1
  elif [ "$(tr -d '\r' < "$TMP" | sha256sum | cut -c1-64)" != "$pinned" ]; then
    echo "changed: $file"
    changed=1
  fi
done <<< "$PINNED"
[ "$gone" = 0 ] || exit 2
[ "$changed" = 0 ] || exit 1
