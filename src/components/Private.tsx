import { HardDrive, KeyRound, Layers, LockKeyhole, LockKeyholeOpen, ShieldAlert, X } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { MIN_PASSWORD, PRIVATE_NAME_CHARS, PRIVATE_SHORTCUT, privateApi, type PrivateStatus } from "../api";

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

/**
 * What a password can't undo, once it has opened a space. Always shown when the space holds no
 * photos: a new password and a mistyped one look the same, and nothing else says so. For a
 * space with photos, until "Don't show this again" (Settings > Private photos turns it back on).
 */
export function PrivateNotice({
  photos,
  onDone,
  onRetry,
}: {
  /** Photos in the space just opened. */
  photos: number;
  /** Closes it; `quiet`: not again for passwords that have photos. */
  onDone: (quiet: boolean) => void;
  /** Locks again and asks for another password. */
  onRetry: () => void;
}) {
  const [quiet, setQuiet] = useState(false);
  const empty = photos === 0;
  const ok = useRef<HTMLButtonElement>(null);
  const done = useRef(() => onDone(quiet));
  done.current = () => onDone(quiet);

  useEffect(() => {
    ok.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        done.current();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  return (
    <div className="modal-back">
      <div className="modal private-notice" role="alertdialog" aria-labelledby="private-notice-title">
        <div className="modal-head">
          <h2 id="private-notice-title">
            <ShieldAlert size={16} /> {empty ? "This password has no private photos" : "About private passwords"}
          </h2>
        </div>
        <div className="form">
          {empty && (
            <p className="empty-space">
              It opened a new, empty space. That is expected for a new password. If you meant one you used before, it
              may be mistyped: Shoebox never says a password is wrong.
            </p>
          )}
          <ul className="notice-points">
            <li>
              <KeyRound size={16} />
              <div>
                <strong>A password can't be recovered.</strong> Not reset, not changed, not by Shoebox or anyone else.
                Forget it and its photos stay hidden in Shoebox for good. Keep it somewhere safe.
              </div>
            </li>
            <li>
              <Layers size={16} />
              <div>
                <strong>Every password opens its own space.</strong> No password is ever wrong: a new or mistyped one
                opens a new, empty space. Photos made private now go into this one, and only this password shows them.
              </div>
            </li>
            <li>
              <HardDrive size={16} />
              <div>
                <strong>Your photo files aren't locked.</strong> Shoebox seals only its own copies (previews,
                thumbnails, faces). The files on your disks stay as they are: anyone with the disk can open them.
              </div>
            </li>
          </ul>
          <div className="field">
            <label className="check">
              <input type="checkbox" checked={quiet} onChange={(e) => setQuiet(e.target.checked)} />
              Don't show this again for passwords that have photos
            </label>
            <small>A password with no photos always shows it.</small>
          </div>
        </div>
        <div className="modal-foot">
          {empty && (
            <button type="button" className="btn ghost" onClick={onRetry}>
              Try another password
            </button>
          )}
          <button ref={ok} type="button" className="btn primary" onClick={() => onDone(quiet)}>
            I understand
          </button>
        </div>
      </div>
    </div>
  );
}

/** The Private view's title being edited: the name of the space open now. */
export function SpaceNameEditor({
  name,
  onRename,
  onCancel,
}: {
  name: string | null;
  onRename: (name: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(name ?? "");
  const done = useRef(false);
  const finish = (f: () => void) => {
    if (done.current) return;
    done.current = true;
    f();
  };
  return (
    <div className="name-editor wide">
      <input
        className="title-name-input"
        autoFocus
        value={text}
        maxLength={PRIVATE_NAME_CHARS}
        placeholder="Name this private space"
        aria-label="Name of this private space"
        onChange={(e) => setText(e.target.value)}
        onBlur={() => finish(() => onRename(text))}
        onKeyDown={(e) => {
          if (e.key === "Enter") finish(() => onRename(text));
          else if (e.key === "Escape") {
            e.stopPropagation();
            finish(onCancel);
          }
        }}
      />
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
  warning,
  onWarning,
}: {
  status: PrivateStatus | null;
  onStatus: (s: PrivateStatus) => void;
  /** The password dialog. */
  onUnlock: () => void;
  /** When they lock again by themselves (minutes without using Shoebox), saved at once. */
  lockMinutes: number | undefined;
  onLockMinutes: (minutes: number) => void;
  /** The warning each time a password opens photos (see PrivateNotice), saved at once. */
  warning: boolean | undefined;
  onWarning: (on: boolean) => void;
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
                <LockKeyholeOpen size={14} className="inline-icon open" /> Open
                {status.name ? <> (“{status.name}”)</> : null}: its private photos are shown
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
      <div className="field">
        <span className="field-label">Warning</span>
        <label className="check">
          <input
            type="checkbox"
            checked={warning ?? true}
            onChange={(e) => onWarning(e.target.checked)}
            disabled={warning == null}
          />
          Warn me each time a password opens private photos
        </label>
        <small>A password with no photos is always warned about: a new one and a mistyped one look the same.</small>
      </div>
      {error && (
        <p className="note warn" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
