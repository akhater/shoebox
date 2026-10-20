import { X } from "lucide-react";
import { useEffect, useState } from "react";
import { datesApi, fmtDateKey, fmtDay, fmtNum, type DateChange, type PhotoDates, type Shifted } from "../api";
import { takenSpan } from "./DatesCheckDialog";

interface Props {
  ids: number[];
  /** The day to offer first (the folder's date a check found); else the
   *  date of the folder they are in, else their own first day. */
  suggested?: string;
  onDone: (done: Shifted, how: DateChange) => void;
  onClose: () => void;
}

const DAY = 86_400;
const dayOf = (ts: number) => new Date(ts * 1000).toISOString().slice(0, 10);
const dayNumber = (key: string) => Math.floor(Date.parse(`${key}T00:00:00Z`) / 1000 / DAY);
const photos = (n: number) => (n === 1 ? "1 photo" : `${fmtNum(n)} photos`);

/** "Fix dates": moves some photos' dates in Shoebox's catalog - to a day,
 *  keeping the time of day and the time between them, or by an amount (a
 *  camera an hour off) - or back to the dates their files give. The files
 *  are never changed; the first and last photo show before and after. */
export function FixDatesDialog({ ids, suggested, onDone, onClose }: Props) {
  const [info, setInfo] = useState<PhotoDates | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<"to" | "by">("to");
  const [date, setDate] = useState(suggested ?? "");
  const [amount, setAmount] = useState({ days: 0, hours: 0, minutes: 0, later: true });

  useEffect(() => {
    datesApi
      .of(ids)
      .then((d) => {
        setInfo(d);
        setDate((s) => s || d.folderDate || (d.first != null ? dayOf(d.first) : ""));
      })
      .catch((e) => setError(String(e)));
  }, [ids]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const validDay = /^\d{4}-\d{2}-\d{2}$/.test(date) && !Number.isNaN(dayNumber(date));
  const seconds =
    mode === "to"
      ? info?.first != null && validDay
        ? (dayNumber(date) - Math.floor(info.first / DAY)) * DAY
        : 0
      : (amount.later ? 1 : -1) * (amount.days * DAY + amount.hours * 3600 + amount.minutes * 60);
  const how: DateChange = mode === "to" ? { kind: "to", date } : { kind: "by", seconds };
  const apply = async (change: DateChange) => {
    setBusy(true);
    setError(null);
    try {
      onDone(await datesApi.change(ids, change), change);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  const num = (v: string) => Math.max(0, Math.floor(Number(v) || 0));
  const before = (ts: number) => fmtDay(ts);
  const after = (ts: number) => fmtDay(ts + seconds);

  return (
    <div
      className="modal-back"
      onClick={(e) => {
        e.stopPropagation();
        if (!busy) onClose();
      }}
    >
      <div className="modal fix-dates-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>
            Fix dates <span className="muted small">{photos(ids.length)}</span>
          </h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>
        {!info && !error && <p className="muted">Reading their dates…</p>}
        {info && info.first == null && <p className="muted">None of these photos has a date to move.</p>}
        {info && info.first != null && info.last != null && (
          <>
            <p className="muted">
              Taken {takenSpan(info.first, info.last)}
              {info.dated < info.photos && `; ${photos(info.photos - info.dated)} without a date stay as they are`}.
              {info.corrected > 0 && ` ${info.corrected === info.photos ? "All" : fmtNum(info.corrected)} corrected already.`}
            </p>
            <div className="fix-dates-choices" role="radiogroup">
              <label className="check">
                <input type="radio" checked={mode === "to"} onChange={() => setMode("to")} />
                <span>
                  Move to{" "}
                  <input
                    type="date"
                    value={date}
                    onChange={(e) => {
                      setDate(e.target.value);
                      setMode("to");
                    }}
                  />{" "}
                  keeping the time of day
                  {info.folderDate && date === info.folderDate && <span className="muted small"> · the date of {info.folder}</span>}
                </span>
              </label>
              <label className="check">
                <input type="radio" checked={mode === "by"} onChange={() => setMode("by")} />
                <span className="fix-dates-by" onFocus={() => setMode("by")}>
                  Move{" "}
                  <input
                    type="number"
                    min={0}
                    value={amount.days}
                    onChange={(e) => setAmount((a) => ({ ...a, days: num(e.target.value) }))}
                    aria-label="Days"
                  />{" "}
                  days{" "}
                  <input
                    type="number"
                    min={0}
                    value={amount.hours}
                    onChange={(e) => setAmount((a) => ({ ...a, hours: num(e.target.value) }))}
                    aria-label="Hours"
                  />{" "}
                  h{" "}
                  <input
                    type="number"
                    min={0}
                    value={amount.minutes}
                    onChange={(e) => setAmount((a) => ({ ...a, minutes: num(e.target.value) }))}
                    aria-label="Minutes"
                  />{" "}
                  min{" "}
                  <select
                    value={amount.later ? "later" : "earlier"}
                    onChange={(e) => setAmount((a) => ({ ...a, later: e.target.value === "later" }))}
                    aria-label="Later or earlier"
                  >
                    <option value="later">later</option>
                    <option value="earlier">earlier</option>
                  </select>
                </span>
              </label>
            </div>
            <dl className="fix-dates-preview">
              <dt>First</dt>
              <dd>
                {before(info.first)} <span className="muted">→</span> {seconds ? after(info.first) : "unchanged"}
              </dd>
              {info.last !== info.first && (
                <>
                  <dt>Last</dt>
                  <dd>
                    {before(info.last)} <span className="muted">→</span> {seconds ? after(info.last) : "unchanged"}
                  </dd>
                </>
              )}
            </dl>
            <p className="muted small">
              Only Shoebox's catalog changes: the files keep the date they give, and it can be put back at any time.
            </p>
          </>
        )}
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          {info && info.corrected > 0 && (
            <button
              className="btn ghost fix-dates-back"
              disabled={busy}
              title="Their dates as their files give them again"
              onClick={() => apply({ kind: "back" })}
            >
              Back to the files' dates
            </button>
          )}
          <button className="btn" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="btn primary"
            disabled={busy || !info || info.first == null || seconds === 0 || (mode === "to" && !validDay)}
            onClick={() => apply(how)}
          >
            {busy ? "Moving…" : mode === "to" && validDay ? `Move to ${fmtDateKey(date)}` : `Move ${photos(info?.dated ?? ids.length)}`}
          </button>
        </div>
      </div>
    </div>
  );
}
