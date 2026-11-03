#!/usr/bin/env bash
# Has Argentum's edited rule moved since this crate was checked against it?
#
# Hashes the parts of Argentum's source the rule depends on (the functions,
# structs and constants below, and a few whole files) and compares the hash
# with the one this crate was proven against. A change is not necessarily a
# change of rule; it means the differential check should be run again.
#
#   argentum-drift.sh            # Argentum's main branch, from GitHub
#   argentum-drift.sh <folder>   # a local Argentum checkout
#
# Prints the hash; exits 1 when it differs from PINNED, 2 when a part is gone.
set -euo pipefail

# Argentum main at 772b1b28, 2026-11-03.
PINNED=7dc2457db374bc72bf8d7912b5f3126525e07a2ee68cfb02876f115d0a561985

# file | start of the item's first line ("*" for the whole file)
PARTS='
src-tauri/src/image_processing.rs|pub struct ImageMetadata
src-tauri/src/image_processing.rs|pub struct Crop
src-tauri/src/image_processing.rs|pub fn get_geometry_params_from_json
src-tauri/src/image_processing.rs|pub fn is_geometry_identity
src-tauri/src/image_processing.rs|pub struct GlobalAdjustments
src-tauri/src/image_processing.rs|const SCALES: AdjustmentScales
src-tauri/src/image_processing.rs|fn parse_hsl_adjustments
src-tauri/src/image_processing.rs|fn parse_color_grade_settings
src-tauri/src/image_processing.rs|fn convert_points_to_aligned
src-tauri/src/image_processing.rs|fn shader_curve
src-tauri/src/image_processing.rs|pub fn resolve_tonemapper_override
src-tauri/src/image_processing.rs|pub fn is_image_edited
src-tauri/src/image_processing.rs|fn is_section_visible
src-tauri/src/image_processing.rs|pub fn target_white_balance
src-tauri/src/image_processing.rs|pub fn is_lens_correction_shown
src-tauri/src/image_processing.rs|fn get_global_adjustments_from_json
src-tauri/src/image_processing.rs|pub fn get_all_adjustments_from_json
src-tauri/src/white_balance.rs|*
src-tauri/src/guided_perspective.rs|pub enum GuideOrientation
src-tauri/src/guided_perspective.rs|pub struct GuideLine
src-tauri/src/guided_perspective.rs|pub struct GuidePoint
src-tauri/src/file_management.rs|pub fn sidecar_for
src-tauri/src/file_management.rs|pub fn parse_sidecar_name
src-tauri/src/exif_processing.rs|pub fn peek_sidecar
src-tauri/src/formats.rs|pub const RAW_EXTENSIONS
src-tauri/src/formats.rs|pub const NON_RAW_EXTENSIONS
src-tauri/src/formats.rs|pub fn is_raw_file
src-tauri/src/mods/tif_raw.rs|pub fn is_camera_raw
src-tauri/src/app_settings.rs|tonemapper_override_enabled: Some(
src-tauri/src/mods/salvage.rs|pub fn readable_keys
src-tauri/src/mods/salvage.rs|pub fn readable<
src-tauri/src/mods/edit_status.rs|impl EditStatus
src-tauri/src/mods/provider_look.rs|pub const MARKER
src-tauri/src/mods/provider_look.rs|pub const KEY
src-tauri/src/mods/provider_look.rs|pub struct ProviderLook
src-tauri/src/mods/provider_look.rs|pub struct RenderingBefore
src-tauri/src/mods/provider_look.rs|fn look_of
src-tauri/src/mods/file_ops.rs|pub type LookOptions
src-tauri/src/mods/file_ops.rs|pub struct LookWb
src-tauri/src/mods/file_ops.rs|pub struct OwnWb
src-tauri/src/mods/sharpen.rs|pub const DEFAULT_CONTRAST
src-tauri/src/mods/sharpen.rs|pub const DEFAULT_RADIUS
src-tauri/src/mods/sharpen.rs|pub const DEFAULT_ITERATIONS
src-tauri/src/mods/sharpen.rs|pub const DEFAULT_USM_RADIUS
src-tauri/src/mods/sharpen.rs|pub const DEFAULT_USM_THRESHOLD
src-tauri/src/mods/sharpen.rs|pub const DNG_STANDARD_AMOUNT
src-tauri/src/mods/sharpen.rs|pub const DNG_STANDARD_USM_RADIUS
src-tauri/src/mods/sharpen.rs|fn number(
src-tauri/src/mods/sharpen.rs|fn flag(
src-tauri/src/mods/sharpen.rs|pub fn from_json
src-tauri/src/mods/sharpen.rs|fn locals
src-tauri/src/mods/raw_tone.rs|pub fn mode_index
src-tauri/src/mods/raw_tone.rs|pub fn mode_of
src-tauri/src/mods/raw_tone.rs|pub fn points_of
src-tauri/src/mods/raw_tone.rs|fn fill_missing
src-tauri/src/mods/raw_tone.rs|pub fn is_camera_raw_style
src-tauri/src/mods/dng_standard.rs|*
src-tauri/src/mods/dng_baseline.tsv|*
src-tauri/src/mods/film_lut.rs|pub fn declared_input
src-tauri/src/mods/film_lut.rs|pub fn exposure_ev
src-tauri/src/mods/clipping.rs|pub fn mode
src-tauri/src/mods/clipping.rs|pub const OFF
src-tauri/src/mods/clipping.rs|pub const ALL
src-tauri/src/mods/clipping.rs|pub const LAST
src-tauri/src/mods/black_white.rs|pub fn from_json
src-tauri/src/mods/black_white.rs|pub fn leave_out_colour_controls
src-tauri/src/mods/color_grade.rs|pub fn of
src-tauri/src/mods/color_grade.rs|fn is_lightroom
src-tauri/src/mods/lens_builtin.rs|pub const KEYS
src-tauri/src/mods/lens_builtin.rs|pub fn from_adjustments
src-tauri/src/mods/lens_builtin.rs|pub fn with_lensfun
src-tauri/src/mods/profile_correction.rs|pub fn rows_for
src-tauri/src/mods/profile_correction.rs|fn built_in_for_this_render
src-tauri/src/mods/profile_correction.rs|fn colour_for_this_render
src-tauri/src/mods/profile_correction.rs|pub fn correction_for
src-tauri/src/mods/profile_correction.rs|fn chosen_correction
src-tauri/src/mods/profile_correction.rs|fn dng_colour_correction
src-tauri/src/mods/profile_correction.rs|fn fits_this_photo
src-tauri/engine/src/rm_lens_builtin.rs|pub struct Radial
src-tauri/engine/src/rm_lens_builtin.rs|fn valid
src-tauri/engine/src/rm_lens_builtin.rs|pub struct Parts
src-tauri/engine/src/rm_lens_builtin.rs|pub struct LensCorrection
src-tauri/engine/src/rm_lens_builtin.rs|pub fn is_empty
src-tauri/engine/src/rm_lens_builtin.rs|pub fn validate
src/utils/defaultAdjustments.json|*
src/argentum/defaultAdjustments.json|*
'

CACHE=$(mktemp -d)
trap 'rm -rf "$CACHE"' EXIT

# A file of Argentum, fetched once.
source_of() {
  local cached="$CACHE/${2////_}"
  if [ ! -f "$cached" ]; then
    if [ -n "$1" ]; then
      cat "$1/$2" > "$cached" || return 1
    else
      curl -fsSL --max-time 30 "https://raw.githubusercontent.com/akhater/Argentum/main/$2" > "$cached" || { rm -f "$cached"; return 1; }
    fi
  fi
  cat "$cached"
}

# The item whose first line starts (after its indentation) with $1, with the
# attributes just above it: through the line closing it at the same
# indentation, or the first line if that ends the item itself.
item() {
  awk -v pat="$1" '
    !on {
      i = index($0, pat)
      if (i > 0 && substr($0, 1, i - 1) ~ /^[ \t]*$/) {
        on = 1; ind = substr($0, 1, i - 1); printf "%s", attrs; print
        if ($0 ~ /[;,][ \t]*$/) exit
      } else if ($0 ~ /^[ \t]*#\[/) {
        attrs = attrs $0 "\n"
      } else {
        attrs = ""
      }
      next
    }
    { print; if ($0 == ind "}" || $0 == ind "};" || $0 == ind "},") exit }'
}

ROOT="${1:-}"
missing=0
all=$(
  while IFS='|' read -r file start; do
    [ -z "$file" ] && continue
    text=$(source_of "$ROOT" "$file" | tr -d '\r') || { echo "missing file: $file" >&2; echo "MISSING $file"; continue; }
    if [ "$start" = "*" ]; then
      part=$text
    else
      part=$(printf '%s\n' "$text" | item "$start")
    fi
    if [ -z "$part" ]; then
      echo "missing: $file: $start" >&2
      echo "MISSING $file $start"
    fi
    printf '== %s | %s\n%s\n' "$file" "$start" "$part"
  done <<< "$PARTS"
)
if printf '%s\n' "$all" | grep -q '^MISSING '; then missing=1; fi
hash=$(printf '%s\n' "$all" | sha256sum | cut -c1-64)
echo "$hash"
[ "$missing" = 1 ] && exit 2
[ "$hash" = "$PINNED" ] || exit 1
