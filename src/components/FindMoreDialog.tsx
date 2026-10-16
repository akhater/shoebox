import { Check, X } from "lucide-react";
import { useEffect, useState } from "react";
import { faceUrl, fmtDateKey, fmtNum, people, type CandidateDay, type Moved, type Person } from "../api";

interface Props {
  person: Person;
  everyone: Person[];
  /** `to`: who the faces went to, in words. */
  onDone: (moved: Moved, to: string) => void;
  onClose: () => void;
}

/** "Find more": faces in other people that look more like this person than
 *  like the rest of the person they're in - Clean up the other way round -
 *  by day. The days near the person's own photos start ticked: to the face
 *  model babies look alike, while a person looks like themselves around the
 *  same time. Ticked faces move to them, placed by hand. */
export function FindMoreDialog({ person, everyone, onDone, onClose }: Props) {
  const [days, setDays] = useState<CandidateDay[] | null>(null);
  const [ticked, setTicked] = useState<Set<number>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    people
      .more(person.id)
      .then((d) => {
        setDays(d);
        setTicked(new Set(d.filter((x) => x.near).flatMap((x) => x.faces.map((f) => f.faceId))));
      })
      .catch((e) => setError(String(e)));
  }, [person.id]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const name = person.name ?? "this person";
  const theirs = person.name ? `${person.name.split(" ")[0]}'s` : "their";
  const all = days?.flatMap((d) => d.faces) ?? [];
  const chosen = all.filter((f) => ticked.has(f.faceId)).map((f) => f.faceId);
  /** Ticks them all, or unticks them all when they already are. */
  const toggle = (ids: number[]) =>
    setTicked((t) => {
      const next = new Set(t);
      const on = ids.every((id) => next.has(id));
      for (const id of ids) {
        if (on) next.delete(id);
        else next.add(id);
      }
      return next;
    });
  const who = (id: number) => everyone.find((p) => p.id === id)?.name ?? null;
  const pct = (x: number) => `${Math.round(x * 100)}%`;

  const move = async () => {
    setBusy(true);
    try {
      onDone(await people.reassign(chosen, { kind: "person", id: person.id }), name);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <div className="modal strays-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Find more of {name}</h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>
        {!days && !error && <p className="muted">Comparing every face with {name}…</p>}
        {days && all.length === 0 && <p>No face in anyone else looks more like {name} than like the person it's in.</p>}
        {all.length > 0 && (
          <>
            <p className="muted">
              {fmtNum(all.length)} {all.length === 1 ? "face" : "faces"} in other people look more like {name} than
              like the rest of the person they're in. The days near {theirs} own photos are
              ticked; check the other days yourself: to the face model, babies look alike. Ticked faces
              move to {name}.
            </p>
            <div className="more-days">
              {days!.map((d) => {
                const ids = d.faces.map((f) => f.faceId);
                const on = ids.every((id) => ticked.has(id));
                return (
                  <section key={d.day ?? "unknown"} className="more-day">
                    <button
                      className="more-day-head"
                      onClick={() => toggle(ids)}
                      aria-pressed={on}
                      title={on ? "Untick this day" : "Tick this day"}
                    >
                      <span className={`tick ${on ? "on" : ""}`}>
                        <Check size={13} />
                      </span>
                      <strong>{d.day ? fmtDateKey(d.day) : "Unknown date"}</strong>
                      <span className="muted">
                        {fmtNum(ids.length)} {ids.length === 1 ? "face" : "faces"}
                        {d.near ? ` · near ${theirs} photos` : ""}
                      </span>
                    </button>
                    <div className="strays-grid">
                      {d.faces.map((f) => {
                        const tickedHere = ticked.has(f.faceId);
                        const from = who(f.personId);
                        const now = f.now == null ? "their only face" : `${pct(f.now)} like them`;
                        return (
                          <button
                            key={f.faceId}
                            className={`stray ${tickedHere ? "on" : ""}`}
                            onClick={() => toggle([f.faceId])}
                            title={`${pct(f.similarity)} like ${name} · now in ${from ?? "an unnamed person"} (${now})${f.byHand ? " · placed there by you" : ""}`}
                          >
                            <img src={faceUrl(f.faceId)} alt="" loading="lazy" draggable={false} />
                            <span className={`tick ${tickedHere ? "on" : ""}`}>
                              <Check size={13} />
                            </span>
                            <span className="from">{from?.split(" ")[0] ?? "unnamed"}</span>
                          </button>
                        );
                      })}
                    </div>
                  </section>
                );
              })}
            </div>
          </>
        )}
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          {all.length > 0 && (
            <button
              className="btn ghost strays-tick-all"
              disabled={busy}
              onClick={() => setTicked(new Set(chosen.length ? [] : all.map((f) => f.faceId)))}
            >
              {chosen.length ? "Untick all" : "Tick all"}
            </button>
          )}
          <button className="btn" onClick={onClose} disabled={busy}>
            {all.length ? "Cancel" : "Close"}
          </button>
          {all.length > 0 && (
            <button className="btn primary" disabled={busy || chosen.length === 0} onClick={move}>
              {busy ? "Moving…" : `Move ${chosen.length === 1 ? "1 face" : `${fmtNum(chosen.length)} faces`} to ${name}`}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
