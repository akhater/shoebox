import { LockKeyhole, LockKeyholeOpen, X } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { MIN_PASSWORD, PRIVATE_SHORTCUT, privateApi, type PrivateStatus } from "../api";

/**
 * The password box for private photos. "unlock" (Ctrl+Shift+L): any password opens its own
 * space, never refused - a new one opens an empty space. "confirm": the first photos made
 * private under a password ask for it again, so nothing goes into a mistyped one.
 */
export function PrivateDialog({
  mode,
  onUnlocked,
  onConfirmed,
  onClose,
}: {
  mode: "unlock" | "confirm";
  onUnlocked: (s: PrivateStatus) => void;
  /** The password typed again: makes the photos private (rejects when it isn't the same). */
  onConfirmed: (password: string) => Promise<void>;
  onClose: () => void;
}) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const box = useRef<HTMLInputElement>(null);
  const close = useRef(onClose);
  close.current = onClose;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close.current();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
  // The password box has the keyboard on opening, and again after a refusal.
  useEffect(() => {
    if (!busy) box.current?.focus();
  }, [busy, error]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    if (password.length < MIN_PASSWORD) return setError(`A password has at least ${MIN_PASSWORD} characters.`);
    setBusy(true);
    setError(null);
    try {
      if (mode === "unlock") onUnlocked(await privateApi.unlock(password));
      else await onConfirmed(password);
    } catch (err) {
      setError(String(err).replace(/^Error: /, ""));
      setPassword("");
      setBusy(false);
    }
  };

  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <form className="modal private-dialog" onClick={(e) => e.stopPropagation()} onSubmit={submit}>
        <div className="modal-head">
          <h2>
            <LockKeyhole size={16} /> {mode === "unlock" ? "Private photos" : "Type the password again"}
          </h2>
          <button type="button" className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>
        <div className="form">
          {mode === "confirm" && (
            <p className="lead">
              These are the first photos made private with this password. Each password keeps its own private photos,
              and a mistyped one would keep these where you can't find them.
            </p>
          )}
          <label>
            <span>Password</span>
            <input
              ref={box}
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={busy}
              aria-invalid={error != null}
            />
          </label>
          {error && (
            <p className="note warn" role="alert">
              {error}
            </p>
          )}
        </div>
        <div className="modal-foot">
          <button type="button" className="btn ghost" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={busy}>
            {busy ? "Opening…" : mode === "unlock" ? "Open" : "Make private"}
          </button>
        </div>
      </form>
    </div>
  );
}

const LOCK_AFTER = [1, 5, 15, 30, 60];

/** Settings > Private photos: how they work, lock and unlock, when they lock by themselves. */
export function PrivateTab({
  status,
  onStatus,
  onUnlock,
  lockMinutes,
  onLockMinutes,
}: {
  status: PrivateStatus | null;
  onStatus: (s: PrivateStatus) => void;
  /** The password dialog. */
  onUnlock: () => void;
  /** When they lock again by themselves (minutes without using Shoebox), saved at once. */
  lockMinutes: number | undefined;
  onLockMinutes: (minutes: number) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  if (!status) return <p className="muted small pad">…</p>;
  return (
    <div className="form private-tab">
      <p className="lead">
        Each password opens its own private photos, like a wallet's passphrase: no password is ever wrong, a new one
        opens an empty set, and none shows another's. Private photos are hidden from every view, count, search, People
        and agent until their password opens them; Shoebox keeps its own copies of them sealed, and the photo files on
        your disks stay as they are.
      </p>
      <div className="field">
        <span className="field-label">Status</span>
        <div className="row">
          <span className="grow">
            {status.unlocked ? (
              <>
                <LockKeyholeOpen size={14} className="inline-icon open" /> Open: its private photos are shown
              </>
            ) : (
              <>
                <LockKeyhole size={14} className="inline-icon" /> Locked
              </>
            )}
          </span>
          {status.unlocked ? (
            <button className="btn small" onClick={() => privateApi.lock().then(onStatus, (e) => setError(String(e)))}>
              Lock now
            </button>
          ) : (
            <button className="btn small" onClick={onUnlock}>
              Open…
            </button>
          )}
        </div>
        <small>
          {PRIVATE_SHORTCUT} opens and locks them from anywhere in Shoebox. While open, select photos and choose Make
          private.
        </small>
      </div>
      <label>
        <span>Lock again after</span>
        <select value={lockMinutes ?? 5} onChange={(e) => onLockMinutes(+e.target.value)} disabled={lockMinutes == null}>
          {LOCK_AFTER.map((m) => (
            <option key={m} value={m}>
              {m} minute{m === 1 ? "" : "s"}
              {m === 5 ? " (default)" : ""}
            </option>
          ))}
        </select>
        <small>Counted from the last time you used Shoebox. They also lock when it closes.</small>
      </label>
      <div className="field">
        <span className="field-label">Passwords</span>
        <small>
          A password can't be recovered or changed: its photos stay hidden in Shoebox for good if it is forgotten (their
          files on your disks are untouched). To keep photos under another password, make them not private, then
          private again with the new one.
        </small>
      </div>
      {error && (
        <p className="note warn" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
