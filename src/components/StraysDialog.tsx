import { Check, X } from "lucide-react";
import { useEffect, useState } from "react";
import { faceUrl, fmtNum, people, type Person, type Stray } from "../api";

interface Props {
  person: Person;
  onDone: (moved: number) => void;
  onClose: () => void;
}

/** "Clean up": faces in a person that don't look like them, to move out. */
export function StraysDialog({ person, onDone, onClose }: Props) {
  const [strays, setStrays] = useState<Stray[] | null>(null);
  const [kept, setKept] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    people
      .strays(person.id)
      .then(setStrays)
      .catch((e) => setError(String(e)));
  }, [person.id]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const out = strays?.filter((s) => !kept.includes(s.faceId)) ?? [];
  const name = person.name ?? "this person";
  const toggle = (id: number) => setKept((k) => (k.includes(id) ? k.filter((x) => x !== id) : [...k, id]));

  const confirm = async () => {
    setBusy(true);
    try {
      onDone(await people.moveOut(person.id, out.map((s) => s.faceId)));
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <div className="modal strays-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Clean up {name}</h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>
        {!strays && !error && <p className="muted">Comparing {fmtNum(person.faces)} faces…</p>}
        {strays && strays.length === 0 && <p>Every face looks like {name}. Nothing to move.</p>}
        {strays && strays.length > 0 && (
          <>
            <p className="muted">
              {fmtNum(strays.length)} of {fmtNum(person.faces)} faces don't look like {name}'s other faces. Ticked ones
              leave {name} and join whoever they resemble, or become someone new. Untick any that really are {name}.
            </p>
            <div className="strays-grid">
              {strays.map((s) => {
                const on = !kept.includes(s.faceId);
                return (
                  <button
                    key={s.faceId}
                    className={`stray ${on ? "on" : ""}`}
                    onClick={() => toggle(s.faceId)}
                    title={`${Math.round(s.similarity * 100)}% like ${name}`}
                  >
                    <img src={faceUrl(s.faceId)} alt="" loading="lazy" draggable={false} />
                    <span className={`tick ${on ? "on" : ""}`}>
                      <Check size={13} />
                    </span>
                  </button>
                );
              })}
            </div>
          </>
        )}
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          <button className="btn" onClick={onClose} disabled={busy}>
            {strays?.length ? "Cancel" : "Close"}
          </button>
          {!!strays?.length && (
            <button className="btn primary" disabled={busy || out.length === 0} onClick={confirm}>
              {busy ? "Moving…" : `Move ${fmtNum(out.length)} faces out`}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
