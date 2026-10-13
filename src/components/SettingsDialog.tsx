import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderInput, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api, fmtBytes, fmtNum, type FolderCheck, type MoveProgress, type Settings, type Stats } from "../api";
import { ModelsTab } from "./ModelsTab";

const SIZES = [720, 1080, 1440, 2048, 2560, 3840];
const QUALITIES: [number, string][] = [
  [70, "70 - smallest, ~35% less space"],
  [80, "80"],
  [84, "84 (default)"],
  [90, "90 - largest"],
];

export type SettingsTab = "general" | "models";
const TABS: [SettingsTab, string][] = [
  ["general", "General"],
  ["models", "AI models"],
];

interface Props {
  initialTab: SettingsTab;
  dataDir: string;
  version: string;
  onRegenerateAll: () => void;
  onClose: () => void;
}

export function SettingsDialog({ initialTab, dataDir, version, onRegenerateAll, onClose }: Props) {
  const [tab, setTab] = useState(initialTab);
  const [stats, setStats] = useState<Stats | null>(null);
  const [s, setS] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [check, setCheck] = useState<FolderCheck | null>(null);
  const [moving, setMoving] = useState<MoveProgress | null>(null);

  useEffect(() => {
    api.settings().then(setS).catch((e) => setError(String(e)));
    api.stats().then(setStats).catch(() => {});
    const un = listen<MoveProgress>("catalog-move", (e) => {
      setMoving(e.payload);
      if (e.payload.error) setError(e.payload.error);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const busy = moving !== null && !moving.error;

  const save = async () => {
    if (!s) return;
    try {
      await api.saveSettings(s);
      onClose();
    } catch (e) {
      setError(String(e));
    }
  };

  const pickFolder = async () => {
    setError(null);
    const dir = await open({ directory: true, multiple: false, title: "Where should Shoebox keep its catalog?" });
    if (typeof dir !== "string") return;
    try {
      setCheck(await api.checkCatalogFolder(dir));
    } catch (e) {
      setError(String(e));
    }
  };

  const confirmMove = async () => {
    if (!check) return;
    setError(null);
    try {
      if (check.hasCatalog) await api.switchCatalog(check.target);
      else {
        setMoving({ done: 0, total: 0, finished: false, error: null });
        await api.moveCatalog(check.target);
      }
    } catch (e) {
      setMoving(null);
      setError(String(e));
    }
  };

  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <div className="modal settings" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>
            Settings <span className="muted small">Shoebox {version && `v${version}`}</span>
          </h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>
        <div className="tabs" role="tablist">
          {TABS.map(([t, label]) => (
            <button
              key={t}
              role="tab"
              aria-selected={tab === t}
              className={tab === t ? "on" : ""}
              onClick={() => setTab(t)}
              disabled={busy}
            >
              {label}
            </button>
          ))}
        </div>
        {tab === "models" && <ModelsTab />}
        {tab === "general" && s && (
          <div className="form">
            <label>
              <span>Preview size</span>
              <select value={s.previewSize} onChange={(e) => setS({ ...s, previewSize: +e.target.value })}>
                {SIZES.map((n) => (
                  <option key={n} value={n}>
                    {n} px{n === 1080 ? " (default)" : ""}
                  </option>
                ))}
              </select>
              <small>
                The copy Shoebox keeps of each photo, shown when its disk is unplugged. Bigger looks sharper and takes
                more space.
              </small>
            </label>
            <label>
              <span>Preview quality</span>
              <select value={s.previewQuality} onChange={(e) => setS({ ...s, previewQuality: +e.target.value })}>
                {QUALITIES.map(([q, label]) => (
                  <option key={q} value={q}>
                    {label}
                  </option>
                ))}
              </select>
              <small>
                Size and quality apply to photos read from now on. To redo the ones already in the catalog, use
                Save &amp; rebuild previews below (their disks must be plugged in).
              </small>
            </label>
            <label>
              <span>Photos read at once</span>
              <select value={s.workers} onChange={(e) => setS({ ...s, workers: +e.target.value })}>
                {[1, 2, 3, 4, 6, 8].map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
              <small>For a slow USB hard disk, 2 is often as fast as more.</small>
            </label>
            <div className="field">
              <span className="field-label">Catalog</span>
              {stats ? (
                <div className="stats">
                  <div>
                    <b>{fmtNum(stats.photos)}</b> photos <span className="muted">({fmtNum(stats.raw)} RAW, {fmtNum(stats.other)} other)</span>
                  </div>
                  <div>
                    <b>{fmtNum(stats.files)}</b> files in <b>{fmtNum(stats.folders)}</b> folders on <b>{fmtNum(stats.disks)}</b>{" "}
                    disk{stats.disks === 1 ? "" : "s"}
                    {stats.missingFiles > 0 && <span className="warn-text"> · {fmtNum(stats.missingFiles)} missing</span>}
                    {stats.noPreview > 0 && <span className="muted"> · {fmtNum(stats.noPreview)} without preview</span>}
                  </div>
                  <div>
                    <b>{fmtBytes(stats.totalBytes)}</b> on disk{" "}
                    <span className="muted">
                      (previews {fmtBytes(stats.previewBytes)}, thumbnails {fmtBytes(stats.thumbBytes)}, database{" "}
                      {fmtBytes(stats.dbBytes)})
                    </span>
                  </div>
                </div>
              ) : (
                <small>Counting…</small>
              )}
              <div className="row">
                <button
                  className="btn small"
                  disabled={busy}
                  onClick={async () => {
                    if (s) await api.saveSettings(s).catch((e) => setError(String(e)));
                    onRegenerateAll();
                    onClose();
                  }}
                >
                  Save &amp; rebuild previews
                </button>
                <small>Reads every photo on the plugged-in disks again with the size and quality above.</small>
              </div>
            </div>
            <div className="field">
              <span className="field-label">Catalog folder</span>
              <div className="row">
                <code className="grow">{dataDir}</code>
                <button className="btn small" onClick={pickFolder} disabled={busy}>
                  <FolderInput size={14} /> Move…
                </button>
              </div>
              <small>Everything Shoebox knows lives in this one folder: catalog, previews, settings.</small>

              {check && !moving && (
                <div className={`confirm ${check.problem ? "bad" : ""}`}>
                  {check.problem ? (
                    <p>{check.problem}</p>
                  ) : check.hasCatalog ? (
                    <>
                      <p>
                        <b>{check.target}</b> already holds a Shoebox catalog. Switch to it? Shoebox restarts; the current
                        catalog stays where it is.
                      </p>
                      <div className="row end">
                        <button className="btn small ghost" onClick={() => setCheck(null)}>
                          Cancel
                        </button>
                        <button className="btn small primary" onClick={confirmMove}>
                          Switch
                        </button>
                      </div>
                    </>
                  ) : (
                    <>
                      <p>
                        Move the catalog ({fmtBytes(check.bytes)}) to <b>{check.target}</b> and use it from now on? On
                        the same disk it takes seconds (folders are moved, as in Explorer); to another disk it's copied.
                        Any scan stops and Shoebox restarts when it's done. Once Shoebox has started on the new folder, the
                        old one goes to the Recycle Bin.
                      </p>
                      <div className="row end">
                        <button className="btn small ghost" onClick={() => setCheck(null)}>
                          Cancel
                        </button>
                        <button className="btn small primary" onClick={confirmMove}>
                          Move catalog
                        </button>
                      </div>
                    </>
                  )}
                </div>
              )}
              <BackupLine />
              {moving && !moving.error && (
                <div className="confirm">
                  <p>
                    {moving.finished
                      ? "Done. Restarting…"
                      : moving.total
                        ? `Moving… ${fmtNum(moving.done)} of ${fmtNum(moving.total)} files`
                        : "Stopping scans…"}
                  </p>
                  <span className="bar wide">
                    <span style={{ width: `${moving.total ? (moving.done / moving.total) * 100 : 0}%` }} />
                  </span>
                </div>
              )}
            </div>
          </div>
        )}
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          {tab === "general" ? (
            <>
              <button className="btn ghost" onClick={onClose} disabled={busy}>
                Cancel
              </button>
              <button className="btn primary" onClick={save} disabled={!s || busy}>
                Save
              </button>
            </>
          ) : (
            <button className="btn" onClick={onClose}>
              Close
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** Backup: consistent copies of the databases for Kopia (or any backup tool),
 *  made now or by `shoebox-cli backup` before each snapshot. */
function BackupLine() {
  const [last, setLast] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  useEffect(() => {
    api.lastBackup().then(setLast).catch(() => {});
  }, []);
  const run = async () => {
    setBusy(true);
    try {
      const r = await api.backup();
      setLast(r.at);
      setNote(`${r.files.length} databases copied (${fmtBytes(r.bytes)}) in ${r.ms} ms.`);
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="backup-line">
      <div className="row">
        <span className="grow">
          <b>Backup</b>{" "}
          <span className="muted">
            {last ? `· copies made ${new Date(last * 1000).toLocaleString()}` : "· no copies made yet"}
          </span>
        </span>
        <button className="btn small" onClick={run} disabled={busy}>
          {busy ? "Copying…" : "Make backup copies now"}
        </button>
      </div>
      <small>
        For Kopia or any backup tool: back up this folder, and run <code>shoebox-cli backup</code> just before each
        snapshot (Kopia: <i>before-snapshot action</i>). It writes consistent copies of the databases in{" "}
        <code>backup\</code>, safe while Shoebox runs; the <code>.kopiaignore</code> it leaves skips the live ones.
        Previews never change once made, so a daily snapshot only adds what's new.
      </small>
      {note && <small>{note}</small>}
    </div>
  );
}
