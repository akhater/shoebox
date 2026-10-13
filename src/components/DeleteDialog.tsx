import { HardDrive, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { cull, fmtBytes, fmtNum, type DeletePlan, type DeleteReport } from "../api";

interface Props {
  onDone: (report: DeleteReport) => void;
  onClose: () => void;
}

const ROLE = { photo: "", twin: "JPEG twin", edits: "Argentum edits" } as const;
const SHOWN_FILES = 500;

/** "Delete marked files…": what would go, disk by disk, then a confirmed delete. */
export function DeleteDialog({ onDone, onClose }: Props) {
  const [plan, setPlan] = useState<DeletePlan | null>(null);
  const [ticked, setTicked] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    setPlan(null);
    setError(null);
    cull
      .plan()
      .then((p) => {
        setPlan(p);
        setTicked(p.disks.map((d) => d.volumeId));
      })
      .catch((e) => setError(String(e)));
  };
  useEffect(load, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const chosen = useMemo(() => plan?.disks.filter((d) => ticked.includes(d.volumeId)) ?? [], [plan, ticked]);
  const chosenFiles = useMemo(() => plan?.files.filter((f) => ticked.includes(f.volumeId)) ?? [], [plan, ticked]);
  const bytes = chosen.reduce((n, d) => n + d.bytes, 0);
  const twins = chosenFiles.filter((f) => f.role === "twin").length;
  const edits = chosenFiles.filter((f) => f.role === "edits").length;
  const toggle = (id: number) => setTicked((t) => (t.includes(id) ? t.filter((x) => x !== id) : [...t, id]));

  const confirm = async () => {
    if (!plan) return;
    setBusy(true);
    setError(null);
    try {
      onDone(await cull.delete(plan.token, ticked));
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const nothing = plan && plan.files.length === 0;

  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <div className="modal delete-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Delete marked files</h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>

        {!plan && !error && <p className="muted">Checking the disks…</p>}

        {nothing && <p>Nothing to delete right now: none of the marked photos is on a plugged-in disk.</p>}

        {plan && !nothing && (
          <>
            <p className="muted">
              {fmtNum(plan.photos)} marked {plan.photos === 1 ? "photo is" : "photos are"} here now. Untick a disk to keep
              the copies on it.
            </p>
            <ul className="del-disks">
              {plan.disks.map((d) => (
                <li key={d.volumeId} className={ticked.includes(d.volumeId) ? "" : "off"}>
                  <label>
                    <input type="checkbox" checked={ticked.includes(d.volumeId)} onChange={() => toggle(d.volumeId)} />
                    <HardDrive size={16} />
                    <span className="grow">
                      <b>{d.label}</b>
                      <span className="muted small">
                        {fmtNum(d.photos)} photos · {fmtNum(d.files)} files · {fmtBytes(d.bytes)}
                      </span>
                    </span>
                    {d.recycle ? (
                      <span className="pill online">To the Recycle Bin</span>
                    ) : (
                      <span className="pill missing">Deleted for good</span>
                    )}
                  </label>
                  {!d.recycle && (
                    <p className="note warn">
                      This disk is removable media (a memory card or USB stick) and has no Recycle Bin: its files can't be
                      brought back.
                    </p>
                  )}
                </li>
              ))}
            </ul>
            {(twins > 0 || edits > 0) && (
              <p className="note">
                Includes{" "}
                {[twins && `${fmtNum(twins)} JPEG twin${twins === 1 ? "" : "s"} of RAW files`, edits && `${fmtNum(edits)} Argentum edit file${edits === 1 ? "" : "s"}`]
                  .filter(Boolean)
                  .join(" and ")}
                .
              </p>
            )}
            {ticked.length < plan.disks.length && (
              <p className="note">Photos keeping a copy on an unticked disk stay marked.</p>
            )}
            {chosenFiles.length > 0 && (
              <details className="del-files">
                <summary>Show the {fmtNum(chosenFiles.length)} files</summary>
                <ul>
                  {chosenFiles.slice(0, SHOWN_FILES).map((f) => (
                    <li key={f.path} title={f.path}>
                      <span className="path">{f.path}</span>
                      <span className="muted">
                        {ROLE[f.role] && `${ROLE[f.role]} · `}
                        {fmtBytes(f.size)}
                      </span>
                    </li>
                  ))}
                  {chosenFiles.length > SHOWN_FILES && (
                    <li className="muted">and {fmtNum(chosenFiles.length - SHOWN_FILES)} more</li>
                  )}
                </ul>
              </details>
            )}
          </>
        )}

        {plan && plan.waiting > 0 && (
          <p className="note">
            {fmtNum(plan.waiting)} marked {plan.waiting === 1 ? "photo is" : "photos are"} on an unplugged disk or in a
            folder Shoebox can't find. {plan.waiting === 1 ? "It stays" : "They stay"} marked until the disk is plugged in.
          </p>
        )}

        {error && (
          <p className="note warn">
            {error}{" "}
            <button className="btn small" onClick={load}>
              Check again
            </button>
          </p>
        )}

        <div className="modal-foot">
          <button className="btn" onClick={onClose} disabled={busy}>
            {nothing ? "Close" : "Cancel"}
          </button>
          {plan && !nothing && (
            <button className="btn danger" disabled={busy || chosenFiles.length === 0} onClick={confirm}>
              <Trash2 size={15} />
              {busy ? "Deleting…" : `Delete ${fmtNum(chosenFiles.length)} files (${fmtBytes(bytes)})`}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
