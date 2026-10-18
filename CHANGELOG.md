# Changelog

Newest first. Same format and versioning as Argentum.

## 26.43.1 - unreleased

### Added

- **Focus points.** F in the viewer, or the Focus points switch on the photo,
  shows where the camera focused, drawn over the photo the way the camera
  makers' own software draws it: the camera's points faint, those that found
  focus in red, a point chosen without focus confirmed in white, a zone it was
  given dashed. Off until turned on, then kept on for the next photos and the
  next time. With the points shown, Z zooms to 100% where the camera focused.
  The info panel says what was in focus and how the camera was set ("1 of 9
  in focus · Single point"); a photo whose file says nothing says so.
  Read from the maker's note of Canon cameras (EOS DSLRs and mirrorless,
  PowerShots, CRW files), Nikon Z cameras and Nikon DSLRs in live view, Sony,
  Fujifilm, Olympus and OM System, Panasonic and Leica's Panasonic-made
  cameras, Pentax and Ricoh in live view, and from the standard EXIF subject
  area that phones write. DSLRs that only name their points through the
  viewfinder (older Nikon, Pentax and Sony A-mount bodies) have nothing to
  show yet. Read with the rest of the metadata when a photo is scanned, so the
  points show for photos on unplugged drives too; photos already in the
  catalog are read again, metadata only, when their drive is plugged in. Kept
  small: a few bytes per photo, each camera's layout of points once.
- `shoebox-cli focus <file>` and `find-focus`; `photo <id>` gives the focus points.

## 26.42.5 - 2026-10-18

### Added

- **Turn photos.** R turns the selected photos a quarter turn right, Shift+R
  left, in the grid or the viewer, with Undo; so does the Turn control, an
  arrow either side of the word, over the photo in the viewer and in the bar
  of the selected photos. Resting the pointer on an arrow shows the photos
  turned that way before anything turns. In the viewer's filmstrip, Shift
  and Ctrl clicks and Shift+arrows pick several photos to turn at once.
  Only the catalog and Shoebox's own copies change: the photo files are
  never touched, and photos on unplugged drives turn too.
  The turn shows everywhere - thumbnails, previews, the viewer's sharp
  picture and 100% zoom, Cull - and stays when previews are rebuilt. Faces
  keep their names, their boxes turned with the photo; the photo is then
  looked at again for faces missed while it stood the wrong way, and word
  search describes it again. The viewer says when a photo is turned, with a
  link back to how its file shows it.
- **Sideways photos.** An AI model (Duarte Barbosa's image orientation
  detector, MIT, 80 MB, in Settings > AI models like the others) looks at
  every photo before faces, and finds those that are sideways or upside down:
  a camera without an orientation sensor, or one pointed up or down. They
  wait in **Sideways** in the sidebar, shown turned the way it suggests:
  **Turn all**, or select the ones it got wrong and leave them as they are.
  Opened from there, the viewer shows each photo as it would be turned, with
  "Turn it" and "Leave it as it is", then moves on to the next; V, or the
  switch above the photos, shows them as they are, to spot the ones it got
  wrong. Nothing is turned until you say so, and a photo you turned or left
  is never suggested again. On previews of a real catalog turned four ways it
  found 96% of the turned ones and suggested turning 0.2% of the upright ones
  (it takes a second look at each photo turned the way it suggests).
- `shoebox-cli turn right|left|half|back`, `sideways`, `upright-check`,
  `sideways-accept`, `sideways-leave`, and `upright-eval` (how the model does
  on a folder of upright JPEGs).

- **Cull's assistant.** A **Cull** button above the photos opens the folder
  or day shown as a shoot, one shot at a time (its hover text says how many
  photos, and that a folder includes its subfolders). The frames of a burst (taken
  within seconds, framed the same and alike to word search) come as one
  stack, the frame to keep first in line; a RAW and its JPEG or export count
  as one frame. Each frame says what's wrong with it - blurry everywhere
  (with the shutter speed when it was slow for the lens), a face out of focus
  while something else is sharp, very dark, highlights blown, nothing in it
  (a lens cap, a pocket) - and what Cull suggests: keep or reject. Culling is trash or not: X
  rejects, P or K takes a frame back out, Enter rejects what Cull suggests
  rejecting in the shot and moves on, and **Accept all suggestions** does it
  for the whole shoot. Ctrl or Shift click selects several frames, or
  several shots in the list on the left, and X or P then applies to all of
  them. Z or a click shows the
  frame at 100% from its original, centred on the face or where you
  clicked; drag to move, and the burst's other frames open at the same
  spot, to compare their sharpness. A reject is a mark for deletion, as
  before: nothing is deleted until you confirm. Every photo is measured in
  the background from its preview, so photos on unplugged drives too; with
  the drive connected, **Look closer** reads the originals at half size
  (read-only) and finds the frames that are only soft at 100%; an original
  it can't read is named, with why, and not offered again. The measures
  live in `quality.db` beside the catalog: they can always be made again.
- **What Cull found, on thumbnails.** A toolbar toggle, off by default and
  remembered like the file type's, shows on each thumbnail what Cull found
  wrong, the reasons on hover.
- `shoebox-cli quality` measures the photos not measured yet, `measure`
  shows a photo's measures and problems, `cull` gives a view as Cull shows
  it (`--closer` looks at the originals first), and `cull-accept` marks the
  rejects it suggests.
- **Remove missing photos from the catalog.** A photo deleted outside
  Shoebox (in Explorer, Finder, another app or another catalog) stays in the
  catalog as missing, as before; now you can take it out. The Missing view
  has "Remove all from catalog", the toolbar has "Remove from catalog" for
  selected missing photos, and so does the viewer. Nothing on any disk is
  touched, and a photo with a copy on any disk, plugged in or not, stays.
  `shoebox-cli missing` lists them and `shoebox-cli forget-missing` removes
  them (by id, `--in-bin` or `--all`).

- **Deleted or moved?** Shoebox looks in the Recycle Bin (the Trash on macOS
  and Linux) for missing photos. Those found there are marked with a bin on
  their thumbnail, the date they were deleted, and a "Remove the N in the
  Recycle Bin" button. Restore one from the bin instead and the next scan
  finds it again with its faces and tags.

- **"Delete marked files" takes marked photos that are already gone.** A
  photo marked for deletion whose file was deleted elsewhere used to wait
  for its disk forever; the delete now takes it out of the catalog.

- **A log a day, kept two weeks.** The log was one file that grew forever.
  Shoebox now writes one per day, `logs\shoebox-<date>.log`, and at each
  start removes anything in the `logs` folder that hasn't been written to
  for 14 days. `shoebox-cli logs` lists the files and shows the newest
  log's last lines.

### Changed

- **A toolbar that fits.** The bar above the photos keeps the title and its
  count on one line and the search box wide, whatever the window's width.
  The filters, the order, the calendar and the thumbnail size are on a
  second bar, and what applies to the selected photos (Tag, Turn, Not
  <name>, Remove from catalog) floats over the photos with their count.
  Selecting photos no longer moves anything above the grid.

### Fixed

- **No slowdown after turning photos.** After each batch of new faces
  (turning photos finds some), the look for people found twice compared
  every person with every other: 7 to 14 s of work each time on a catalog
  of 5,800 people. It now compares only who changed since its last look,
  in a tenth of a second; the first look after a start still compares
  everyone. Word search no longer copies all its descriptions after a turn.

## 26.42.4 - 2026-10-17

### Added

- **Check people.** People has a Check button: it looks for likely mix-ups
  among the people you named and lists them by kind, each with its faces
  and one button. Someone twice in one photo: the face less like them moves
  out. A face from another age, unlike the person the year it was taken but
  like them years apart: a brother or sister, a child or a parent filed
  under them. Faces much more like another named person: suggested when the
  photo is within a month of theirs, listed as unsure otherwise (to the face
  model newborns look alike). Two named people who may be one, alike and
  never in the same photo or called the same: "Same person". Every change
  has Undo on its card, merges included. Under a second on a catalog of
  108,000 faces, 67,000 of them named.
  `shoebox-cli people-check` gives the same list as JSON; `people-merge`
  now prints what it changed, and `people-unmerge` undoes it.

- **Undo for merges in People.** "Merge into one person" and "Same person"
  (while naming someone in People or on their page) now say in the status
  bar who became one ("Bee is now part of Bea"), with Undo, as moving faces
  does. Undo makes them two people again, each under their own name with
  their own faces; merged from a person's page, it goes back to that page.

- **Leave as is, in Check.** Each card of Check has "Leave as is" for what
  you looked at and kept: someone really twice in a photo (a reflection, a
  collage), look-alikes who are two people. Kept in the catalog, Check
  leaves it out from then on, until it changes: another face of theirs in
  that photo, the face now in someone else or more like someone else.
  "Show N left as is" lists them again, with "Bring back".
  `shoebox-cli people-leave` does the same headless; `people-check --all`
  lists those left as is too.

- **Find more of a person.** A person's page has "Find more…": the faces in
  other people that look more like them than like the rest of the person
  they're in, faces placed by hand included, grouped by the day they were
  taken. The days within a month of the person's own photos start ticked;
  the others are left to check, since to the face model babies look alike.
  The ticked faces move to the person in one step, with Undo.
  `shoebox-cli more-faces <person-id> [--apply]` does the same.

### Fixed

- **New photos tell children apart.** A new face is compared with each
  person as they looked around the date of its photo - their faces from the
  nearest months, up to a year either side - instead of one average over
  their whole life. To the face model a child's baby and school-age faces
  are far apart and all babies look alike, so a newborn often joined an
  older brother or a cousin. Replayed on a copy of a real catalog (108,000
  faces), the faces the user had placed by hand go to the right person 90%
  of the time instead of 75%, and to someone else 8% instead of 23%; for
  babies under one, 89% instead of 69%. Someone not photographed within a
  year, or a photo with no date, is compared with the whole-life average as
  before, and people found twice are joined the same way. In a photo with
  several faces, the most alike face and person go together first. Groups
  already made stay as they are: Regroup faces in People sorts the unnamed
  ones again with the new comparison. Reading everyone's faces at start
  takes about 0.05 s more.
- **"Who is this" and "Find more" tell children apart too.** Both now
  compare a face with each person as they looked around the date of its
  photo, as grouping does. On a copy of a real catalog, "Who is this" shows
  the person the user chose first for 81% of faces placed by hand instead of
  69% (babies under one: 82% instead of 62%), and among the five suggestions
  for 84% instead of 58% of the faces old groups put in someone else.
  "Find more" compares each face with the person it is in around then too:
  after "not them" for a child's faces, it ticks 41% of them instead of 29%,
  and offers a tenth as many faces of other people to check.

## 26.42.3 - 2026-10-16

### Added

- **Catalog memory, and a benchmark for any computer.** Settings > General >
  Catalog memory sets how much of the catalog is read straight from memory;
  Automatic (the default) follows the catalog's size, and Windows gives the
  memory back when other programs need it. `shoebox-cli bench` times what
  the window does most (start-up, the sidebar, People, all photos, a camera,
  a year, a person) on this catalog and computer, and `--cache-mb` /
  `--map-mb` try other values for one run. `shoebox-cli settings` shows and
  changes every setting.

### Fixed

- **Opens at once, however big the catalog.** The window no longer waits
  for word search's descriptions and everyone's faces to be read: they are
  read in the background, and a search or a face waits for them only if
  it comes first. People's faces are read in one pass, about four times
  faster (0.5 s instead of 2.1 s for 108,000 faces). On a catalog of
  190,000 photos, a person's page and People take half the time they did.
- **Camera and lens pages are quick on a big catalog.** Each photo's camera
  and lens names are kept with it instead of being worked out again for
  every photo of every view and filter. On 190,000 photos a camera's page
  takes about 1.1 s instead of 3.6 s, and all photos 1.5 s instead of 2.5 s.
  The names follow renames, merges and splits as before. The first start
  after the update takes a moment more (under half a second for 64,000
  photos) while they are stored.
- **Going back to a view is instant, and a scan no longer floods the
  window.** What the window asks (a view's photos and filter counts, the
  sidebar, the totals, People) is kept until the catalog changes - by
  Shoebox, or by `shoebox-cli` or another program - or a disk comes or
  goes: asked again, it comes back at once (under 25 ms instead of up to
  1.6 s on 190,000 photos). While a scan changes the catalog every few
  seconds, the window never starts a refresh before the last one has
  answered. `shoebox-cli bench` shows both times.
- **The thumbnail size slider stays smooth, and People opens at once.**
  Dragging the slider no longer waits for the grid to be laid out again
  at each step (tens of milliseconds on a big catalog): the grid follows
  when it can. People draws its first few hundred faces, and more as the
  list is scrolled near its end, instead of thousands at once.

- **No more freezes between views.** The window no longer stops drawing
  while it waits for the catalog: its questions run beside the scan and the
  AI instead of behind them, and a new view asks only for its own photos.
  The grid keeps the last view's photos (dimmed if it takes a moment) until
  the new ones are there, instead of showing them under the new title, and
  an answer that comes back for a view already left is dropped. Measured on
  a catalog of 64,000 photos: a person's page opens in about 30 ms instead
  of 40 s, a folder of a hundred photos in a few milliseconds instead of
  0.8 s, and a year's filters in 30 ms instead of 1.6 s. Results are the
  same as before. While faces are found, People and the photos shown still
  fill in every few seconds, without reloading everything else.

## 26.42.2 - 2026-10-16

### Added

- **Photoshop files.** `.psd` and `.psb` files join the catalog: Shoebox
  shows the flattened picture Photoshop saves inside the file (with
  "Maximize compatibility", Photoshop's default) and the original photo's
  date, camera and lens. Saved without "Maximize compatibility", a document
  holds only a white placeholder instead of that picture: Shoebox says so
  ("re-save it with that option on to see it here") rather than show it
  white. A file is recognised by its content, not only its name: a
  Photoshop document saved as `.raw` is read as one. Each folder is
  listed once more when its disk is plugged in, so the Photoshop files
  already there join, and photos an older version couldn't read are tried
  again.
- **Point at a name to see the face.** In the viewer's People list, pointing
  at someone draws a box around their face on the photo and dims the rest.
  At 100% zoom the photo scrolls to the face if it is out of sight.
  `shoebox-cli photo-faces` already gives each face's `bbox`.
- **Go to folder.** Each copy of a photo in the viewer's Location has a
  "Folder" button: it shows that folder in the grid with the photo
  selected, and opens the sidebar tree down to it. It works with the disk
  unplugged, and Back (Esc) returns to the photo. `shoebox-cli photo` gives
  each copy's `folder`, to list it with `shoebox-cli list`.
- **`shoebox-cli people-ignore <person-id>... [--show]`**: ignore strangers
  (or show them again) headless, as the eye in People does. An unknown id
  changes nobody and fails.

### Fixed

- **Previews left blank by a PC that stopped are made again.** A crash or
  power cut right after Shoebox saved a preview, thumbnail or face picture
  could leave that file at its full size but empty (all zeros): a blank
  tile, and word search and faces skipped the photo at every start. Saved
  files now reach the disk before they replace anything. At start, after
  the PC has restarted, Shoebox looks into the files saved since its last
  check: a thumbnail or face picture is made again from the preview, and a
  photo whose preview is damaged or missing is read again from its
  original - right away if its disk is plugged in, otherwise when it is.
  `shoebox-cli check-files [--all]` does the same check headless.
- **A file no reader understands isn't retried at every plug-in.** A photo
  still unreadable when retried after its disk is plugged in (and still
  there) is left alone: a retry is for a disk pulled out mid-read, not a
  file Shoebox can't decode. "Scan again" on its folder tries it again, and
  so will a Shoebox that reads more formats.
- **Someone found twice becomes one person.** A face joins a person only
  when it looks enough like them at that moment, so the same person seen
  from another angle or in other light could end up as two people - and the
  second one stayed in People after the first was named or ignored. Now an
  unnamed person whose average face matches someone else's joins them: under
  their name, hidden with them if they were ignored, or into the bigger of
  two unnamed people. Named and ignored people and faces placed by hand
  never move, and two people seen in the same photo never join. Runs after
  each face scan, after regrouping, and once at start.
- **`shoebox-cli` prints only its JSON on stdout.** Log lines go to stderr
  (and the log file), so a command that also logs something still gives
  output a script can parse.
- **A move, rename or delete cut short is never left half-done.** If Shoebox
  is killed or crashes in the middle of one, the next start finishes it if
  the catalog had already taken it, or puts disk and catalog back as they
  were: a partial copy on the other disk is removed (so the move can simply
  be done again), a renamed folder is followed, files already in the Recycle
  Bin leave the catalog. An original is removed only once its copy has been
  compared byte for byte, checked again after a restart. Whatever needs an
  unplugged disk waits for it; meanwhile scans and other changes leave those
  folders alone. `shoebox-cli` does the same when it starts.
- **Linux downloads.** 26.42.1 had none: the Linux release builds stopped
  at a check of their own (whether `shoebox-cli` is in the .deb) even
  though it was there. The .deb, .rpm, .AppImage and portable .tar.gz are
  built again.

## 26.42.1 - 2026-10-14

### Added

- **The catalog.** Add a folder; Shoebox lists every photo in it - every RAW
  format plus JPEG, HEIC/HIF, TIFF, PNG, WebP, AVIF - and keeps a preview
  (1080 px by default, changeable in Settings) and a grid thumbnail of each,
  in `data/`. Browsing works with the disk unplugged; those photos are
  marked offline, never removed.
- **Disks are known by serial + filesystem + size**, not drive letter, so a
  disk that comes back as another letter is the same disk.
- **Photos are known by content.** A fingerprint (size + BLAKE3 of the first
  and last 256 KB) identifies each photo. A file moved or copied, inside a
  folder or to another disk, is recognised on the next scan and keeps its
  photo, with no re-reading. A file moved *and* rewritten by another app is
  matched by its capture (camera, serial, time to the 1/100 s, extension).
- **Missing means confirmed missing.** Only a complete scan of a plugged-in
  disk can mark a file missing, and only in folders it could read. Missing
  photos stay in the catalog (their own view) and are relinked when found.
- Reading: rawler's embedded full-size JPEG (same rawler revision as
  Argentum), then the Windows codecs (Microsoft RAW extension, which is
  LibRaw, and HEIF), then a full RAW develop. Photos are opened read-only.
- Justified grid by month, viewer with camera details and every copy's
  location (Show in Explorer / Open when the disk is plugged in), live disk
  plug/unplug detection, rename a disk by double-clicking it.
- **Cull after a shoot.** Delete marks a photo (catalog only; in the viewer it
  then moves to the next photo, and Delete again unmarks). Marked photos are
  dimmed and listed under "Marked for deletion". "Delete marked files…" shows
  what would go, disk by disk with a tickbox each, and sends it to the Recycle
  Bin - or says plainly that a memory card or USB stick has none. A RAW takes
  its camera JPEG twin and Argentum's edits with it. Marked photos on
  unplugged disks wait until the disk is back. The only code allowed to delete
  is `organize.rs`, enforced by test.
- **Back** (button or Escape) from a person's page to where you came from,
  including the photo you were looking at.
- **Clean up a person**: finds faces that don't look like the rest of them and
  moves the ones you tick to whoever they resemble, or someone new. **Show all
  faces** lists every face of the person, the least alike first, none ticked,
  so any of them can be picked; **Move to…** sends the ticked ones to someone
  you pick; **Untick all** / **Tick all**. The faces scroll inside the dialog,
  the buttons stay in view. Faces moved out are placed by hand, so "Regroup
  faces" never brings them back.
- **Who is this?** - the other half of merge: take one photo's face out of a
  person and say who it is. On a person's page, select photos and click
  **Not <name>…**; in the viewer, click the icon next to a face (or "Who is
  this?" on a face too small for Shoebox to recognise). Pick someone from the
  people that face looks like, find them by name, type a new name, or choose
  "don't know who" (it joins whoever it resembles most, never the person it
  left). Your choice sticks: regrouping never moves the face back. A person
  stays once per photo: if they were already in it, that other face goes to
  whoever it resembles, and the message says who. Undo in the status bar puts
  every face changed back as it was. Headless: `shoebox-cli photo-faces`,
  `person-faces`, `look-alikes`, `move-faces`, `restore-faces`.
- **Keyboard in the sidebar**: arrows move and open/close folders, Enter shows,
  Home/End, F2 renames, the Menu key opens a folder's menu.
- **Select in the grid** as in Explorer: click selects, double-click or Enter
  opens, Ctrl+click and Shift+click, arrows / Shift+arrows / Page Up-Down /
  Home-End, Ctrl+A. Delete marks or unmarks everything selected. The status bar\r
  shows the selected photo's name, type, date, resolution and size. A toolbar\r
  button puts the file type (CR3, JPG, TIF...) on every thumbnail.
- **Oldest first**, everywhere: grid, folders (A-Z, numbers by value) and
  dates; bursts follow the camera's 1/100 s. A sort menu switches to newest
  first, file name or recently added.
- **Folder menu** on right-click: Show in Explorer, New folder, Rename (on the
  disk and in the catalog at once), Scan again. The browser's own menu is gone.
- **Drag a folder onto another** to move it, photos and everything else in it.
  On the same disk it's instant and can be undone from the status bar. To
  another disk Shoebox asks first, copies, checks every byte, then removes the
  original; Stop or any failure leaves the original untouched. The catalog
  follows; nothing is read again.
- **People: named, to name, ignored.** Three groups that fold away (Ignored starts folded; no separate view any more). Naming someone (under their face, or by clicking the title of their page) suggests the people already named (face, name, photos); tick **Same person** to merge into them - a matching name alone never merges, two people can share a name.
- **People in the sidebar says what's new**: a yellow number next to People, like the ones on the faces, when faces found since you last looked add someone new, or add photos to someone you know. It counts the people People shows (not the ignored, not those seen only once), updates while faces are found, and goes down as you read their pages.
- **Find a person by name** on People: the toolbar's search box becomes "Find a person by name…". Names are suggested as you type (face, name, photos) and the cards below narrow to the matches; any part of the name works, in any order ("smi jan" finds Jane Smith), case and accents ignored ("zoe" finds Zoé). ↑↓ and Enter open someone, Esc clears; the search is still there when you come back from their page. Headless: `shoebox-cli people [name]` (JSON, best match first).
- **Only new, to check what was found**: on People, **Only new** keeps just the people who are new or were found in new photos (clicking the yellow number in the sidebar opens People that way). Open one and you see only their new photos; once you leave their page they're read and drop off the list, so you work through it. On a person's page, **Only new** does the same for their photos (55 of 768 instead of all of them). Headless: `shoebox-cli people --new`, and `shoebox-cli list` takes a scope: `{"kind":"person","id":42,"newOnly":true}` (each photo says whether it is new).
- **Tags**: your own keywords on photos - T in the grid (one or many selected) or in the viewer; suggestions as you type. Tags stay with the photo through moves and copies, live in the catalog only, and filter in the bar (Tags: photos with all ticked tags).
- **Filters**: one bar above the grid - People (with their faces), Date (years, months, days), Camera,
  Lens, File type (RAW and Images, each type with its count), Status (plugged
  in / unplugged / missing). Tick any values; filters combine, narrow whatever
  is shown (search included) and stay on from folder to folder. Counts follow
  the other filters. Replaces the Dates and Cameras sections of the sidebar.
- **Viewer fills the screen**, sharp when the disk is plugged in (a picture from the original sized to the screen swaps in under a second); click or Z zooms to 100%, drag to look around. A **filmstrip** under the photo.
- **Sources** instead of Disks: each folder you add is the top of its tree.
  Disks mounted in a folder (`C:\Mounts\Disk1`) are recognised, not
  just drive letters.

- **Moving the catalog takes seconds on the same disk**: previews, thumbnails and faces move as whole folders, the way Explorer moves them; the databases are copied consistently; anything failing puts it all back. To another disk it still copies. Once Shoebox has started on the new folder, the old one goes to the Recycle Bin (only the catalog's own files: an installed Shoebox's models and `shoebox.json` stay); if its disk is unplugged, at a later start.
- **Backup, Kopia-ready**: `shoebox-cli backup` (or Settings > Make backup copies now) writes consistent copies of the databases into `backup` while Shoebox runs, and a `.kopiaignore` that skips the live ones. Run it as Kopia's before-snapshot action; daily snapshots only add new previews plus a few MB.
- **Settings > AI models**: which AI models Shoebox can use (word search, faces, and the AI runtime for the graphics card or the processor), whether each is here, and its size. **Download** fetches only what is missing, from the original publishers (Immich's Hugging Face pages, Microsoft), and keeps a file only if its SHA-256 matches; Stop at any time. **Delete** frees the space; what was already found (descriptions, faces, people) stays. Models copied in for later features can be deleted too. Word search and People say when their model is missing and open the tab. Headless: `shoebox-cli models`, `models download <id>`, `models delete <id>`, `models verify [id]`.
- **Date rail** on the grid's right edge, in place of the scrollbar: the years, placed where their photos are (a busy year takes more room). Drag along it to fly through the years; a bubble names the month you'll land on, and shows for a moment whenever you scroll.
- **Calendar** (toolbar button, Esc to leave): every year as a grid of days, shaded by how many photos were taken that day, with the scope, filters and search applied (People + Calendar = the days you photographed someone). Hover a day to see a few of its photos; click a day, a month or a year to go there in the grid. Headless: `shoebox-cli days [filters]`.
- **Gear: one name per camera and lens.** Files name the same gear in different ways ("EF70-200mm f/4L IS USM" in the camera's JPEG, "Canon EF 70-200mm f/4L IS USM" from the RAW; "XF23mmF1.4 R" / "Fujifilm XF 23mm F1.4 R"; "NIKON D850" / "Nikon D850"): names that differ only by the maker's name, case, spaces or "f/4" vs "F4" are now one, under the fullest spelling. Gear in the sidebar lists cameras and lenses with counts; click one for its photos, rename it (F2; another one's name merges the two), merge it ("Same lens as…"), or split it back into the names in the files. Your names stick, and new photos, rescans and new spellings join them. Catalog only, never written into the photos. Headless: `shoebox-cli gear`, `gear-rename`, `gear-split`.
- **Lenses that showed as unknown are found**: Shoebox reads the lens where each format keeps it (EXIF, CR3 boxes, a Fuji RAF's JPEG, Canon's maker note and lens id, CRW), instead of relying on rawler's lens table, which gave up on lenses sharing an id (the EF 135mm f/2L). Photos already in the catalog are read again in the background, metadata only (1-2 ms each), from plugged-in disks; a RAW without a lens takes its JPEG twin's. Headless: `shoebox-cli find-lenses`, `shoebox-cli lens <file>`.
- **The lens, for any camera.** One reader for every container (RAW, DNG, CR3, RAF, CRW, JPEG/HEIC): the lens's name where the file has one (EXIF LensModel; the maker's note: Canon's name or id, Pentax's id - also from the note a DNG keeps), else rawler's lens table, else what the lens is: its focal range and widest apertures, from the standard EXIF LensSpecification / DNG LensInfo or the maker's note (Canon's settings, Nikon's lens tag) - "24-70mm f/2.8", "18-55mm f/3.5-5.6". Such a name joins the catalog's lens of that range and aperture when exactly one fits (a Canon and a Tamron 24-70 f/2.8 both fitting: no guess; Lightroom's "24.0-70.0 mm f/2.8" joins the same way); an aperture no real lens has (a lens reporting nonsense) is dropped. Checked on files from 12 cameras of 9 makers. The 10D, which records no lens id, now has its lenses. Photos already read are read again by themselves, metadata only.
- **A new version is announced** while Shoebox runs: it asks GitHub for the latest published release 15 s after start and every 6 hours (only a release counts, never a commit on a branch). A new one shows in the status bar once and stays at the bottom of the sidebar; click to open its release page. Nothing is downloaded or installed by itself. Settings > General > Updates: the latest release, **Check now**, and a switch to stop asking. Headless: `shoebox-cli version`.
- **Fold the sidebar** to a narrow rail of icons: the button at the end of the Settings row, or Ctrl+B. All photos, People (with its count of new people), Missing and Marked stay one click away, names and counts in their tooltips; Sources and Gear unfold the sidebar on their section. Remembered between starts.

### Internal

- Renamed from OtO (working name) to Shoebox: window, app id
  `co.shoebox.catalog`, `shoebox-cli`, `SHOEBOX_DATA`, `shoebox.json`.
- `shoebox-cli` (add | scan | stats | photo) runs the catalog without the
  window; `SHOEBOX_DATA` points it at a throwaway catalog. Culling and
  people too: `mark`, `unmark`, `marked` (the plan, with a token),
  `delete-marked <token>`, `person-strays <id> [--apply]`.
- Models for search, faces and text copied from Immich's install and the
  ONNX runtime from Argentum into `models/`. The SHA-256 of every file
  Shoebox can download is pinned in `models.rs`: Hugging Face's own for
  the models, and for ONNX Runtime 1.22.0 those of Microsoft's packages and
  of the library inside, for each system (Windows, macOS, Linux; Intel/AMD
  and ARM). Downloads are written only inside the models folder (write
  guard in `paths.rs`, tested). An AI runtime downloaded while Shoebox runs
  is picked up without a restart. `SHOEBOX_MODELS` points at a throwaway
  models folder.
- Release builds for Windows, macOS and Linux, each on Intel/AMD and ARM,
  made on GitHub only when a release is published, as in Argentum:
  installers (Windows setup .exe, per user, no admin prompt; macOS .dmg;
  Linux .deb / .rpm / .AppImage) and a portable Shoebox folder, all with
  the app and `shoebox-cli`. Installed, Shoebox keeps its catalog and
  `shoebox.json` in `~/.shoebox` and its AI models in the install folder
  (`~/.shoebox` on macOS and Linux, whose install folders can't be
  written); portable, all of it next to the app as before. On macOS and
  Linux it opens and browses but can't add disks yet; Show in Explorer /
  Open use Finder and the desktop's file manager
  there.

## Versioning: `yy.isoWeek.release`

```
26.42.1
 |  |  +-- release within that week, counting from 1
 |  +----- ISO week number
 +-------- ISO year, two digits
```

Identical to Argentum (see its CHANGELOG for the full reasoning): the version
says when it was built; two digits because Windows installers cap the major
version at 255; **bump immediately after releasing, never before**, so a
number one ahead of the last release means "built from source". The release
commit sets the true `yy.ww.n`. Current week: `date +%g.%V`.

The version lives in three places that must match: `package.json`,
`src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`.
