# agdata

Tells, from an [Argentum](https://github.com/akhater/Argentum) sidecar
(`.agdata`), whether its photo was edited and how, and gives a fingerprint of
the edit that changes only when the edit does.

```rust
let status = agdata::status(&std::fs::read(sidecar_path)?, is_raw)?;
match status.kind {
    agdata::Kind::None => {}  // no mark
    agdata::Kind::Edit => {}  // Argentum's edit mark
    agdata::Kind::Film => {}  // Argentum's film mark: a provider look on
}
let key = status.fingerprint.to_hex(); // re-render when this changes
```

- **Names.** `<photo>.agdata` is the photo's edit, `<photo>.<id>.agdata` a
  virtual copy's (`id`: six lowercase hex digits): `sidecar_name`,
  `copy_sidecar_name`, `parse_sidecar_name`.
- **Kind.** The answer Argentum's library gives, with Argentum's default
  settings (no tone mapper forced in Settings). It depends on whether the photo
  is a RAW file: an edit that names no tone rendering is in DNG Standard for a
  RAW and in none for any other photo.
- **Fingerprint.** BLAKE3 of the edit (`adjustments`) alone, independent of key
  order and layout. Rating, tags, flag, the EXIF copy, history and snapshots do
  not count, so Argentum rewriting those leaves it as it was.
- **Lean.** One pass over the file. The EXIF copy, history and snapshots are
  skipped without being built; masks and AI patches (often megabytes) are
  hashed as they stream past; only the few small values the rule reads are
  kept. An unreadable sidecar is an `Err`, never a panic; Argentum shows its
  photo unedited.

Dependencies: `serde`, `serde_json`, `blake3`. The crate writes nothing.

## How the answer is kept Argentum's

Argentum marks a photo edited when its edit names a camera profile, holds AI
patches or masks, crops off-centre, turns, flips or reshapes the picture, or
else when what its renderer is given differs from what an empty edit gives it,
value by value as 32-bit floats behind each section's eye. This crate makes the
same test from the same values (`src/edited.rs`).

That is proven, not assumed: a differential check ran Argentum's own code and
this crate on the same sidecars and required the same answer for each, for a
RAW photo and for any other. `tests/fixtures/generated.jsonl` is the generated
part of that corpus with Argentum's answers, so the crate's tests guard them.

`argentum-drift.sh` hashes the parts of Argentum's source the rule depends on;
CI warns when Argentum's main branch no longer matches the pinned hash, which
means the check should be run again.

Not covered by the fingerprint staying put: an edit saved before Argentum's
white balance change (no `whiteBalance` key) is converted once by Argentum to
new numbers for the same colour, so its fingerprint changes once. A render made
from the file before that conversion would differ anyway.

## Credits and licence

The sidecar's key names, default values and the rule's thresholds are facts
of Argentum's file format, used here as such. This crate contains no Argentum
or RapidRAW code.

MIT, see `LICENSE`.
