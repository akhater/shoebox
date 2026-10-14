import { Check, X } from "lucide-react";
import { useEffect, useState } from "react";
import { faceUrl, fmtNum, people, type Moved, type Person, type Stray } from "../api";
import { WhoIsIt } from "./WhoIsIt";

interface Props {
  person: Person;
  everyone: Person[];
  /** `to`: who the faces went to, in words. */
  onDone: (moved: Moved, to: string) => void;
  onClose: () => void;
}

/** "Clean up": faces in a person that don't look like them, to move out -
 *  or any of their faces, with "Show all faces". */
export function StraysDialog({ person, everyone, onDone, onClose }: Props) {
  const [strays, setStrays] = useState<Stray[] | null>(null);
  // Every face, the least alike first (loaded when asked for).
  const [all, setAll] = useState<Stray[] | null>(null);
  const [showAll, setShowAll] = useState(false);
  // Ticked faces: the unlike ones start ticked; with all faces shown, none
  // does (picking by hand among hundreds).
  const [tickedUnlike, setTickedUnlike] = useState<Set<number>>(() => new Set());
  const [tickedAll, setTickedAll] = useState<Set<number>>(() => new Set());
  const [ticked, setTicked] = showAll ? [tickedAll, setTickedAll] : [tickedUnlike, setTickedUnlike];
  const [busy, setBusy] = useState(false);
  const [who, setWho] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    people
      .strays(person.id)
      .then((s) => {
        setStrays(s);
        setTickedUnlike(new Set(s.map((x) => x.faceId)));
      })
      .catch((e) => setError(String(e)));
  }, [person.id]);

  useEffect(() => {
    if (!showAll || all) return;
    people
      .faces(person.id)
      .then(setAll)
      .catch((e) => setError(String(e)));
  }, [showAll, all, person.id]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy && !who) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, who, onClose]);

  const name = person.name ?? "this person";
  const shown = showAll ? all : strays;
  const out = (shown ?? []).filter((s) => ticked.has(s.faceId)).map((s) => s.faceId);
  const toggle = (id: number) =>
    setTicked((t) => {
      const next = new Set(t);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  const moveOut = async () => {
    setBusy(true);
    try {
      onDone(await people.reassign(out, { kind: "elsewhere" }), "whoever they look like");
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <>
      <div className="modal-back" onClick={busy ? undefined : onClose}>
        <div className="modal strays-modal" onClick={(e) => e.stopPropagation()}>
          <div className="modal-head">
            <h2>Clean up {name}</h2>
            <button
              className={`btn small ${showAll ? "" : "ghost"}`}
              onClick={() => setShowAll(!showAll)}
              title="Pick any face yourself, the least alike first"
            >
              {showAll ? "Only unlike faces" : `Show all ${fmtNum(person.faces)} faces`}
            </button>
            <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
              <X size={18} />
            </button>
          </div>
          {!shown && !error && <p className="muted">Comparing {fmtNum(person.faces)} faces…</p>}
          {shown && !showAll && shown.length === 0 && (
            <p>Every face looks like {name}. To take one out anyway, show all faces and tick it.</p>
          )}
          {shown && shown.length > 0 && (
            <>
              <p className="muted">
                {showAll
                  ? `All ${fmtNum(shown.length)} faces of ${name}, the least alike first. Tick the ones that aren't ${name}.`
                  : `${fmtNum(shown.length)} of ${fmtNum(person.faces)} faces don't look like ${name}'s other faces. Untick any that really are ${name}.`}{" "}
                Ticked ones leave {name}: to whoever they resemble, or to someone you pick with “Move to…”.
              </p>
              <div className="strays-grid">
                {shown.map((s) => {
                  const on = ticked.has(s.faceId);
                  return (
                    <button
                      key={s.faceId}
                      className={`stray ${on ? "on" : ""}`}
                      onClick={() => toggle(s.faceId)}
                      title={`${Math.round(s.similarity * 100)}% like ${name}${s.byHand ? " · placed here by you" : ""}`}
                    >
                      <img src={faceUrl(s.faceId)} alt="" loading="lazy" draggable={false} />
                      <span className={`tick ${on ? "on" : ""}`}>
                        <Check size={13} />
                      </span>
                      {s.byHand && <span className="by-hand">yours</span>}
                    </button>
                  );
                })}
              </div>
            </>
          )}
          {error && <p className="note warn">{error}</p>}
          <div className="modal-foot">
            {!!shown?.length && (
              <button
                className="btn ghost strays-tick-all"
                disabled={busy}
                onClick={() => setTicked(new Set(out.length ? [] : shown.map((s) => s.faceId)))}
              >
                {out.length ? "Untick all" : "Tick all"}
              </button>
            )}
            <button className="btn" onClick={onClose} disabled={busy}>
              {shown?.length ? "Cancel" : "Close"}
            </button>
            {!!shown?.length && (
              <>
                <button className="btn" disabled={busy || out.length === 0} onClick={() => setWho(true)}>
                  Move to…
                </button>
                <button className="btn primary" disabled={busy || out.length === 0} onClick={moveOut}>
                  {busy ? "Moving…" : `Move ${out.length === 1 ? "1 face" : `${fmtNum(out.length)} faces`} out`}
                </button>
              </>
            )}
          </div>
        </div>
      </div>
      {who && (
        <WhoIsIt
          faceIds={out}
          from={person}
          everyone={everyone}
          onClose={() => setWho(false)}
          onDone={onDone}
        />
      )}
    </>
  );
}
