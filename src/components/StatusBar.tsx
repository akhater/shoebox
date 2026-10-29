import { LoaderCircle, X } from "lucide-react";
import { fmtBytes, fmtDay, fmtNum, type AiProgress, type FolderMoveEvent, type Overview, type PhotoDetail, type Progress } from "../api";
import type { Message } from "../App";

interface Props {
  /** The photo selected in the grid (the current one, when several are). */
  photo: PhotoDetail | null;
  /** How many are selected. */
  selected: number;
  overview: Overview | null;
  progress: Progress | null;
  aiProgress: AiProgress | null;
  queued: number;
  message: Message | null;
  onCancel: () => void;
  /** A folder going to another disk. */
  move: FolderMoveEvent | null;
  onStopMove: () => void;
}

const MOVE_PHASE = { copying: "copying", checking: "checking every byte", removing: "removing the original" } as const;
/** Photos are copied and checked one by one, then their originals removed. */
const PHOTOS_PHASE = { copying: "copying and checking", checking: "checking every byte", removing: "removing the originals" } as const;

export function StatusBar({ photo, selected, overview, progress, aiProgress, queued, message, onCancel, move, onStopMove }: Props) {
  if (move?.progress) {
    const m = move.progress;
    const pct = m.phase === "removing" ? (m.files / Math.max(1, m.totalFiles)) * 100 : (m.bytes / Math.max(1, m.totalBytes)) * 100;
    return (
      <footer className="statusbar busy">
        <LoaderCircle size={14} className="spinning" />
        <span>
          {move.photos ? (
            <b>
              Moving {move.photos === 1 ? "1 photo" : `${fmtNum(move.photos)} photos`} to “{move.name}”
            </b>
          ) : (
            <b>Moving “{move.name}”</b>
          )}{" "}
          · {(move.photos ? PHOTOS_PHASE : MOVE_PHASE)[m.phase]} · {fmtNum(m.files)} of {fmtNum(m.totalFiles)} files
          {m.phase !== "removing" && <span className="muted"> · {fmtBytes(m.bytes)} of {fmtBytes(m.totalBytes)}</span>}
        </span>
        <span className="grow" />
        <span className="bar">
          <span style={{ width: `${pct}%` }} />
        </span>
        {m.phase !== "removing" && (
          <button
            className="btn small ghost"
            onClick={onStopMove}
            title={
              move.photos
                ? "Stop after this file: the photos moved so far stay moved, the others stay where they are"
                : "Stop: the copy is removed, the original stays"
            }
          >
            <X size={14} /> Stop
          </button>
        )}
      </footer>
    );
  }
  if (progress && progress.phase !== "done") {
    const pct = progress.phase === "reading" && progress.toRead > 0 ? (progress.read / progress.toRead) * 100 : null;
    return (
      <footer className="statusbar busy">
        <LoaderCircle size={14} className="spinning" />
        <span>
          <b>{progress.rootName}</b>
          {progress.phase === "listing"
            ? ` · looking for photos · ${fmtNum(progress.found)} found`
            : progress.rootId === 0
              ? ` · reading again ${fmtNum(progress.read)} of ${fmtNum(progress.toRead)} photos that couldn't be read before`
              : progress.toRead === 0
                ? ` · ${fmtNum(progress.found)} photos, nothing new`
                : ` · reading ${fmtNum(progress.read)} of ${fmtNum(progress.toRead)} new photos`}
        </span>
        {progress.current && <span className="muted ellipsis">{progress.current}</span>}
        {queued > 0 && <span className="muted">· {queued} more queued</span>}
        <span className="grow" />
        {pct !== null && (
          <span className="bar">
            <span style={{ width: `${pct}%` }} />
          </span>
        )}
        <button className="btn small ghost" onClick={onCancel}>
          <X size={14} /> Stop
        </button>
      </footer>
    );
  }
  if (aiProgress && aiProgress.total > 0) {
    const pct = (aiProgress.done / aiProgress.total) * 100;
    return (
      <footer className="statusbar busy">
        <LoaderCircle size={14} className="spinning" />
        <span>
          <b>
            {aiProgress.task === "faces"
              ? "Finding faces"
              : aiProgress.task === "upright"
                ? "Looking for sideways photos"
                : "Preparing word search"}
          </b>{" "}
          · {fmtNum(aiProgress.done)} of {fmtNum(aiProgress.total)} photos
          <span className="muted">
            {` · on the ${aiProgress.gpu ? "GPU" : "CPU"} · ${
              aiProgress.task === "faces"
                ? "People fills in as it goes"
                : aiProgress.task === "upright"
                  ? "nothing is turned until you say so"
                  : "search works on the ones done"
            }`}
          </span>
        </span>
        <span className="grow" />
        <span className="bar">
          <span style={{ width: `${pct}%` }} />
        </span>
      </footer>
    );
  }
  // Left: a message (with its Undo) or the catalog totals, as always.
  // Right: the selected photo. When space is short the left side and the
  // photo's details give way; the photo's name never does.
  return (
    <footer className="statusbar">
      {message ? (
        <>
          <span className={`status-left ${message.warn ? "warn-text" : ""}`}>{message.text}</span>
          {message.action && (
            <button className="btn small" onClick={message.action.run}>
              {message.action.label}
            </button>
          )}
        </>
      ) : overview ? (
        <span className="status-left muted">
          {fmtNum(overview.total)} photos · {fmtNum(overview.online)} on plugged-in disks
          {overview.offline > 0 && ` · ${fmtNum(overview.offline)} on unplugged disks`}
          {overview.missing > 0 && ` · ${fmtNum(overview.missing)} missing`}
        </span>
      ) : null}
      {photo && (
        <>
          <span className="grow" />
          <PhotoLine photo={photo} selected={selected} />
        </>
      )}
    </footer>
  );
}

/** The selected photo: name in full, then type, when, resolution, size; where it is if not plugged in. */
function PhotoLine({ photo, selected }: { photo: PhotoDetail; selected: number }) {
  const f = photo.files.find((x) => x.state === "online") ?? photo.files[0];
  const ext = f?.name.split(".").pop()?.toUpperCase() ?? "";
  const parts = [
    photo.kind === "raw" ? `${ext} RAW` : ext,
    photo.takenTs ? fmtDay(photo.takenTs) : "Unknown date",
    photo.width && photo.height && `${photo.width} × ${photo.height}`,
    f && fmtBytes(f.size),
  ].filter(Boolean);
  const where =
    f?.state === "offline"
      ? `on ${f.volumeLabel}, unplugged`
      : f?.state === "unreachable"
        ? "folder not found"
        : f?.state === "missing"
          ? "missing"
          : null;
  return (
    <span className="photo-line">
      {selected > 1 && <span className="photo-count muted">{fmtNum(selected)} selected ·</span>}
      <b className="photo-name" title={f?.name}>
        {f?.name ?? "Photo"}
      </b>
      {where && <span className="photo-where warn-text">{where}</span>}
      <span className="photo-more muted">· {parts.join(" · ")}</span>
    </span>
  );
}
