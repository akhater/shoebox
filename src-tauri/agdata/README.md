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
  `parse_sidecar_name`.
- **Kind.** The answer Argentum's library gives, with Argentum's default
  settings (no tone mapper forced in Settings). It depends on whether the photo
  is a RAW file: an edit that names no tone rendering is in DNG Standard for a
  RAW and in none for any other photo.
- **`is_raw` must be Argentum's own answer** for the photo, or the kind can
  differ: `agdata::is_raw(file_name)` holds Argentum's list of RAW extensions
  (a fact of its format support, watched by `argentum-drift.sh`). A
  `.tif`/`.tiff` that is a camera's RAW is one in Argentum too, which a name
  alone does not tell. A caller's own idea of "RAW" (another list, or "not a
  JPEG") is not enough.
- **Fingerprint.** BLAKE3 of the edit's (`adjustments`) text as the sidecar
  holds it. Rating, tags, flag, the EXIF copy, history and snapshots do not
  count, so Argentum rewriting those leaves it as it was. Argentum writes the
  same edit as the same text (keys sorted, numbers in their shortest form, one
  layout), so the text moves exactly when the edit does; the same values
  written another way hash differently, which costs one needless re-render.
- **Lean.** The file is checked as JSON and kept as slices of its text; of the
  edit, a value is built only when the rule reads it. The EXIF copy, history,
  snapshots, masks and AI patches (often megabytes) never are: about 30 KB of
  heap for a 9.5 MB sidecar. An unreadable sidecar is an `Err`, never a
  panic; Argentum shows its photo unedited.

Dependencies: `serde`, `serde_json`, `blake3`. The crate writes nothing.

## How the answer is kept Argentum's

Argentum marks a photo edited when its edit names a camera profile, holds AI
patches or masks, crops off-centre, turns, flips or reshapes the picture, or
else when what its renderer is given differs from what an empty edit gives it,
value by value as 32-bit floats behind each section's eye. This crate makes the
same test from the same values (`src/edited.rs`).

That is proven, not assumed: a differential check ran Argentum's own code and
this crate on the same sidecars and required the same answer for each, for a
RAW photo and for any other: 6,237 generated sidecars (every editor value moved
off its default one at a time, alone and among the editor's defaults; edges of
each rule; film looks; malformed files and text Argentum's reader refuses;
random mixes; null, missing and array-written fields of every shape the rule
reads) and 295 real ones, 13,064 answers, all identical but those of the
hand-damaged sidecars below. `tests/fixtures/generated.jsonl` is the
generated part (6,031 distinct sidecars) with Argentum's answers, so the
crate's tests guard them;
`tests/fixtures/editor-defaults.json` is the editor's starting edit those cases
are built on.

`argentum-drift.sh` holds the SHA-256 of each Argentum file the rule depends
on; CI warns when one on Argentum's main branch no longer matches, which means
the check should be run again.

## Limits

- **Hand-damaged sidecars.** Argentum's reader refuses a whole sidecar with a
  lone surrogate escape (`"�"`), a number past a 64-bit float or nesting
  deeper than 128 anywhere in it, and shows its photo unedited. This crate
  passes over such a part (and reads it as null where the rule asks for it), so
  it may mark that photo edited. Argentum never writes such a file.
- **A fingerprint that moves once.** Argentum keeps the edit as a parsed value
  and writes it back whenever it saves the sidecar. An edit it never rewrote
  since, whose numbers it reads back a little off or whose keys another of its
  writers left unsorted, changes text once on that save, then stays: 13 of the
  295 real sidecars. The same for an edit saved before Argentum's white balance
  change (no `whiteBalance` key), converted once to new numbers for the same
  colour.

## Credits and licence

The sidecar's key names, default values and the rule's thresholds are facts
of Argentum's file format, used here as such. This crate contains no Argentum
or RapidRAW code: the rule (`src/edited.rs`) and the naming rule
(`src/name.rs`) were written clean-room, from a description of their behaviour
only, by an implementer who did not see Argentum's source (who also made its
later changes); the reader and the fingerprint are this crate's own design.

MIT, see `LICENSE`.
