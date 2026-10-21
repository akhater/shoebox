# Shoebox

**Every photo, every disk.**

Shoebox is a desktop catalog for photo archives spread across external drives.
Browse, search and organize your whole collection while the drives sit
unplugged in a drawer; plug one in and Shoebox picks up where it left off.

> **Status: early development.** Windows is the supported platform. macOS and
> Linux builds are previews (see [Platforms](#platforms)).

## Features

- **Browse everything offline.** Shoebox keeps a preview of every photo, so
  the whole archive stays browsable when its drives are unplugged. Offline
  photos are marked as such, never removed.
- **Backup drives welcome.** The same photos on two drives are one photo with
  two copies, not duplicates. Adding the backup drive builds no new previews,
  and each photo opens from whichever of its drives is connected.
- **Reads what your cameras write.** RAW files from most cameras (CR2, CR3,
  NEF, ARW, RAF, ORF, DNG and more), plus JPEG, HEIC, TIFF, PNG, WebP, AVIF
  and Photoshop documents (PSD, PSB). A Photoshop file is recognised by its
  content, even saved under another name.
- **Search by description.** Type "kids on a beach" and find the matching
  photos. It runs on your own GPU, from the previews, so it works on unplugged
  drives too.
- **People.** Faces are detected and grouped automatically. Name a person once
  and find every photo they appear in. **Check** finds likely mix-ups among
  the people you named (someone twice in a photo, a sibling filed under the
  wrong child, one person under two names) to fix in a click, with Undo, or
  to leave as is: it stays out of the next checks until something changes.
- **Filters** by person, tag, date, camera, lens, file type and status
  (online, offline, missing).
- **Tags** that stay with the photo, even when the file is moved or copied.
- **Gear.** Each camera and lens is listed under one name, however each file
  spells it; rename or merge them from the sidebar.
- **Navigate by date** with a date rail on the grid and a calendar of every
  day you took photos.
- **Culling.** Mark photos as you review them, then send them all to the
  Recycle Bin (the Trash on macOS and Linux) in one step. A RAW's camera
  JPEG goes with it.
- **Cull's assistant.** A shoot one shot at a time: the frames of a burst
  gathered, the sharpest first in line (at the face, else where the camera
  focused), and what's wrong with each frame -
  blurry, shaken, a face out of focus, the focus missed where the camera says
  it focused, too dark, blown out, nothing in it (lens cap). At 100% it looks
  first where the camera focused.
  It only suggests: you reject, take back, or accept its suggestions in one
  key. A shoot is measured from its previews when you open it, so unplugged
  drives too; with the drive connected it looks closer at the originals and
  finds what is only soft at 100%.
- **Turn photos** with R and Shift+R, in the grid or the viewer. Only Shoebox's
  copies turn, never the files; faces keep their names. **Sideways** lists the
  photos an AI model finds sideways or upside down (an old camera without an
  orientation sensor, a camera pointed up or down), shown turned the way it
  suggests: turn them all, or leave the ones it got wrong.
- **Organize folders.** Rename, create and move folders by drag and drop. The
  catalog updates in the same step, with no rescan.
- **Full-screen viewer**, sharp from the original when its drive is connected,
  with 100% zoom and a filmstrip.
- **Focus points.** Press F in the viewer to see where the camera focused, as
  the camera makers' own software shows it: the points that found focus in
  red, over the camera's other points. Read from the files of Canon, Nikon
  (Z and live view), Sony, Fujifilm, Olympus and OM System, Panasonic and
  Leica, Pentax (live view) cameras, and phones; shown for photos on unplugged
  drives too.
- **Private photos.** Press Ctrl+Shift+L and type a password: every password
  opens its own private space, like a wallet's passphrase, and none is ever
  refused. Photos you make private there are hidden from every view, count,
  search and script until that password opens them again, and Shoebox's own
  copies of them are sealed on disk. The photo files are never touched.
  Give a space a name and it shows next to Private while it is open.
- **Scriptable.** Everything the window does is also available from
  `shoebox-cli`, with JSON output, for scripts and AI agents.

## Your photos are safe

- **Shoebox never writes to a photo file.** Photos are opened read-only. The
  only file operations are the ones you ask for (moving a folder, deleting
  photos you marked), and they go through a dedicated module with its own
  checks: a move to another drive is copied, verified byte for byte, and only
  then is the original removed.
- **Nothing is left half-done.** A move, rename or delete interrupted by a
  crash is finished, or rolled back, the next time Shoebox starts.
- **An unplugged drive never changes the catalog.**
- **Nothing is removed because it wasn't found.** A file is marked *missing*
  only after a complete scan of its connected drive. It stays in the catalog
  and is relinked automatically if it turns up again. The Missing view shows
  which missing photos are in the Recycle Bin or Trash (deleted, not moved);
  only you can take them out of the catalog.
- **A photo is its content, not its path.** Photos are identified by a content
  fingerprint, so moved, renamed and copied files keep their tags and people.
  Drives are identified by serial number, not by drive letter.
- **Everything stays on your computer.** No account, no cloud, no telemetry.
  Shoebox connects to the internet only to download the AI models you choose
  and to check for new releases (which can be turned off in Settings).

## Download

Download the latest version from the
[Releases page](https://github.com/akhater/shoebox/releases).

| Platform | Installer | Portable |
|---|---|---|
| Windows (x64, ARM64) | `.exe` setup, per user, no admin rights needed | `.zip` |
| macOS (Intel, Apple silicon) | `.dmg` | `.tar.gz` |
| Linux (x64, ARM64) | `.deb`, `.rpm`, `.AppImage` | `.tar.gz` |

The portable version keeps its catalog and models in its own folder, so it can
run from anywhere, including an external drive.

The builds are not code-signed yet, so Windows SmartScreen or macOS Gatekeeper
may show a warning on first launch.

On macOS, Gatekeeper may refuse to open Shoebox at all, saying it is damaged or
cannot be opened. After moving Shoebox to Applications, remove the quarantine
flag macOS puts on downloaded apps by running this in Terminal:

```bash
xattr -dr com.apple.quarantine "/Applications/Shoebox.app"
```

For the portable version, run the same command on the extracted `Shoebox`
folder instead.

### Platforms

- **Windows 10 and 11.** Fully supported. HEIC photos are read through
  Windows' own HEIF support (Microsoft's HEIF Image Extensions).
- **macOS and Linux.** Preview builds: drive identification is not implemented
  on these systems yet, so folders cannot be added, and HEIC is not supported
  there yet.

## Getting started

1. Install and open Shoebox.
2. Click **Add folder** in the sidebar and choose a folder of photos, or a
   whole drive. Shoebox scans it and builds a preview of each photo.
   You can start with a few folders and add their parent later: the folders
   already scanned become part of it and are not read again.
3. Repeat for each drive. Once scanned, a drive can be unplugged; when it is
   connected again, Shoebox notices and catches up on its own.
4. For word search, people and sideways photos, open **Settings > AI models** and download the
   models.

## AI models

Word search, face recognition and finding sideways photos run entirely on
your computer. The models
are not bundled with the app: Shoebox downloads them on request from their
original publishers and verifies each file against a pinned SHA-256 hash.

| Feature | Model | Size |
|---|---|---|
| Word search | CLIP ViT-B-32 (as packaged by Immich) | 581 MB |
| Faces | InsightFace buffalo_l (as packaged by Immich) | 182 MB |
| Sideways photos | Image orientation detector v2, EfficientNetV2 (Duarte Barbosa) | 81 MB |
| AI runtime | Microsoft ONNX Runtime 1.22, on the GPU (DirectML) or the CPU | |

Download, verify or delete them in **Settings > AI models**, or with
`shoebox-cli models`.

The buffalo_l face model is licensed by InsightFace for non-commercial use
only.

## Where your catalog lives

| | Catalog | AI models |
|---|---|---|
| Installed on Windows | `%USERPROFILE%\.shoebox` | `models` in the install folder |
| Installed on macOS / Linux | `~/.shoebox` | `~/.shoebox/models` |
| Portable | `data` next to the app | `models` next to the app |

The catalog can be moved to any folder or drive from **Settings**. It is made
of a small database plus one preview and one thumbnail file per photo, so
incremental backups stay small. **Settings > Make backup copies now** (or
`shoebox-cli backup`) writes consistent copies of the databases while Shoebox
runs, for backup tools such as Kopia. Its `logs` folder holds one log per
day; anything there is removed after 14 days.

## Command line

`shoebox-cli` ships with the app and covers what the window does, with JSON
output for most commands:

```bash
shoebox-cli add "D:\Photos"      # add a folder and scan it
shoebox-cli scan                 # rescan every connected folder
shoebox-cli stats                # catalog overview
shoebox-cli search kids on a beach
shoebox-cli people               # everyone found, with their names
shoebox-cli people-check         # likely mix-ups among named people
shoebox-cli turn right 42        # a quarter turn clockwise, in Shoebox only
shoebox-cli sideways             # photos that look sideways or upside down
shoebox-cli focus "D:\Photos\IMG_0001.CR3"   # where the camera focused, as Shoebox reads it
shoebox-cli cull '{}' '{"kind":"root","id":1}'   # a folder as Cull shows it: bursts, problems, suggestions
shoebox-cli models               # which AI models are installed
shoebox-cli bench                # how fast the main views are on this catalog and computer
shoebox-cli logs                 # the log files and the newest log's last lines
```

The full list of commands is documented at the top of
[`shoebox-cli.rs`](src-tauri/src/bin/shoebox-cli.rs). Set `SHOEBOX_DATA` to a
folder to use a different catalog. An MCP server is planned.

## Building from source

Requirements: [Rust](https://rustup.rs) (stable), [Node.js](https://nodejs.org)
22, and the [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/)
for your system.

```bash
git clone https://github.com/akhater/shoebox.git
cd shoebox
npm install
npm run tauri dev
```

In development, the catalog and the AI models live in `data/` and `models/`
at the repository root. `SHOEBOX_DATA=<folder>` and `SHOEBOX_MODELS=<folder>`
point the app or the CLI at other folders, for example throwaway ones for
testing.

```bash
cd src-tauri
cargo run --bin shoebox-cli -- stats
```

Tests: `npm test` for the window (Vitest and jsdom, against a fake backend
in `src/test`), `cargo test` in `src-tauri` for the rest.

## Contributing

Issues and pull requests are welcome. Every change goes through a pull request
and is merged only when CI passes: formatting, lint, tests (including the
guards that keep photo files read-only and the window responsive), type check
and build. Release builds are produced only when a release is published. See
[CHANGELOG.md](CHANGELOG.md) for what changed in each version.
