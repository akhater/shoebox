//! Headless Shoebox, for testing the catalog without the window.
//! Set SHOEBOX_DATA to use a throwaway catalog folder.
//!
//!   shoebox-cli add <folder>     add a folder and scan it
//!   shoebox-cli scan [root-id]   rescan one folder, or every plugged-in one
//!   shoebox-cli stats            catalog overview
//!   shoebox-cli photo <id>       one photo's details
//!   shoebox-cli backup           consistent copies of the databases in backup\ (run it
//!                                before each Kopia snapshot; see .kopiaignore)
//!   shoebox-cli list [oldest|newest|name|added] [limit] [filters]   photos in grid order (JSON)
//!   shoebox-cli facets [filters]  what each filter offers, with counts (JSON)
//!       filters = JSON, any of {"dates":["2026-09-26"],"cameras":[..],"lenses":[..],
//!                               "types":["cr3"],"status":["online","offline","missing"],
//!                               "people":[42],"tags":[3,7] (photos with all of these tags)}
//!
//! Tags (catalog only, never written into photos):
//!   shoebox-cli tags [photo-id...]              every tag, or those on some photos
//!   shoebox-cli tag "<name>" <photo-id>...      add (made if new)
//!   shoebox-cli untag "<name>" <photo-id>...
//!   shoebox-cli tag-rename "<old>" "<new>"      onto an existing name = merge
//!
//! Folders (on the disk and in the catalog in one step; JSON out):
//!   shoebox-cli rename-folder <volume-id> <folder> <new name>
//!   shoebox-cli new-folder <volume-id> <parent folder> [name]
//!   shoebox-cli move-folder <volume-id> <folder> <to-volume-id> <into folder>
//!       same disk: moved at once; another disk: copied, checked byte for byte, original removed
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
//!   shoebox-cli marked                         what deleting would remove now, by disk, with its token
//!   shoebox-cli delete-marked <token> [volume-id...]
//!                                              delete it (Recycle Bin; removable media for good),
//!                                              on the listed disks or every disk in the plan

use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{self, Progress};
use shoebox_lib::{organize, tags};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    shoebox_lib::volumes::quiet_errors();
    let data = DataDir::locate();
    data.ensure()?;
    shoebox_lib::setup_logging(&data);
    let cat = Catalog::open(data)?;
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
        Some("stats") => {
            println!("{}", serde_json::to_string_pretty(&cat.overview()?)?);
            let list = cat.list_photos(&Scope::All, Sort::default(), &Filters::default())?;
            let failed = list.preview.iter().filter(|&&s| s == 2).count();
            println!("photos listed: {}, without preview: {failed}", list.ids.len());
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
            let (hits, clear) = index.search(&q, 1000)?;
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
                println!(
                    "  person {:>4}: {:>3} faces in {:>3} photos  {:?}{}",
                    p.id,
                    p.faces,
                    p.photos,
                    p.name,
                    if p.ignored { " (ignored)" } else { "" }
                );
            }
        }
        Some("faces-reset") => {
            // Test catalogs only: forget every face, person and name.
            if std::env::var_os("SHOEBOX_DATA").is_none() {
                anyhow::bail!("faces-reset only runs on a test catalog (set SHOEBOX_DATA)");
            }
            cat.db().execute_batch("DELETE FROM faces; DELETE FROM people; DELETE FROM face_scan;")?;
            rusqlite::Connection::open(cat.data.root.join("faces.db"))?.execute_batch("DELETE FROM emb;")?;
            println!("faces reset");
        }
        Some("person") => {
            // Faces of one person: shoebox-cli person <id>
            let pid: i64 = args.get(1).ok_or_else(|| anyhow::anyhow!("person <id>"))?.parse()?;
            let db = cat.db();
            let mut stmt = db.prepare(
                "SELECT id, photo_id, round(x1,3), round(y1,3), round(x2,3), round(y2,3), round(score,3) FROM faces WHERE person_id = ?1 ORDER BY photo_id LIMIT 12",
            )?;
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
                println!("{}", serde_json::json!({ "moved": faces.move_out(&cat, pid, &ids)? }));
            } else {
                println!("{}", serde_json::to_string_pretty(&strays)?);
            }
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
            // Photos in the grid's order: shoebox-cli list [oldest|newest|name|added] [limit] [filters JSON]
            let sort: Sort = match args.get(1) {
                Some(s) => serde_json::from_value(serde_json::Value::String(s.clone()))?,
                None => Sort::default(),
            };
            let limit: usize = args.get(2).map(|n| n.parse()).transpose()?.unwrap_or(50);
            let filters: Filters = args.get(3).map(|f| serde_json::from_str(f)).transpose()?.unwrap_or_default();
            let list = cat.list_photos(&Scope::All, sort, &filters)?;
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
            let (file, kind) = cat.original(id)?;
            let t = std::time::Instant::now();
            let bytes = shoebox_lib::imaging::sharp(&file, kind, long)?;
            println!(
                "{}",
                serde_json::json!({ "file": file, "long": long, "kb": bytes.len() / 1024, "ms": t.elapsed().as_millis() as u64 })
            );
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
        Some("facets") => {
            // What each filter offers, with counts: shoebox-cli facets [filters JSON]
            let filters: Filters = args.get(1).map(|f| serde_json::from_str(f)).transpose()?.unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&cat.facets(&Scope::All, &filters)?)?);
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
            println!("{}", serde_json::to_string_pretty(&cat.photo_detail(id)?)?);
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
        Some("delete-marked") => {
            let token = args.get(1).ok_or_else(|| anyhow::anyhow!("delete-marked <token> [volume-id...]"))?;
            let mut disks = args[2..].iter().map(|a| a.parse()).collect::<Result<Vec<i64>, _>>()?;
            if disks.is_empty() {
                disks = organize::plan(&cat)?.disks.iter().map(|d| d.volume_id).collect();
            }
            println!("{}", serde_json::to_string_pretty(&organize::delete_confirmed(&cat, token, &disks)?)?);
        }
        _ => eprintln!(
            "usage: shoebox-cli add <folder> | scan [root-id] | stats | photo <id> | models [download|delete|verify] <id> | mark <id>... | unmark <id>... | unmark --all | marked | delete-marked <token> [volume-id...]"
        ),
    }
    Ok(())
}
