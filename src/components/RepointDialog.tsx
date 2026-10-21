import { ArrowRight, Check, FolderSync, HardDrive, TriangleAlert, X } from "lucide-react";
import { useEffect } from "react";
import { fmtNum, type Repoint } from "../api";

interface Props {
  /** The source, as the sidebar names it. */
  name: string;
  /** Where it was: its path, its disk's name, and whether that disk is unplugged (else its folder is gone). */
  was: { path: string; disk: string; unplugged: boolean };
  /** Where its photos were found, and how many of those looked for are there. */
  now: Repoint;
  onUse: () => void;
  /** "Not now", Escape, or a click outside. */
  onClose: () => void;
}

/** "Pictures is on Acer (C:) now": a source found where its photos are now (a new computer, a replaced disk),
 *  the old place and the new one side by side, before anything changes. */
export function RepointDialog({ name, was, now, onUse, onClose }: Props) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const all = now.found === now.checked;
  return (
    <div className="modal-back" onClick={onClose}>
      <div
        className="modal repoint-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="repoint-title"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2 id="repoint-title">
            <FolderSync size={18} /> {name} is on {now.disk} now
          </h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close">
            <X size={18} />
          </button>
        </div>
        <p className="lead">
          {was.unplugged
            ? "It looks like its disk was replaced: a new computer, or a disk copied onto another."
            : "It looks like it moved to another disk."}
        </p>
        <div className="repoint-places">
          <div className="repoint-place was">
            <span className="repoint-when">Was</span>
            <span className="repoint-disk">
              <HardDrive size={15} /> <span className="ellipsis">{was.disk}</span>
            </span>
            <span className="repoint-path" title={was.path}>
              {was.path}
            </span>
            <span className="repoint-state">
              <span className="dot off" /> {was.unplugged ? "Not plugged in" : "Folder not there"}
            </span>
          </div>
          <ArrowRight size={18} className="repoint-arrow" />
          <div className="repoint-place now">
            <span className="repoint-when">Now</span>
            <span className="repoint-disk">
              <HardDrive size={15} /> <span className="ellipsis">{now.disk}</span>
            </span>
            <span className="repoint-path" title={now.path}>
              {now.path}
            </span>
            <span className="repoint-state">
              <span className="dot on" /> Plugged in
            </span>
          </div>
        </div>
        {now.checked > 0 && (
          <p className={`repoint-check ${all ? "" : "some"}`}>
            {all ? <Check size={16} /> : <TriangleAlert size={16} />}
            {all
              ? `All ${fmtNum(now.checked)} photos checked are there, unchanged.`
              : `${fmtNum(now.found)} of the ${fmtNum(now.checked)} photos checked are there, unchanged; the others aren't.`}
          </p>
        )}
        <p className="repoint-note">
          Tags, faces and people stay with the photos, and a quick scan picks up new ones.
          {all ? "" : " Photos not found there show as missing after it; they stay in the catalog."} Your files are not
          touched.
        </p>
        <div className="modal-foot">
          <button className="btn" onClick={onClose}>
            Not now
          </button>
          <button className="btn primary" autoFocus onClick={onUse}>
            Use it
          </button>
        </div>
      </div>
    </div>
  );
}
