import { RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { datesApi, fmtDateKey, fmtDay, fmtNum, thumbUrl, type DateCheck, type DateFinding, type Shifted } from "../api";
import { FixDatesDialog } from "./FixDatesDialog";

interface Props {
  /** A folder (and those in it), or the whole catalog. */
  where: { volumeId: number; relPath: string; name: string } | null;
  /** Dates moved or put back: the views follow. */
  onChanged: () => void;
  /** "Show photos": their folder, those photos selected. */
  onShow: (f: DateFinding) => void;
  onClose: () => void;
}

/** What was done to a card, and its Undo. */
interface Done {
  text: string;
  undo: () => Promise<unknown>;
  /** Dates moved (not only the check). */
  changed: boolean;
}

const photos = (n: number) => (n === 1 ? "1 photo" : `${fmtNum(n)} photos`);
const days = (n: number) => (Math.abs(n) === 1 ? "1 day" : `${fmtNum(Math.abs(n))} days`);
const DAY = 86_400;
const dayOf = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 10);

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
/** When photos were taken: "Mon, Feb 13, 2006, 05:29 PM to 06:31 PM", or two days in full. */
export function takenSpan(first: number, last: number) {
  if (first === last) return fmtDay(first);
  if (dayOf(first) === dayOf(last)) return `${fmtDay(first)} to ${timeFmt.format(new Date(last * 1000))}`;
  return `${fmtDay(first)} to ${fmtDay(last)}`;
}

/** "4 Nov 2006", or "26 Feb 2005 to 4 Mar 2005". */
function span(from: string, to: string) {
  return from === to ? fmtDateKey(from) : `${fmtDateKey(from)} to ${fmtDateKey(to)}`;
}

/** Where the photos are, from the folder whose name dates them: "2006-11-04 Birthday › Guest". */
function placeOf(f: DateFinding) {
  const parent = f.dated.includes("\\") ? f.dated.slice(0, f.dated.lastIndexOf("\\") + 1) : "";
  return f.relPath.slice(parent.length).split("\\").join(" › ");
}

/** An amount of time in words: "264 days", "2 h 30 min". */
function amountText(seconds: number) {
  const s = Math.abs(seconds);
  const [d, h, m] = [Math.floor(s / DAY), Math.floor((s % DAY) / 3600), Math.round((s % 3600) / 60)];
  return [d ? days(d) : "", h ? `${h} h` : "", m ? `${m} min` : ""].filter(Boolean).join(" ") || `${s} s`;
}

/** Moved photos, in words: "Moved 140 photos 264 days later: they start on Sat, 4 Nov 2006." */
export function movedText(done: Shifted) {
  const n = done.photos.length;
  if (n === 0) return "No date moved: none of them has a date to move there.";
  const amounts = new Set(done.photos.map((p) => p.by));
  const by = done.photos[0].by;
  const how = amounts.size > 1 ? "" : ` ${amountText(by)} ${by > 0 ? "later" : "earlier"}`;
  const first = Math.min(...done.photos.map((p) => p.to));
  const rest = done.unchanged ? ` ${photos(done.unchanged)} stayed as they were.` : "";
  return `Moved ${photos(n)}${how}: ${n === 1 ? "now" : "they start"} on ${fmtDateKey(dayOf(first))}.${rest}`;
}

/** "Check dates": folders named with a date (or in one) whose photos from one
 *  camera are days away from it and from the other photos there - a camera
 *  clock set wrong, most likely. Each card moves those photos to the folder's
 *  day keeping the time of day (or another date, by hand), with Undo on the
 *  card, or is left as is. Only Shoebox's catalog changes, never a file. */
export function DatesCheckDialog({ where, onChanged, onShow, onClose }: Props) {
  const [check, setCheck] = useState<DateCheck | null>(null);
  const [run, setRun] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<Map<string, Done>>(() => new Map());
  const [busy, setBusy] = useState<string | null>(null);
  const [showLeft, setShowLeft] = useState(false);
  // "Another date…": Fix dates for one card's photos.
  const [fixing, setFixing] = useState<{ key: string; f: DateFinding } | null>(null);

  useEffect(() => {
    setCheck(null);
    setError(null);
    // All of them, those left as is flagged: shown on demand, with no second check.
    datesApi
      .check(where, true)
      .then((c) => {
        setCheck(c);
        setDone(new Map());
      })
      .catch((e) => setError(String(e)));
  }, [run, where]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy && !fixing) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, fixing, onClose]);

  const keyOf = (f: DateFinding) => `${f.folderId}:${f.key}:${f.first}`;
  const act = async (key: string, work: () => Promise<Done | null>, changed = true) => {
    setBusy(key);
    setError(null);
    try {
      const d = await work();
      if (d) setDone((m) => new Map(m).set(key, d));
      if (changed) onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };
  const moved = (key: string, d: Shifted) =>
    setDone((m) => new Map(m).set(key, { text: movedText(d), undo: () => datesApi.undo(d), changed: true }));
  const undo = (key: string, d: Done) =>
    act(
      key,
      async () => {
        await d.undo();
        setDone((m) => {
          const next = new Map(m);
          next.delete(key);
          return next;
        });
        return null;
      },
      d.changed,
    );
  const leave = (f: DateFinding, left: boolean) =>
    act(
      keyOf(f),
      async () => {
        await datesApi.leave([f], left);
        return {
          text: left ? "Left as is: the check leaves it out from now on." : "Back in the check.",
          undo: () => datesApi.leave([f], !left),
          changed: false,
        };
      },
      false,
    );

  const card = (f: DateFinding) => {
    const key = keyOf(f);
    const did = done.get(key);
    const n = f.photos.length;
    const when = takenSpan(f.first, f.last);
    const side = f.daysOff < 0 ? "before" : "after";
    const said = f.fileDate
      ? `No date in ${n === 1 ? "this file" : "these files"}: ${n === 1 ? "it was" : "they were"} written ${when}`
      : `${n === 1 ? "Its" : "Their"} camera's clock says ${when}`;
    const camera = f.camera ?? (f.fileDate ? "No camera date" : "Unknown camera");
    return (
      <div key={key} className={`check-card ${did ? "done" : ""}`}>
        <div className="check-card-head">
          <strong>
            {camera}
            {f.serial ? <span className="muted"> (body {f.serial})</span> : null} · {photos(n)}
          </strong>
          <span className="small">{placeOf(f)}</span>
          <span className="muted small">
            {said}, {days(f.daysOff)} {side} the folder's {f.from === f.to ? "date" : "dates"}, {span(f.from, f.to)}.{" "}
            {f.others > 0 && f.othersFrom && f.othersTo
              ? `The other ${photos(f.others)} there are from ${span(f.othersFrom, f.othersTo)}.`
              : "No other photos there."}
            {f.corrected > 0 && ` ${f.corrected === n ? "All" : fmtNum(f.corrected)} corrected already.`}
          </span>
        </div>
        <div className="date-thumbs" title={when}>
          {f.photos.slice(0, 8).map((id) => (
            <img key={id} src={thumbUrl(id, 0)} alt="" loading="lazy" draggable={false} />
          ))}
          {n > 8 && <span className="more muted small">+{fmtNum(n - 8)}</span>}
        </div>
        <div className="check-card-foot">
          {did ? (
            <>
              <span className="muted">{did.text}</span>
              <button className="btn small" disabled={!!busy} onClick={() => undo(key, did)}>
                Undo
              </button>
            </>
          ) : (
            <>
              {f.left ? (
                <button className="btn small" disabled={!!busy} title="Back among the findings" onClick={() => leave(f, false)}>
                  Bring back
                </button>
              ) : (
                <button
                  className="btn small"
                  disabled={!!busy}
                  title="Right as they are (a trip, a folder named after the day its photos were copied): the check leaves them out"
                  onClick={() => leave(f, true)}
                >
                  Leave as is
                </button>
              )}
              <button
                className="btn small ghost"
                disabled={!!busy}
                title="Their folder, these photos selected"
                onClick={() => {
                  onShow(f);
                  onClose();
                }}
              >
                Show photos
              </button>
              <button className="btn small" disabled={!!busy} onClick={() => setFixing({ key, f })}>
                Another date…
              </button>
              <button
                className="btn small primary"
                disabled={!!busy}
                title={`${days(f.shift / DAY)} ${f.shift > 0 ? "later" : "earlier"}, the time of day kept. Only Shoebox's catalog changes: the files keep their own date.`}
                onClick={() =>
                  act(key, async () => {
                    const d = await datesApi.change(f.photos, { kind: "to", date: f.from });
                    return { text: movedText(d), undo: () => datesApi.undo(d), changed: true };
                  })
                }
              >
                {busy === key ? "Moving…" : `Move to ${fmtDateKey(f.from)}`}
              </button>
            </>
          )}
        </div>
      </div>
    );
  };

  const open = check?.findings.filter((f) => !f.left) ?? [];
  const left = check?.findings.filter((f) => f.left) ?? [];
  const shownLeft = showLeft && left.length > 0;
  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <div className="modal strays-modal check-modal dates-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>
            Check dates{where && <span className="muted"> in {where.name}</span>}
          </h2>
          <button className="btn small ghost" disabled={!check || !!busy} onClick={() => setRun((n) => n + 1)}>
            <RefreshCw size={14} /> Check again
          </button>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={!!busy}>
            <X size={18} />
          </button>
        </div>
        {!check && !error && <p className="muted">Comparing each folder's name with its photos' dates…</p>}
        {check && (
          <p className="muted check-summary">
            <span>
              {check.folders === 0
                ? "No folder here is named with a date (2006-11-04 Birthday): nothing to compare."
                : open.length
                  ? `In ${fmtNum(check.folders)} folders named with a date (${photos(check.photos)}), these photos from one camera are more than ${days(check.days)} away from their folder's date and from the other photos there: a camera clock set wrong, most likely. Moving them changes Shoebox's catalog only, never the files; each move has Undo.`
                  : `The ${photos(check.photos)} of ${fmtNum(check.folders)} folders named with a date agree with their folders' dates.`}
              {check.corrected > 0 && ` ${photos(check.corrected)} of the catalog have a corrected date.`}
            </span>
            {left.length > 0 && (
              <button className="btn small ghost" onClick={() => setShowLeft((s) => !s)}>
                {showLeft ? "Hide those left as is" : `Show ${fmtNum(left.length)} left as is`}
              </button>
            )}
          </p>
        )}
        {(open.length > 0 || shownLeft) && (
          <div className="check-list">
            {open.map(card)}
            {shownLeft && (
              <section className="check-section left">
                <h3>
                  Left as is <span className="muted">{fmtNum(left.length)}</span>
                </h3>
                {left.map(card)}
              </section>
            )}
          </div>
        )}
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          <button className="btn" onClick={onClose} disabled={!!busy}>
            Close
          </button>
        </div>
      </div>
      {fixing && (
        <FixDatesDialog
          ids={fixing.f.photos}
          suggested={fixing.f.from}
          onClose={() => setFixing(null)}
          onDone={(d) => {
            moved(fixing.key, d);
            setFixing(null);
            onChanged();
          }}
        />
      )}
    </div>
  );
}
