# Shoebox

*Every photo, every disk.*

An offline catalog of every photo on every disk, for a photographer whose
archive lives on USB disks that are usually unplugged. Browse, search and
organise all of it with the disks in a drawer; plug one in and Shoebox picks
up where it left off.

Windows first. Rust + Tauri 2 + React.

## What it promises

1. **It never writes to a photo file.** Photos are opened read-only. Every
   file write lives in one module and is refused outside Shoebox's own
   folders; a test fails if write code appears anywhere else. The only
   exception is what you ask for explicitly - moving a folder, deleting the
   photos you marked - and that lives in its own module with its own checks
   (a move to another disk is copied, verified byte for byte, and only then is
   the original removed).
2. **An unplugged disk never changes the catalog.** Its photos stay,
   browsable from Shoebox's own previews, marked offline.
3. **Nothing is removed because it wasn't found.** A file is *missing* only
   after a complete scan of a plugged-in disk, and stays in the catalog.
4. **A photo is its content, not its path.** Moves and copies keep the same
   photo, with its tags and people. Disks are known by serial number, never by
   drive letter, so a disk mounted at `E:` today and at
   `C:\Mounts\Disk1` tomorrow is the same disk.

## What it does

- Catalogs every RAW format plus JPEG, HEIC, TIFF, PNG, WebP, AVIF; keeps a
  preview of each so everything stays browsable offline.
- Word search ("kids on a beach") and face recognition, on the GPU, from the
  previews - so they work on unplugged disks too.
- Filters: people, tags, date (to the day), camera, lens, file type, status.
- Tags that stay with the photo wherever it goes.
- Culling: mark, review, then send to the Recycle Bin (a RAW takes its JPEG
  twin and its Argentum edits along).
- Reorganise by drag and drop; rename and create folders in place.
- A viewer that fills the screen, sharp from the original when its disk is
  plugged in, with 100% zoom and a filmstrip.
- Built to be usable without its window: `shoebox-cli` already does most of
  it in JSON, for scripts and AI agents; an MCP server is next.

## Build

```bash
npm install
npm run tauri dev
```

The AI models are not in the repository. Shoebox looks for them in `models/`
in the Shoebox folder: next to the app when it runs from its own folder (dev,
the portable download), `~/.shoebox` when installed.

```bash
cd src-tauri
cargo run --bin shoebox-cli -- stats
```

`SHOEBOX_DATA=<folder>` points the app or the CLI at a throwaway catalog.

## Working on it

`main` is protected: every change goes through a pull request, and merges
only when CI passes (format, lint, tests including the write guards,
typecheck, build). Builds for download (Windows, macOS, Linux; Intel/AMD
and ARM) are made only when a release is published. See
[CHANGELOG.md](CHANGELOG.md).
