# focus-points

Reads the autofocus points a camera records in a photo's maker note, as
fractions of the picture, ready to draw over it.

- Canon (AFInfo, AFInfo2 / AFInfo3, focus bracketing), Nikon, Sony, Fujifilm,
  Olympus / OM System, Panasonic / Leica and Pentax / Ricoh maker notes.
- The standard EXIF SubjectArea of any maker (phones).
- The autofocus mode in plain words, whatever each maker calls it.
- Points turned as the picture is turned (EXIF orientation, quarter turns).

The crate depends only on `serde`. It does not parse files: you give it the
tags through the small `Directory` trait, implemented over whatever TIFF / EXIF
reader you already use, and it returns a `Focus`.

```rust
let focus = focus_points::read(&source, None); // Option<Focus>
```

Makers that only name their points ("upper left") without placing them, or that
encipher them, give nothing.

## Credits

The tag numbers, offsets and record layouts were researched from
[ExifTool](https://exiftool.org) by Phil Harvey, whose work is the reference
for these formats. This crate contains no ExifTool code.

## Licence

MIT, see `LICENSE`.
