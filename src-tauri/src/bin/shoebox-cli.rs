//! Headless Shoebox, for testing the catalog without the window.
//! Set SHOEBOX_DATA to use a throwaway catalog folder.
//!
//!   shoebox-cli add <folder>     add a folder and scan it
//!   shoebox-cli scan [root-id]   rescan one folder, or every plugged-in one
//!   shoebox-cli retry [disk-id]  read again only the files that couldn't be read (or were left
//!                                half-read by a stopped scan), on one plugged-in disk or all of
//!                                them (the window does it at start and when a disk is plugged in).
//!                                A photo still unreadable when retried is left alone after:
//!                                `scan <root-id>` tries it again
//!   shoebox-cli check-files [--all]   previews, thumbnails and face pictures written since the
//!                                last check (--all: every one) that a PC stopping mid-write left
//!                                damaged or missing: thumbnails and face pictures are made again
//!                                from the preview, photos whose preview is damaged marked to be
//!                                read again - `retry` reads them (JSON; the window checks at
//!                                start after the PC has restarted)
//!   shoebox-cli stats            catalog overview
//!   shoebox-cli settings [key=value ...]   the settings, as Settings in the window, changed when
//!                                given (JSON out), e.g. catalogMapMb=1024; 0 = automatic for the
//!                                catalog memory, whose values in use come out as "memory"
//!   shoebox-cli bench [--cache-mb N] [--map-mb N|off]   times what the window does most on this
//!                                catalog and computer (best of 3, ms; JSON out): start-up, the
//!                                sidebar, People, all photos, a camera, a year, a person. Reads only.
//!                                The flags try other catalog memory for this run only
//!   shoebox-cli version          this version and the latest published release (JSON; asks
//!                                GitHub - a commit on a branch never counts)
//!   shoebox-cli logs [lines]     the log folder, its files newest first (a log a day, everything
//!                                there removed after 14 days) and the newest log's last lines
//!                                (50 unless given) (JSON)
//!   shoebox-cli photo <id>       one photo's details; each copy's volumeId and folder make the
//!                                scope of its folder ("Go to folder") for list:
//!                                {"kind":"folder","volumeId":3,"relPath":"<folder>"}
//!                                "problems": what Cull found wrong with it, once measured (blurry,
//!                                focus missed...): a camera that confirmed focus says nothing of shake
//!   shoebox-cli backup           consistent copies of the databases in backup\ (run it
//!                                before each Kopia snapshot; see .kopiaignore)
//!   shoebox-cli list [oldest|newest|name|added] [limit] [filters] [scope]   photos in grid order
//!                                (JSON); scope = JSON as the window's, e.g. a person's photos found
//!                                since the user last looked: {"kind":"person","id":42,"newOnly":true}
//!   shoebox-cli facets [filters]  what each filter offers, with counts (JSON)
//!   shoebox-cli days [filters]    photos taken each day, as the calendar shows them (JSON)
//!       filters = JSON, any of {"dates":["2026-09-26"],"cameras":[..],"lenses":[..],
//!                               "types":["cr3"],"status":["online","offline","missing"],
//!                               "people":[42,43],"peopleMatch":"any"|"all" (default any;
//!                               all = only photos with them together),
//!                               "tags":[3,7],"tagsMatch":"all"|"any" (default all)}
//!
//! People (JSON out):
//!   shoebox-cli people [name] [--new]   everyone, or whoever's name matches, best first, as
//!                                 People's search box finds them (any part of the name, accents
//!                                 ignored); --new: only those new or in new photos ("Only new")
//!   shoebox-cli photo-faces <photo-id>...     the faces in photos, and who each is
//!   shoebox-cli person-faces <person-id>      a person's faces, the least like them first
//!   shoebox-cli look-alikes <face-id>...      who those faces look like most (to suggest a name)
//!   shoebox-cli move-faces <face-id>... --to <person-id> | --new ["name"] | --elsewhere
//!       "who is this": placed by hand, never regrouped back. --elsewhere = not them, don't
//!       know who: whoever they resemble, or someone new. A person stays once per photo:
//!       a face already theirs in that photo makes room
//!   shoebox-cli restore-faces '<undo JSON>'   undo a move-faces (its "undo" output)
//!   shoebox-cli person-strays <person-id> [--apply]   Clean up: faces unlike the person
//!   shoebox-cli more-faces <person-id> [--apply]   Find more: faces in other people that look more
//!       like this person than like the person they are in (placed by hand too), by day, the days
//!       within 31 days of their own photos marked "near"; --apply moves the near ones to them
//!       (placed by hand; its "undo" output goes to restore-faces)
//!   shoebox-cli person-rename <person-id> ["name"]    name someone (no name: unnamed again).
//!       A name already taken makes two people called the same, never one
//!   shoebox-cli people-merge <person-id>... --into <person-id>   "Same person": they become
//!       one, keeping --into's name (or the first name among them); its output goes to
//!       people-unmerge to undo it
//!   shoebox-cli people-unmerge '<merge JSON>'   undo a people-merge: they are apart again
//!   shoebox-cli people-check [--all]   Check: likely mix-ups among named people (nothing changes).
//!       "twice": someone twice in one photo ("elsewhere": the faces to move out with
//!       move-faces --elsewhere); "ageJumps": faces unlike the person that year but like them
//!       3+ years apart (a sibling, child or parent filed under them: --elsewhere);
//!       "likeOthers": faces much more like another named person ("otherId"), "near" when the
//!       photo is within 31 days of one of theirs (move-faces --to them; the others are unsure:
//!       newborns look alike); "samePeople": two named people alike and never in the same
//!       photo, or called the same (people-merge <otherId> --into <personId> when "together"
//!       is 0). A face is in one finding at most. Findings left as is (people-leave) are left
//!       out, "left" says how many; --all lists them too, with "left": true
//!   shoebox-cli people-leave '<finding JSON>'... [--back]   "Leave as is": looked at, and right
//!       as it is (a reflection, a collage, two look-alikes): people-check leaves it out until
//!       what it is about changes (another face of theirs in that photo; the face in someone
//!       else, or more like someone else). --back: in the check again. Each finding is one of
//!       {"kind":"twice","personId":..,"photoId":..,"faces":[<its faceIds>]}
//!       {"kind":"ageJump","personId":..,"faceId":..}
//!       {"kind":"likeOther","personId":..,"faceId":..,"otherId":..}
//!       {"kind":"samePerson","personId":..,"otherId":..}   (or a JSON list of them)
//!   shoebox-cli people-ignore <person-id>... [--show]   strangers: hidden from People, their
//!       future faces still joining them (--show: shown again)
//!
//! People found since the user last looked at them show "(new)" / "(+N new photos)" in
//! `shoebox-cli faces`; the window marks a person read when the user leaves their page:
//!   shoebox-cli people-read <person-id>... | --all | --group named|to-name|once|ignored
//!       (--group: the people of one People group, "once" = seen in one photo only)
//!
//! Tags (catalog only, never written into photos):
//!   shoebox-cli tags [photo-id...]              every tag, or those on some photos
//!   shoebox-cli tag "<name>" <photo-id>...      add (made if new)
//!   shoebox-cli untag "<name>" <photo-id>...
//!   shoebox-cli tag-rename "<old>" "<new>"      onto an existing name = merge
//!
//! Gear: cameras and lenses, one name each (catalog only, never written into photos; JSON out).
//! Spellings of the same gear ("EF70-200mm f/4L IS USM" = "Canon EF 70-200mm f/4L IS USM")
//! are grouped by themselves; names given here win, and later photos join them:
//!   shoebox-cli gear                                  every camera and lens, with the names in their files
//!   shoebox-cli gear-rename camera|lens "<name>" "<new>"   onto an existing name = merge
//!   shoebox-cli gear-split camera|lens "<name>"       back to the names in the files, kept apart
//!   shoebox-cli find-lenses       photos without a lens: read again from their originals
//!                                 (metadata only, plugged-in disks), or from the same shot's other file
//!   shoebox-cli lens <file>       the lens Shoebox reads in a file
//!   shoebox-cli find-focus        photos read before Shoebox knew their camera's focus points:
//!                                 read again from their originals (metadata only, plugged-in disks)
//!   shoebox-cli focus <file>      where the camera focused, as Shoebox reads it in a file: each
//!                                 point a fraction of the upright picture (x, y its top-left
//!                                 corner), "inFocus", "chosen", "area" (the zone given) or
//!                                 "other" (the camera's other points); also in `photo <id>`
//!
//! Private photos (JSON out). Every password is its own private space: no
//! password is wrong, a new one starts an empty space. Its photos are hidden from
//! everything Shoebox shows until that password opens it in the window; Shoebox's
//! copies of them (previews, thumbnails, face pictures) are kept sealed. Agents can
//! make photos private, never see them: no command here lists, counts or opens one.
//!   shoebox-cli private [--new] <photo-id>... | --in '<scope JSON>'   make photos private
//!                                              in the space of the password read from the
//!                                              first line of stdin (those of a view with --in,
//!                                              e.g. a folder); --new: a password with no
//!                                              private photos yet (a typo would start a space)
//!   shoebox-cli private --name '<name>'          names the space of the password read from
//!                                              stdin, shown next to Private in the window
//!                                              while it is open ('' = no name); it must have
//!                                              private photos (the name is kept with them)
//!
//! Folders (on the disk and in the catalog in one step; JSON out):
//!   shoebox-cli rename-folder <volume-id> <folder> <new name>
//!   shoebox-cli new-folder <volume-id> <parent folder> [name]
//!   shoebox-cli move-folder <volume-id> <folder> <to-volume-id> <into folder>
//!       same disk: moved at once; another disk: copied, checked byte for byte, original removed
//!   A move, rename or delete cut short (a kill, a crash) is finished or undone by the next
//!   start of shoebox-cli or the window, as far as the disks plugged in allow (told on stderr).
//!
//! AI models (JSON out; as Settings > AI models):
//!   shoebox-cli models                    which models Shoebox can use, and which are here
//!   shoebox-cli models download <id>      fetch what is missing from its publisher, checked by SHA-256
//!   shoebox-cli models delete <id>
//!   shoebox-cli models verify [id]        hash the files here against the pinned SHA-256
//!
//! Culling (JSON out). Marking is catalog-only; files are deleted only by
//! `delete-marked` quoting the token of the plan a person was shown:
//!   shoebox-cli mark <photo-id>...             mark for deletion
//!   shoebox-cli unmark <photo-id>... | --all
//!   shoebox-cli marked                         what deleting would remove now, by disk, with its token;
//!                                              "gone": marked photos already missing everywhere
//!   shoebox-cli delete-marked <token> [volume-id...]
//!                                              delete it (the system's bin: Recycle Bin, Trash;
//!                                              removable media for good), on the listed disks or every
//!                                              disk in the plan; the "gone" ones only leave the catalog
//!
//! Turning photos (JSON out). Only the catalog and Shoebox's own copies change, never a file:
//!   shoebox-cli turn right|left|half|back <photo-id>...   a quarter turn clockwise or
//!                                              counter-clockwise, half a turn, or back to how
//!                                              the file shows it. Faces keep their names; a photo
//!                                              turned is never suggested as sideways again
//!   shoebox-cli sideways                       photos the upright model would turn, the surest first
//!                                              ("turn": quarter turns clockwise, "sure": 0..1)
//!   shoebox-cli upright-check [--again]        look at the photos not looked at yet (the window does
//!                                              it in the background once its model is here);
//!                                              --again: every photo nobody decided about
//!   shoebox-cli sideways-accept <photo-id>... | --all   turn them the way the model suggests
//!   shoebox-cli sideways-leave <photo-id>... | --all    fine as they are: never suggested again
//!   shoebox-cli upright-eval <folder> [limit]  how the model does on upright JPEGs in a folder,
//!                                              each shown to it four ways (reads only)
//!
//! Missing photos (JSON out): no copy found on any disk, plugged in or not.
//!   shoebox-cli missing                        each one, where its files were, and whether they are
//!                                              in the system's bin (deleted, not moved: restoring
//!                                              them brings the photo back with its faces and tags)
//!   shoebox-cli forget-missing <photo-id>... | --in-bin | --all
//!                                              take them out of the catalog (no file is touched);
//!                                              photos with a copy anywhere stay
//!
//! Cull's assistant (JSON out). It only suggests, keep or reject; a reject is a mark
//! for deletion as above (`mark` / `unmark`), and nothing touches a file:
//!   shoebox-cli quality                   measure every photo of the catalog not measured yet,
//!                                         from its preview (`cull` and the window measure only
//!                                         the shoot they are asked about)
//!   shoebox-cli measure <photo-id>...     a photo's measures (taken first if it has none yet)
//!                                         and what's wrong with it. Blur is
//!                                         a Gaussian sigma in px of the picture measured
//!                                         ("long" px wide); "close" = measured from the original
//!   shoebox-cli cull [filters] [scope] [--closer]   a view as a shoot, as Cull shows it (up to
//!                                         5,000 photos: a folder or a day): bursts gathered in
//!                                         stacks, each frame's problems, and Cull's suggestion
//!                                         (keep, reject, none) with the reasons; "keeper" is the
//!                                         sharpest frame, "compared" where: "face", else
//!                                         "focusPoint" (where the camera focused, when it is sharp
//!                                         in one frame at least), else "sharpest"; "focus" is where
//!                                         to look at 100% first: on "focusPoints", where the
//!                                         camera focused (when its file says), else on the
//!                                         largest face; "focusMissed" = soft there while
//!                                         something else is sharp. --closer first
//!                                         looks closer at the originals plugged in (read at half
//!                                         size, read-only), which shows a photo only soft at 100%;
//!                                         originals it couldn't read are listed under "unreadable", with why
//!   shoebox-cli cull-accept [filters] [scope]   mark for deletion what Cull suggests rejecting
//!                                         (frames not marked yet)

use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{self, Progress};
use shoebox_lib::{cull, focus, gear, organize, tags, upright};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    shoebox_lib::volumes::quiet_errors();
    let data = DataDir::locate();
    data.ensure()?;
    shoebox_lib::setup_logging(&data);
    shoebox_lib::undo_unfinished_move(&data);
    let cat = Catalog::open(data)?;
    shoebox_lib::tidy_old_catalog(&cat.data);
    scan::heal_saved_previews(&cat)?;
    // Turns a stop cut short: previews brought to the turn the catalog says.
    shoebox_lib::turn::settle(&cat)?;
    // A folder move, rename or delete cut short (a kill, a crash): finished or
    // undone first, as the window does at start. Told on stderr.
    let resumed = organize::resume(&cat)?;
    if !resumed.is_empty() {
        eprintln!("changes cut short earlier: {}", serde_json::to_string(&resumed)?);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cancel = AtomicBool::new(false);
    let show = |p: &Progress| {
        eprintln!("  [{}] {} found {} read {}/{} {}", p.root_name, p.phase, p.found, p.read, p.to_read, p.current)
    };

    match args.first().map(String::as_str) {
        Some("add") => {
            let folder = args.get(1).ok_or_else(|| anyhow::anyhow!("add <folder>"))?;
            let id = cat.add_root(Path::new(folder))?;
            let report = scan::scan_root(&cat, id, &cancel, false, &show)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Some("scan") => {
            let ids = match args.get(1) {
                Some(id) => vec![id.parse()?],
                None => cat.root_ids()?,
            };
            for id in ids {
                match scan::scan_root(&cat, id, &cancel, args.get(2).is_some_and(|a| a == "--regenerate"), &show) {
                    Ok(report) => println!("{}", serde_json::to_string_pretty(&report)?),
                    Err(e) => println!("folder {id}: {e:#}"),
                }
            }
        }
        Some("retry") => {
            let ids = match args.get(1) {
                Some(id) => vec![id.parse()?],
                None => cat.online_ids(),
            };
            for id in ids {
                if scan::failed_count(&cat, id)? > 0 {
                    println!("{}", serde_json::to_string_pretty(&scan::retry_failed(&cat, id, &cancel, &show)?)?);
                }
            }
        }
        Some("check-files") => {
            let all = args.get(1).is_some_and(|a| a == "--all");
            println!("{}", serde_json::to_string_pretty(&shoebox_lib::repair::check_files(&cat, all)?)?);
        }
        Some("stats") => {
            println!("{}", serde_json::to_string_pretty(&cat.overview()?)?);
            let list = cat.list_photos(&Scope::All, Sort::default(), &Filters::default())?;
            let failed = list.preview.iter().filter(|&&s| s == 2).count();
            println!("photos listed: {}, without preview: {failed}", list.ids.len());
        }
        Some("settings") => {
            let mut changed = serde_json::to_value(cat.settings())?;
            let known = changed.as_object().unwrap().keys().cloned().collect::<Vec<_>>().join(", ");
            for kv in &args[1..] {
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("settings [key=value ...], e.g. catalogMapMb=1024"))?;
                let o = changed.as_object_mut().unwrap();
                if !o.contains_key(k) {
                    anyhow::bail!("no setting {k:?} (settings: {known})");
                }
                o.insert(k.into(), serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.into())));
            }
            if args.len() > 1 {
                cat.save_settings(serde_json::from_value(changed)?)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({ "settings": cat.settings(), "memory": cat.memory() })
                )?
            );
        }
        Some("bench") => bench(&cat, &args[1..])?,
        Some("version") => {
            println!("{}", serde_json::to_string_pretty(&shoebox_lib::updates::check(None))?);
        }
        Some("logs") => {
            let lines = match args.get(1) {
                Some(n) => n.parse().map_err(|_| anyhow::anyhow!("logs [lines]"))?,
                None => 50,
            };
            let logs = serde_json::json!({
                "folder": cat.data.log_dir(),
                "keptDays": shoebox_lib::paths::LOGS_KEPT_DAYS,
                "files": cat.data.log_files(),
                "lines": cat.data.log_tail(lines)?,
            });
            println!("{}", serde_json::to_string_pretty(&logs)?);
        }
        Some("backup") => {
            // Consistent copies of the databases in backup\ - Kopia's before-snapshot action.
            println!("{}", serde_json::to_string_pretty(&cat.backup()?)?);
        }
        Some("move") => {
            let folder = args.get(1).ok_or_else(|| anyhow::anyhow!("move <folder>"))?;
            let check = cat.check_catalog_folder(Path::new(folder))?;
            println!("{}", serde_json::to_string_pretty(&check)?);
            if check.problem.is_none() && !check.has_catalog {
                cat.move_catalog(Path::new(&check.target), &|d, t| eprint!("\r  copied {d}/{t}"))?;
                println!("\nmoved to {}", check.target);
            }
        }
        Some("index") => {
            let index = shoebox_lib::search::SearchIndex::open(&cat)?;
            let gpu = shoebox_lib::ai::runtime()?;
            eprintln!("AI runtime: {}", if gpu { "GPU (DirectML)" } else { "CPU" });
            let t = std::time::Instant::now();
            index.index_pending(&cat, &cancel, &|p| eprint!("\r  described {}/{} gpu={}", p.done, p.total, p.gpu))?;
            println!("\n{} photos indexed, {:.1} s", index.indexed(), t.elapsed().as_secs_f64());
        }
        Some("search") => {
            let q = args[1..].join(" ");
            let index = shoebox_lib::search::SearchIndex::open(&cat)?;
            let t = std::time::Instant::now();
            let (hits, clear) = index.search(&q, 1000, &cat.hidden_ids()?)?;
            let n_clear = hits.iter().filter(|h| h.1 >= clear).count();
            println!("{n_clear} clear + {} possible for {q:?} in {} ms", hits.len() - n_clear, t.elapsed().as_millis());
            let hits: Vec<_> = hits.into_iter().take(8).collect();
            for (id, score) in hits {
                let d = cat.photo_detail(id)?;
                println!("  {score:.3}  #{id}  {}", d.files.first().map(|f| f.name.as_str()).unwrap_or("?"));
            }
        }
        Some("faces") => {
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            let t = std::time::Instant::now();
            faces.index_pending(&cat, &cancel, &|p| eprint!("\r  looked at {}/{} gpu={}", p.done, p.total, p.gpu))?;
            let people = faces.people(&cat)?;
            let total: i64 = people.iter().map(|p| p.faces).sum();
            println!("\n{total} faces, {} people, {:.1} s", people.len(), t.elapsed().as_secs_f64());
            for p in people.iter().take(15) {
                let new = match (p.is_new, p.new_photos) {
                    (true, _) => " (new)".to_string(),
                    (false, 0) => String::new(),
                    (false, n) => format!(" (+{n} new photos)"),
                };
                println!(
                    "  person {:>4}: {:>3} faces in {:>3} photos  {:?}{}{new}",
                    p.id,
                    p.faces,
                    p.photos,
                    p.name,
                    if p.ignored { " (ignored)" } else { "" }
                );
            }
        }
        Some("people") => {
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            let new_only = args[1..].iter().any(|a| a == "--new");
            let name = args[1..].iter().filter(|a| *a != "--new").cloned().collect::<Vec<_>>().join(" ");
            let mut people =
                if name.trim().is_empty() { faces.people(&cat)? } else { faces.find_people(&cat, &name)? };
            if new_only {
                people.retain(|p| p.has_news());
            }
            println!("{}", serde_json::to_string_pretty(&people)?);
        }
        Some("people-read") => {
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            let n = if args.get(1).is_some_and(|a| a == "--all") {
                faces.mark_all_read(&cat)?
            } else if args.get(1).is_some_and(|a| a == "--group") {
                let group = args
                    .get(2)
                    .and_then(|g| shoebox_lib::faces::PeopleGroup::parse(g))
                    .ok_or_else(|| anyhow::anyhow!("people-read --group named|to-name|once|ignored"))?;
                faces.mark_group_read(&cat, group)?
            } else {
                let ids = args[1..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
                if ids.is_empty() {
                    anyhow::bail!("people-read <person-id>... | --all | --group named|to-name|once|ignored");
                }
                faces.mark_read(&cat, &ids)?
            };
            println!("{}", serde_json::json!({ "facesRead": n }));
        }
        Some("faces-reset") => {
            // Test catalogs only: forget every face, person and name.
            if std::env::var_os("SHOEBOX_DATA").is_none() {
                anyhow::bail!("faces-reset only runs on a test catalog (set SHOEBOX_DATA)");
            }
            // Faces, people and "looked at" marks go together or not at all. The
            // fingerprints go after: left behind by a stop, they are only unused.
            {
                let db = cat.db();
                let tx = db.unchecked_transaction()?;
                tx.execute_batch(
                    "DELETE FROM faces; DELETE FROM people; DELETE FROM face_scan; DELETE FROM check_left;",
                )?;
                tx.commit()?;
            }
            rusqlite::Connection::open(cat.data.root.join("faces.db"))?.execute_batch("DELETE FROM emb;")?;
            println!("faces reset");
        }
        Some("person") => {
            // Faces of one person: shoebox-cli person <id>
            let pid: i64 = args.get(1).ok_or_else(|| anyhow::anyhow!("person <id>"))?.parse()?;
            let db = cat.db();
            let mut stmt = db.prepare(&format!(
                "SELECT id, photo_id, round(x1,3), round(y1,3), round(x2,3), round(y2,3), round(score,3) FROM faces
                  WHERE person_id = ?1 AND {} ORDER BY photo_id LIMIT 12",
                cat.shown_photo_sql("photo_id")
            ))?;
            let rows = stmt.query_map([pid], |r| {
                Ok(format!(
                    "face {:>4} photo {:>4} box {:.3},{:.3}-{:.3},{:.3} score {:.3}",
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, f64>(6)?
                ))
            })?;
            for r in rows {
                println!("{}", r?);
            }
        }
        Some("person-strays") => {
            // Faces that don't look like the person: shoebox-cli person-strays <id> [--apply]
            let pid: i64 = args.get(1).ok_or_else(|| anyhow::anyhow!("person-strays <id> [--apply]"))?.parse()?;
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            let strays = faces.strays(&cat, pid)?;
            if args.get(2).is_some_and(|a| a == "--apply") {
                let ids: Vec<i64> = strays.iter().map(|s| s.face_id).collect();
                let moved = faces.reassign(&cat, &ids, &shoebox_lib::faces::FaceTarget::Elsewhere)?;
                println!("{}", serde_json::json!({ "moved": moved.faces }));
            } else {
                println!("{}", serde_json::to_string_pretty(&strays)?);
            }
        }
        Some("more-faces") => {
            // Faces elsewhere that may be the person: shoebox-cli more-faces <id> [--apply]
            let pid: i64 = args.get(1).ok_or_else(|| anyhow::anyhow!("more-faces <person-id> [--apply]"))?.parse()?;
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            let days = faces.more_of(&cat, pid)?;
            if args.get(2).is_some_and(|a| a == "--apply") {
                let ids: Vec<i64> =
                    days.iter().filter(|d| d.near).flat_map(|d| d.faces.iter().map(|f| f.face_id)).collect();
                let to = shoebox_lib::faces::FaceTarget::Person { id: pid };
                println!("{}", serde_json::to_string_pretty(&faces.reassign(&cat, &ids, &to)?)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&days)?);
            }
        }
        Some("person-rename") => {
            let usage = || anyhow::anyhow!("person-rename <person-id> [\"name\"]");
            let pid: i64 = args.get(1).ok_or_else(usage)?.parse()?;
            let name = args[2..].join(" ");
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            faces.rename(&cat, pid, &name)?;
            let name = name.trim();
            println!("{}", serde_json::json!({ "person": pid, "name": (!name.is_empty()).then_some(name) }));
        }
        Some("people-merge") => {
            let usage = || anyhow::anyhow!("people-merge <person-id>... --into <person-id>");
            let flag = args.iter().position(|a| a == "--into").ok_or_else(usage)?;
            let into: i64 = args.get(flag + 1).ok_or_else(usage)?.parse()?;
            let ids = args[1..flag].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if ids.is_empty() {
                return Err(usage());
            }
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            println!("{}", serde_json::to_string_pretty(&faces.merge(&cat, &ids, into)?)?);
        }
        Some("people-unmerge") => {
            // Undo of people-merge: shoebox-cli people-unmerge '<its JSON>'
            let json = args.get(1).ok_or_else(|| anyhow::anyhow!("people-unmerge '<JSON from people-merge>'"))?;
            let merged: shoebox_lib::faces::Merged = serde_json::from_str(json)?;
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            println!("{}", serde_json::json!({ "facesBack": faces.unmerge(&cat, &merged)? }));
        }
        Some("people-check") => {
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            let t = std::time::Instant::now();
            let check = faces.check_people(&cat, args[1..].iter().any(|a| a == "--all"))?;
            eprintln!("checked {} faces of {} people in {} ms", check.faces, check.people, t.elapsed().as_millis());
            println!("{}", serde_json::to_string_pretty(&check)?);
        }
        Some("people-leave") => {
            // shoebox-cli people-leave '<finding JSON>'... [--back]
            let back = args[1..].iter().any(|a| a == "--back");
            let mut findings: Vec<shoebox_lib::faces::LeftAsIs> = Vec::new();
            for a in args[1..].iter().filter(|a| *a != "--back") {
                match serde_json::from_str::<serde_json::Value>(a)? {
                    list @ serde_json::Value::Array(_) => findings.extend(serde_json::from_value::<Vec<_>>(list)?),
                    one => findings.push(serde_json::from_value(one)?),
                }
            }
            if findings.is_empty() {
                anyhow::bail!("people-leave '<finding JSON>'... [--back]");
            }
            let n = shoebox_lib::faces::leave_as_is(&cat, &findings, !back)?;
            let said = if back { "back" } else { "left" };
            println!("{}", serde_json::json!({ said: n }));
        }
        Some("people-ignore") => {
            let show = args[1..].iter().any(|a| a == "--show");
            let ids =
                args[1..].iter().filter(|a| *a != "--show").map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if ids.is_empty() {
                anyhow::bail!("people-ignore <person-id>... [--show]");
            }
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            faces.set_ignored(&cat, &ids, !show)?;
            println!("{}", serde_json::json!({ "people": ids, "ignored": !show }));
        }
        Some("photo-faces") => {
            let ids = args[1..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if ids.is_empty() {
                anyhow::bail!("photo-faces <photo-id>...");
            }
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            println!("{}", serde_json::to_string_pretty(&faces.faces_in(&cat, &ids)?)?);
        }
        Some("person-faces") => {
            let pid: i64 = args.get(1).ok_or_else(|| anyhow::anyhow!("person-faces <person-id>"))?.parse()?;
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            println!("{}", serde_json::to_string_pretty(&faces.person_faces(&cat, pid)?)?);
        }
        Some("look-alikes") => {
            let ids = args[1..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if ids.is_empty() {
                anyhow::bail!("look-alikes <face-id>...");
            }
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            println!("{}", serde_json::to_string_pretty(&faces.look_alikes(&cat, &ids, &[], 6)?)?);
        }
        Some("move-faces") => {
            use shoebox_lib::faces::FaceTarget;
            let usage = || anyhow::anyhow!("move-faces <face-id>... --to <person-id> | --new [\"name\"] | --elsewhere");
            let flag = args.iter().position(|a| a.starts_with("--")).ok_or_else(usage)?;
            let ids = args[1..flag].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            let to = match args[flag].as_str() {
                "--to" => FaceTarget::Person { id: args.get(flag + 1).ok_or_else(usage)?.parse()? },
                "--new" => FaceTarget::New { name: args.get(flag + 1).cloned() },
                "--elsewhere" => FaceTarget::Elsewhere,
                _ => return Err(usage()),
            };
            if ids.is_empty() {
                return Err(usage());
            }
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            println!("{}", serde_json::to_string_pretty(&faces.reassign(&cat, &ids, &to)?)?);
        }
        Some("restore-faces") => {
            // Undo of move-faces: shoebox-cli restore-faces '<its "undo" JSON>'
            let json = args.get(1).ok_or_else(|| anyhow::anyhow!("restore-faces '<undo JSON from move-faces>'"))?;
            let placements: Vec<shoebox_lib::faces::Placement> = serde_json::from_str(json)?;
            let faces = shoebox_lib::faces::FaceIndex::open(&cat)?;
            println!("{}", serde_json::json!({ "restored": faces.restore(&cat, &placements)? }));
        }
        Some("probe") => {
            // Prints a model's inputs and outputs: shoebox-cli probe <model.onnx>
            let path = args.get(1).ok_or_else(|| anyhow::anyhow!("probe <model.onnx>"))?;
            let (s, gpu) = shoebox_lib::ai::session(Path::new(path))?;
            println!("gpu={gpu}");
            for i in &s.inputs {
                println!("in  {} {:?}", i.name, i.input_type);
            }
            for o in &s.outputs {
                println!("out {} {:?}", o.name, o.output_type);
            }
        }
        Some("list") => {
            // Photos in the grid's order: shoebox-cli list [oldest|newest|name|added] [limit] [filters JSON] [scope JSON]
            let sort: Sort = match args.get(1) {
                Some(s) => serde_json::from_value(serde_json::Value::String(s.clone()))?,
                None => Sort::default(),
            };
            let limit: usize = args.get(2).map(|n| n.parse()).transpose()?.unwrap_or(50);
            let filters: Filters = args.get(3).map(|f| serde_json::from_str(f)).transpose()?.unwrap_or_default();
            let scope: Scope = args.get(4).map(|s| serde_json::from_str(s)).transpose()?.unwrap_or(Scope::All);
            let list = cat.list_photos(&scope, sort, &filters)?;
            let mut out = Vec::new();
            const STATES: [&str; 3] = ["missing", "offline", "online"];
            for (i, id) in list.ids.iter().take(limit).enumerate() {
                let d = cat.photo_detail(*id)?;
                let taken =
                    d.taken_ts.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|t| t.naive_utc().to_string());
                let state = STATES[list.state[i].clamp(0, 2) as usize];
                out.push(serde_json::json!({
                    "id": id,
                    "taken": taken,
                    "name": d.files.first().map(|f| f.name.clone()),
                    "state": state,
                    "marked": list.marked[i] == 1,
                    // On a person: found in it since the user last looked at them.
                    "new": list.fresh[i] == 1,
                }));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "total": list.ids.len(), "photos": out }))?
            );
        }
        Some("sharp") => {
            // Times the viewer's sharp picture (nothing is written): shoebox-cli sharp <photo-id> [long, 0 = full]
            let id: i64 = args.get(1).ok_or_else(|| anyhow::anyhow!("sharp <photo-id> [long]"))?.parse()?;
            let long: u32 = args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(2560);
            let (file, kind, turn) = cat.original(id)?;
            let t = std::time::Instant::now();
            let bytes = shoebox_lib::imaging::sharp(&file, kind, long, turn)?;
            println!(
                "{}",
                serde_json::json!({ "file": file, "long": long, "kb": bytes.len() / 1024, "ms": t.elapsed().as_millis() as u64 })
            );
        }
        Some("private") => {
            // echo <password> | shoebox-cli private [--new] <photo-id>... | --in '<scope JSON>' | --name '<name>'
            let usage = || {
                anyhow::anyhow!(
                    "echo <password> | shoebox-cli private [--new] <photo-id>... | --in '<scope JSON>' | --name '<name>'"
                )
            };
            let read_password = || -> anyhow::Result<String> {
                let mut password = String::new();
                std::io::stdin().read_line(&mut password)?;
                Ok(password.trim_end_matches(['\r', '\n']).to_string())
            };
            if args.get(1).map(|s| s.as_str()) == Some("--name") {
                let name = args.get(2).ok_or_else(usage)?;
                shoebox_lib::private::rename_with_password(&cat, &read_password()?, name)?;
                println!("{}", serde_json::json!({ "named": true }));
                return Ok(());
            }
            let new = args.iter().any(|a| a == "--new");
            let rest: Vec<&String> = args[1..].iter().filter(|a| *a != "--new").collect();
            let ids: Vec<i64> = match rest.first().map(|s| s.as_str()) {
                None => return Err(usage()),
                Some("--in") => {
                    let scope: Scope = serde_json::from_str(rest.get(1).ok_or_else(usage)?)?;
                    cat.list_photos(&scope, Sort::default(), &Filters::default())?.ids
                }
                Some(_) => rest.iter().map(|s| s.parse()).collect::<Result<_, _>>()?,
            };
            let made = shoebox_lib::private::hide_with_password(&cat, &read_password()?, &ids, new)?;
            println!("{}", serde_json::json!({ "madePrivate": made }));
        }
        Some("tags") => {
            // Every tag, or the tags on some photos: shoebox-cli tags [photo-id...]
            let ids = args[1..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            let list = if ids.is_empty() { tags::all(&cat)? } else { tags::on(&cat, &ids)? };
            println!("{}", serde_json::to_string_pretty(&list)?);
        }
        Some(cmd @ ("tag" | "untag")) => {
            // shoebox-cli tag "<name>" <photo-id>...   |   shoebox-cli untag "<name>" <photo-id>...
            let name = args.get(1).ok_or_else(|| anyhow::anyhow!("{cmd} <name> <photo-id>..."))?;
            let ids = args[2..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if ids.is_empty() {
                anyhow::bail!("{cmd} <name> <photo-id>...");
            }
            if cmd == "tag" {
                println!("{}", serde_json::to_string_pretty(&tags::add(&cat, &ids, std::slice::from_ref(name))?)?);
            } else {
                let found: Vec<i64> =
                    tags::all(&cat)?.into_iter().filter(|t| t.name.eq_ignore_ascii_case(name)).map(|t| t.id).collect();
                println!("{}", serde_json::json!({ "removed": tags::remove(&cat, &ids, &found)? }));
            }
        }
        Some("tag-rename") => {
            // shoebox-cli tag-rename "<old>" "<new>"   (onto an existing name = merge)
            let usage = || anyhow::anyhow!("tag-rename <old> <new>");
            let (old, new) = (args.get(1).ok_or_else(usage)?, args.get(2).ok_or_else(usage)?);
            let id = tags::all(&cat)?
                .into_iter()
                .find(|t| t.name.eq_ignore_ascii_case(old))
                .ok_or_else(|| anyhow::anyhow!("No tag {old}"))?
                .id;
            tags::rename(&cat, id, new)?;
            println!("{}", serde_json::json!({ "renamed": new }));
        }
        Some("gear") => println!("{}", serde_json::to_string_pretty(&gear::list(&cat.db(), &cat.shown_sql("p"))?)?),
        Some(cmd @ ("gear-rename" | "gear-split")) => {
            let usage = || anyhow::anyhow!("gear-rename camera|lens <name> <new>  |  gear-split camera|lens <name>");
            let kind = gear::Kind::parse(args.get(1).ok_or_else(usage)?)?;
            let name = args.get(2).ok_or_else(usage)?;
            if cmd == "gear-rename" {
                let shown = gear::rename(&cat.db(), kind, name, args.get(3).ok_or_else(usage)?)?;
                println!("{}", serde_json::json!({ "renamed": name, "to": shown }));
            } else {
                println!("{}", serde_json::json!({ "split": name, "into": gear::split(&cat.db(), kind, name)? }));
            }
        }
        Some("find-lenses") => println!("{}", serde_json::to_string_pretty(&gear::find_lenses(&cat, &cancel)?)?),
        Some("lens") => {
            // The lens Shoebox reads in a file (metadata only): shoebox-cli lens <file>
            let file = args.get(1).ok_or_else(|| anyhow::anyhow!("lens <file>"))?;
            let kind = shoebox_lib::formats::kind_of(file).ok_or_else(|| anyhow::anyhow!("{file}: not a photo"))?;
            let t = std::time::Instant::now();
            let lens = shoebox_lib::imaging::read_lens(Path::new(file), kind);
            println!("{}", serde_json::json!({ "file": file, "lens": lens, "ms": t.elapsed().as_millis() as u64 }));
        }
        Some("find-focus") => println!("{}", serde_json::to_string_pretty(&focus::find(&cat, &cancel)?)?),
        Some("focus") => {
            let file = args.get(1).ok_or_else(|| anyhow::anyhow!("focus <file>"))?;
            let kind = shoebox_lib::formats::kind_of(file).ok_or_else(|| anyhow::anyhow!("{file}: not a photo"))?;
            let t = std::time::Instant::now();
            let focus = shoebox_lib::imaging::read_focus(Path::new(file), kind);
            println!("{}", serde_json::json!({ "file": file, "focus": focus, "ms": t.elapsed().as_millis() as u64 }));
        }
        Some("facets") => {
            // What each filter offers, with counts: shoebox-cli facets [filters JSON]
            let filters: Filters = args.get(1).map(|f| serde_json::from_str(f)).transpose()?.unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&cat.facets(&Scope::All, &filters)?)?);
        }
        Some("days") => {
            // Photos per day, as the calendar shows them: shoebox-cli days [filters JSON]
            let filters: Filters = args.get(1).map(|f| serde_json::from_str(f)).transpose()?.unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&cat.days(&Scope::All, &filters)?)?);
        }
        Some("models") => {
            use shoebox_lib::models;
            let id = || args.get(2).map(String::as_str).ok_or_else(|| anyhow::anyhow!("models download|delete <id>"));
            match args.get(1).map(String::as_str) {
                None | Some("list") => println!("{}", serde_json::to_string_pretty(&models::overview())?),
                Some("download") => {
                    let bytes = models::download(id()?, &cancel, &|done, total| {
                        eprint!("\r  {} of {} MB   ", done >> 20, total >> 20)
                    })?;
                    eprintln!();
                    println!("{}", serde_json::json!({ "downloaded": id()?, "bytes": bytes }));
                }
                Some("delete") => {
                    println!("{}", serde_json::json!({ "deleted": id()?, "bytesFreed": models::delete(id()?)? }))
                }
                Some("verify") => {
                    let ids: Vec<String> = match args.get(2) {
                        Some(id) => vec![id.clone()],
                        None => models::overview()
                            .models
                            .into_iter()
                            .filter(|m| !m.later && m.state != "missing")
                            .map(|m| m.id)
                            .collect(),
                    };
                    let mut out = serde_json::Map::new();
                    for id in ids {
                        out.insert(id.clone(), serde_json::to_value(models::verify(&id)?)?);
                    }
                    println!("{}", serde_json::to_string_pretty(&out)?);
                }
                Some(other) => anyhow::bail!("models: unknown {other:?} (list, download, delete, verify)"),
            }
        }
        Some("photo") => {
            let id: i64 = args.get(1).ok_or_else(|| anyhow::anyhow!("photo <id>"))?.parse()?;
            let quality = cull::QualityIndex::open(&cat)?;
            println!("{}", serde_json::to_string_pretty(&quality.photo_detail(&cat, id)?)?);
        }
        Some(cmd @ ("mark" | "unmark")) => {
            let n = if cmd == "unmark" && args.get(1).is_some_and(|a| a == "--all") {
                organize::unmark_all(&cat)?
            } else {
                let ids = args[1..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
                if ids.is_empty() {
                    anyhow::bail!("{cmd} <photo-id>...");
                }
                organize::mark(&cat, &ids, cmd == "mark")?
            };
            println!("{}", serde_json::json!({ cmd: n }));
        }
        Some("rename-folder") => {
            // shoebox-cli rename-folder <volume-id> <folder path on the disk> <new name>
            let usage = || anyhow::anyhow!("rename-folder <volume-id> <folder> <new name>");
            let vid: i64 = args.get(1).ok_or_else(usage)?.parse()?;
            let (rel, name) = (args.get(2).ok_or_else(usage)?, args.get(3).ok_or_else(usage)?);
            println!("{}", serde_json::json!({ "renamed": organize::rename_folder(&cat, vid, rel, name)? }));
        }
        Some("move-folder") => {
            // shoebox-cli move-folder <volume-id> <folder> <to-volume-id> <into folder>
            let usage = || anyhow::anyhow!("move-folder <volume-id> <folder> <to-volume-id> <into folder>");
            let m = organize::FolderMove {
                from: args.get(1).ok_or_else(usage)?.parse()?,
                rel: args.get(2).ok_or_else(usage)?.clone(),
                to: args.get(3).ok_or_else(usage)?.parse()?,
                to_parent: args.get(4).ok_or_else(usage)?.clone(),
            };
            let r = organize::move_folder(&cat, &m, &cancel, &|p| {
                eprint!("\r  {} {}/{} files, {} MB   ", p.phase, p.files, p.total_files, p.bytes >> 20)
            })?;
            eprintln!();
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        Some("new-folder") => {
            // shoebox-cli new-folder <volume-id> <parent folder path on the disk> [name]
            let usage = || anyhow::anyhow!("new-folder <volume-id> <parent folder> [name]");
            let vid: i64 = args.get(1).ok_or_else(usage)?.parse()?;
            let parent = args.get(2).ok_or_else(usage)?;
            let name = args.get(3).map(String::as_str).unwrap_or("New folder");
            println!("{}", serde_json::json!({ "created": organize::create_folder(&cat, vid, parent, name)? }));
        }
        Some("marked") => println!("{}", serde_json::to_string_pretty(&organize::plan(&cat)?)?),
        Some("quality") => {
            let quality = cull::QualityIndex::open(&cat)?;
            let t = std::time::Instant::now();
            quality.index_pending(&cat, &cancel, &|p| eprint!("\r  measured {}/{}", p.done, p.total))?;
            eprintln!();
            println!("{}", serde_json::json!({ "measured": quality.measured(), "seconds": t.elapsed().as_secs_f64() }));
        }
        Some("measure") => {
            let ids = args[1..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if ids.is_empty() {
                anyhow::bail!("measure <photo-id>...");
            }
            let quality = cull::QualityIndex::open(&cat)?;
            quality.measure(&cat, &ids, &cancel, &|_, _| {})?;
            let reports = ids.iter().map(|id| quality.report(&cat, *id)).collect::<anyhow::Result<Vec<_>>>()?;
            println!("{}", serde_json::to_string_pretty(&reports)?);
        }
        Some(cmd @ ("cull" | "cull-accept")) => {
            // shoebox-cli cull [filters JSON] [scope JSON] [--closer]
            let closer = args[1..].iter().any(|a| a == "--closer");
            let rest: Vec<&String> = args[1..].iter().filter(|a| *a != "--closer").collect();
            let filters: Filters = rest.first().map(|f| serde_json::from_str(f)).transpose()?.unwrap_or_default();
            let scope: Scope = rest.get(1).map(|s| serde_json::from_str(s)).transpose()?.unwrap_or(Scope::All);
            let quality = cull::QualityIndex::open(&cat)?;
            let search = shoebox_lib::search::SearchIndex::open(&cat)?;
            let mut shoot = quality.shoot(&cat, &search, &scope, &filters)?;
            if !shoot.to_measure.is_empty() {
                quality.measure(&cat, &shoot.to_measure, &cancel, &|d, t| eprint!("\r  measured {d}/{t}"))?;
                eprintln!();
                shoot = quality.shoot(&cat, &search, &scope, &filters)?;
            }
            if closer && !shoot.can_look_closer.is_empty() {
                quality
                    .look_closer(&cat, &shoot.can_look_closer, &cancel, &|d, t| eprint!("\r  looked closer {d}/{t}"))?;
                eprintln!();
                shoot = quality.shoot(&cat, &search, &scope, &filters)?;
            }
            if cmd == "cull" {
                println!("{}", serde_json::to_string_pretty(&shoot)?);
            } else {
                let rejects = shoot.suggested();
                println!("{}", serde_json::json!({ "marked": organize::mark(&cat, &rejects, true)? }));
            }
        }
        Some("delete-marked") => {
            let token = args.get(1).ok_or_else(|| anyhow::anyhow!("delete-marked <token> [volume-id...]"))?;
            let mut disks = args[2..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if disks.is_empty() {
                disks = organize::plan(&cat)?.disks.iter().map(|d| d.volume_id).collect();
            }
            println!("{}", serde_json::to_string_pretty(&organize::delete_confirmed(&cat, token, &disks)?)?);
        }
        Some("missing") => {
            let ids = cat.list_photos(&Scope::Missing, Sort::default(), &Filters::default())?.ids;
            println!("{}", serde_json::to_string_pretty(&cat.missing(&ids)?)?);
        }
        Some("forget-missing") => {
            let missing = || cat.list_photos(&Scope::Missing, Sort::default(), &Filters::default()).map(|l| l.ids);
            let ids: Vec<i64> = match args.get(1).map(String::as_str) {
                None => anyhow::bail!("forget-missing <photo-id>... | --in-bin | --all"),
                Some("--all") => missing()?,
                Some("--in-bin") => {
                    cat.missing(&missing()?)?.photos.into_iter().filter(|p| p.in_bin).map(|p| p.id).collect()
                }
                Some(_) => args[1..].iter().map(|a| a.parse()).collect::<Result<_, _>>()?,
            };
            println!("{}", serde_json::to_string_pretty(&cat.forget_missing(&ids)?)?);
        }
        Some("turn") => {
            let usage = || anyhow::anyhow!("turn right|left|half|back <photo-id>...");
            let how = args.get(1).ok_or_else(usage)?.as_str();
            let ids = args[2..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if ids.is_empty() {
                return Err(usage());
            }
            let by = match how {
                "right" => 1,
                "left" => -1,
                "half" => 2,
                "back" => 0,
                _ => return Err(usage()),
            };
            let report = if by == 0 {
                shoebox_lib::turn::back(&cat, &ids)?
            } else {
                shoebox_lib::turn::turn(&cat, &ids.iter().map(|id| (*id, by)).collect::<Vec<_>>())?
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Some("sideways") => println!("{}", serde_json::to_string_pretty(&upright::suggestions(&cat)?)?),
        Some("upright-check") => {
            if args.get(1).is_some_and(|a| a == "--again") {
                eprintln!("{} photos to look at again", upright::look_again(&cat)?);
            }
            let t = std::time::Instant::now();
            upright::Upright::default()
                .check_pending(&cat, &cancel, &|p| eprint!("\r  looked at {}/{} gpu={}", p.done, p.total, p.gpu))?;
            eprintln!();
            let found = upright::suggestions(&cat)?.len();
            println!("{}", serde_json::json!({ "sideways": found, "seconds": t.elapsed().as_secs_f64() }));
        }
        Some(cmd @ ("sideways-accept" | "sideways-leave")) => {
            let ids: Vec<i64> = match args.get(1).map(String::as_str) {
                None => anyhow::bail!("{cmd} <photo-id>... | --all"),
                Some("--all") => upright::suggestions(&cat)?.into_iter().map(|s| s.id).collect(),
                Some(_) => args[1..].iter().map(|a| a.parse()).collect::<Result<_, _>>()?,
            };
            if cmd == "sideways-accept" {
                println!("{}", serde_json::to_string_pretty(&upright::accept(&cat, &ids)?)?);
            } else {
                println!("{}", serde_json::json!({ "left": upright::leave(&cat, &ids)? }));
            }
        }
        Some("upright-eval") => {
            let usage = || anyhow::anyhow!("upright-eval <folder> [limit] [--as-they-stand]");
            let as_they_stand = args.iter().any(|a| a == "--as-they-stand");
            let rest: Vec<&String> = args[1..].iter().filter(|a| *a != "--as-they-stand").collect();
            let folder = Path::new(rest.first().ok_or_else(usage)?);
            let limit = rest.get(1).map(|n| n.parse()).transpose()?.unwrap_or(usize::MAX);
            if as_they_stand {
                println!("{}", serde_json::to_string_pretty(&upright::eval_as_they_stand(folder, limit)?)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&upright::eval(folder, limit)?)?);
            }
        }
        _ => eprintln!(
            "usage: shoebox-cli add <folder> | scan [root-id] | stats | version | photo <id> | gear | gear-rename camera|lens <name> <new> | gear-split camera|lens <name> | find-lenses | lens <file> | models [download|delete|verify] <id> | mark <id>... | unmark <id>... | unmark --all | marked | delete-marked <token> [volume-id...] | quality | measure <id>... | cull [filters] [scope] [--closer] | cull-accept [filters] [scope] | missing | forget-missing <id>... | --in-bin | --all | turn right|left|half|back <id>... | sideways | upright-check [--again] | sideways-accept <id>... | --all | sideways-leave <id>... | --all | upright-eval <folder> [limit]"
        ),
    }
    Ok(())
}

/// `shoebox-cli bench`: what the window does most, timed on this catalog and computer.
fn bench(cat: &Catalog, args: &[String]) -> anyhow::Result<()> {
    use std::time::Instant;
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let mut memory = cat.memory();
    if let Some(mb) = flag("--cache-mb") {
        memory.cache_mb = mb.parse()?;
    }
    if let Some(mb) = flag("--map-mb") {
        memory.map_mb = if mb == "off" { 0 } else { mb.parse()? };
    }
    cat.set_memory(memory)?;
    let ms = |t: Instant| (t.elapsed().as_secs_f64() * 10_000.0).round() / 10.0;
    // Start-up: what is read before word search and faces work.
    let t = Instant::now();
    let search = shoebox_lib::search::SearchIndex::open(cat)?;
    let search_ms = ms(t);
    let t = Instant::now();
    let faces = shoebox_lib::faces::FaceIndex::open(cat)?;
    let faces_ms = ms(t);
    // Worked out each time (the answers kept are forgotten first), then asked
    // again with nothing changed: what the window gets back from memory.
    let timed = |f: &dyn Fn() -> anyhow::Result<()>, fresh: bool| -> anyhow::Result<f64> {
        let mut b = f64::MAX;
        for _ in 0..3 {
            if fresh {
                cat.forget_kept();
                faces.forget_kept();
            }
            let t = Instant::now();
            f()?;
            b = b.min(ms(t));
        }
        Ok(b)
    };
    let best =
        |f: &dyn Fn() -> anyhow::Result<()>| -> anyhow::Result<(f64, f64)> { Ok((timed(f, true)?, timed(f, false)?)) };

    let none = Filters::default();
    let browse = cat.browse()?;
    let people = faces.people(cat)?;
    let view = |scope: Scope| {
        best(&|| {
            cat.list_photos(&scope, Sort::default(), &none)?;
            cat.facets(&scope, &none)?;
            Ok(())
        })
    };
    let mut views = vec![
        ("overview".to_string(), best(&|| cat.overview().map(|_| ()))?),
        ("sidebar".to_string(), best(&|| cat.browse().map(|_| ()))?),
        ("people".to_string(), best(&|| faces.people(cat).map(|_| ()))?),
        ("all photos".to_string(), view(Scope::All)?),
        ("all photos by file name".to_string(), best(&|| cat.list_photos(&Scope::All, Sort::Name, &none).map(|_| ()))?),
    ];
    if let Some(c) = browse.gear.cameras.iter().filter(|c| !c.unknown).max_by_key(|c| c.count) {
        views.push((format!("a camera ({} photos)", c.count), view(Scope::Camera { name: c.name.clone() })?));
    }
    if let Some(y) = browse.years.iter().max_by_key(|y| y.count) {
        views.push((format!("a year ({} photos)", y.count), view(Scope::Date { key: y.year.clone() })?));
    }
    if let Some(p) = people.iter().filter(|p| !p.ignored).max_by_key(|p| p.photos) {
        views.push((format!("a person ({} photos)", p.photos), view(Scope::Person { id: p.id, new_only: false })?));
    }
    let (photos, catalog_bytes): (i64, u64) = (
        cat.db().query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))?,
        cat.data.db_path().metadata().map(|m| m.len()).unwrap_or(0),
    );
    let views: Vec<_> = views
        .into_iter()
        .map(|(v, (ms, kept))| serde_json::json!({ "view": v, "ms": ms, "askedAgainMs": kept }))
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "photos": photos,
            "catalogMb": catalog_bytes >> 20,
            "memory": cat.memory(),
            "startup": { "wordSearchMs": search_ms, "descriptions": search.indexed(), "facesMs": faces_ms },
            "views": views,
            "note": "ms, best of 3; a view is its photos and its filter counts; askedAgainMs: the same with nothing changed",
        }))?
    );
    Ok(())
}
