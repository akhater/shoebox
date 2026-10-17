import { Check, RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { faceUrl, fmtNum, people as peopleApi, type CheckFace, type FaceTarget, type PeopleCheck, type Person } from "../api";

interface Props {
  /** Everyone in People, for names and pictures (reloaded after each change). */
  everyone: Person[];
  /** Something moved or merged (or was undone): People is reloaded. */
  onChanged: () => void;
  onClose: () => void;
}

/** A face in a card, with why it is there. */
interface CardFace {
  face: CheckFace;
  title: string;
}

/** What a card offers: faces to move (ticked to start with when suggested), or two people to merge. */
type Card = {
  key: string;
  /** The people it is about: once one of them is gone (merged), it waits for "Check again". */
  people: number[];
  head: string;
  note: string;
} & (
  | {
      kind: "faces";
      faces: CardFace[];
      /** Shown photo by photo (someone twice in each). */
      byPhoto?: boolean;
      ticked: number[];
      to: FaceTarget;
      verb: (n: number) => string;
    }
  | { kind: "merge"; personId: number; otherId: number; together: number }
);

/** What was done to a card, and its Undo. */
interface Done {
  text: string;
  undo: () => Promise<unknown>;
}

const pct = (x: number) => `${Math.round(x * 100)}%`;
const faces = (n: number) => (n === 1 ? "1 face" : `${fmtNum(n)} faces`);

/** The check's findings as cards, by kind: one action each. */
function cardsOf(check: PeopleCheck, name: (id: number) => string) {
  const byKey = <T,>(items: T[], key: (t: T) => string) => {
    const m = new Map<string, T[]>();
    for (const t of items) m.set(key(t), [...(m.get(key(t)) ?? []), t]);
    return [...m.values()];
  };
  const twice: Card[] = byKey(check.twice, (t) => `${t.personId}`).map((ts) => {
    const who = name(ts[0].personId);
    const all = ts.flatMap((t) => t.faces.map((f) => ({ face: f, title: `${pct(f.similarity)} like ${who}'s other faces` })));
    return {
      key: `twice:${ts[0].personId}`,
      people: [ts[0].personId],
      head: `${who} is twice in ${ts.length === 1 ? "a photo" : `${fmtNum(ts.length)} photos`}`,
      note: `In each photo one face is someone else. The ticked ones look less like ${who}: they go to whoever they resemble.`,
      kind: "faces",
      faces: all,
      byPhoto: true,
      ticked: ts.flatMap((t) => t.elsewhere),
      to: { kind: "elsewhere" },
      verb: (n) => `Move ${faces(n)} out`,
    };
  });
  const ages: Card[] = byKey(check.ageJumps, (a) => `${a.personId}`).map((as) => {
    const who = name(as[0].personId);
    return {
      key: `age:${as[0].personId}`,
      people: [as[0].personId],
      head: `${faces(as.length)} in ${who} from another age`,
      note: `Unlike ${who} the year they were taken, like ${who} years apart: a brother or sister, a child or a parent?`,
      kind: "faces",
      faces: as.map((a) => ({
        face: a.face,
        title: `${a.year}: ${pct(a.own)} like ${who} that year, ${pct(a.like)} like ${who} in ${a.likeYear}`,
      })),
      ticked: as.map((a) => a.face.faceId),
      to: { kind: "elsewhere" },
      verb: (n) => `Not ${who}: move ${faces(n)} out`,
    };
  });
  const others = (near: boolean): Card[] =>
    byKey(
      check.likeOthers.filter((l) => l.near === near),
      (l) => `${l.personId}:${l.otherId}`,
    ).map((ls) => {
      const [who, other] = [name(ls[0].personId), name(ls[0].otherId)];
      return {
        key: `like:${near}:${ls[0].personId}:${ls[0].otherId}`,
        people: [ls[0].personId, ls[0].otherId],
        head: `${faces(ls.length)} in ${who} look like ${other}`,
        note: near
          ? `Much more like ${other} than like ${who}, in photos taken within a month of ${other}'s.`
          : `Much more like ${other}, but months or years from ${other}'s photos: check them, babies look alike.`,
        kind: "faces",
        faces: ls.map((l) => ({
          face: l.face,
          title: `${pct(l.other)} like ${other}, ${pct(l.face.similarity)} like ${who}${l.face.byHand ? " · placed by you" : ""}`,
        })),
        ticked: near ? ls.map((l) => l.face.faceId) : [],
        to: { kind: "person", id: ls[0].otherId },
        verb: (n) => `Move ${faces(n)} to ${other}`,
      };
    });
  const same: Card[] = check.samePeople.map((s) => {
    const [a, b] = [name(s.personId), name(s.otherId)];
    return {
      key: `same:${s.personId}:${s.otherId}`,
      people: [s.personId, s.otherId],
      head: s.sameName ? `Two people called ${a}` : `${a} and ${b} look alike`,
      note:
        s.together > 0
          ? `In ${s.together === 1 ? "a photo" : `${fmtNum(s.together)} photos`} together: two people. Give one another name.`
          : `${pct(s.similarity)} alike and never in the same photo: one person${s.sameName ? "" : ", at two ages"}?`,
      kind: "merge",
      personId: s.personId,
      otherId: s.otherId,
      together: s.together,
    };
  });
  return [
    { title: "Twice in one photo", cards: twice },
    { title: "A face from another age", cards: ages },
    { title: "More like someone else", cards: others(true) },
    { title: "Unsure: more like someone else, far apart in time", cards: others(false) },
    { title: "One person under two names?", cards: same },
  ].filter((s) => s.cards.length);
}

/** "Check" in People: likely mix-ups among named people, by kind, each with
 *  its faces and one action - moves and merges as everywhere else, each with
 *  Undo on its card. Nothing changes until a button is pressed. */
export function PeopleCheckDialog({ everyone, onChanged, onClose }: Props) {
  const [check, setCheck] = useState<PeopleCheck | null>(null);
  const [run, setRun] = useState(0);
  const [error, setError] = useState<string | null>(null);
  // Faces unticked or ticked by the user, from what each card ticks to start with.
  const [flipped, setFlipped] = useState<Set<number>>(() => new Set());
  const [done, setDone] = useState<Map<string, Done>>(() => new Map());
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    setCheck(null);
    setError(null);
    peopleApi
      .check()
      .then((c) => {
        setCheck(c);
        setFlipped(new Set());
        setDone(new Map());
      })
      .catch((e) => setError(String(e)));
  }, [run]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const known = new Map(everyone.map((p) => [p.id, p]));
  const name = (id: number) => known.get(id)?.name ?? "someone";
  const sections = check ? cardsOf(check, name) : [];
  const flip = (id: number) =>
    setFlipped((f) => {
      const next = new Set(f);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  /** Runs a card's action; its card then says what was done, with Undo. */
  const act = async (key: string, work: () => Promise<Done | null>) => {
    setBusy(key);
    setError(null);
    try {
      const d = await work();
      if (d) setDone((m) => new Map(m).set(key, d));
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };
  const undo = (key: string, d: Done) =>
    act(key, async () => {
      await d.undo();
      setDone((m) => {
        const next = new Map(m);
        next.delete(key);
        return next;
      });
      return null;
    });

  const card = (c: Card) => {
    const did = done.get(c.key);
    const gone = !did && c.people.some((id) => !known.has(id));
    const ticked = c.kind === "faces" ? c.faces.filter((f) => c.ticked.includes(f.face.faceId) !== flipped.has(f.face.faceId)) : [];
    const tile = ({ face, title }: CardFace) => {
      const on = ticked.some((t) => t.face.faceId === face.faceId);
      return (
        <button
          key={face.faceId}
          className={`stray ${on ? "on" : ""}`}
          disabled={!!did || gone}
          onClick={() => flip(face.faceId)}
          title={title}
        >
          <img src={faceUrl(face.faceId)} alt="" loading="lazy" draggable={false} />
          <span className={`tick ${on ? "on" : ""}`}>
            <Check size={13} />
          </span>
          {face.byHand && <span className="by-hand">yours</span>}
        </button>
      );
    };
    return (
      <div key={c.key} className={`check-card ${did ? "done" : ""}`}>
        <div className="check-card-head">
          <strong>{c.head}</strong>
          <span className="muted small">{c.note}</span>
        </div>
        {c.kind === "faces" && c.byPhoto ? (
          <div className="check-photos">
            {[...new Set(c.faces.map((f) => f.face.photoId))].map((photo) => (
              <div key={photo} className="check-photo" title="One photo">
                {c.faces.filter((f) => f.face.photoId === photo).map(tile)}
              </div>
            ))}
          </div>
        ) : c.kind === "faces" ? (
          <div className="strays-grid">{c.faces.map(tile)}</div>
        ) : (
          <div className="check-pair">
            {[c.personId, c.otherId].map((id) => (
              <figure key={id}>
                {known.get(id) && <img src={faceUrl(known.get(id)!.cover)} alt="" loading="lazy" draggable={false} />}
                <figcaption>
                  {name(id)}
                  <span className="muted small"> · {fmtNum(known.get(id)?.photos ?? 0)} photos</span>
                </figcaption>
              </figure>
            ))}
          </div>
        )}
        <div className="check-card-foot">
          {did ? (
            <>
              <span className="muted">{did.text}</span>
              <button className="btn small" disabled={!!busy} onClick={() => undo(c.key, did)}>
                Undo
              </button>
            </>
          ) : gone ? (
            <span className="muted">Changed since the check: check again.</span>
          ) : c.kind === "faces" ? (
            <button
              className="btn small primary"
              disabled={!!busy || ticked.length === 0}
              onClick={() => {
                const ids = ticked.map((t) => t.face.faceId);
                act(c.key, async () => {
                  const moved = await peopleApi.reassign(ids, c.to);
                  // A face already theirs in the photo made room (a person is once per photo).
                  const room = moved.bumped.length ? `, ${faces(moved.bumped.length)} made room` : "";
                  return { text: `${faces(moved.faces)} moved${room}.`, undo: () => peopleApi.restore(moved.undo) };
                });
              }}
            >
              {busy === c.key ? "Moving…" : c.verb(ticked.length)}
            </button>
          ) : c.together === 0 ? (
            <button
              className="btn small primary"
              disabled={!!busy}
              title={`${name(c.otherId)} joins ${name(c.personId)}, who keeps the name`}
              onClick={() =>
                act(c.key, async () => {
                  const merged = await peopleApi.merge([c.personId, c.otherId], c.personId);
                  return { text: `Now one person: ${name(c.personId)}.`, undo: () => peopleApi.unmerge(merged) };
                })
              }
            >
              {busy === c.key ? "Merging…" : "Same person"}
            </button>
          ) : null}
        </div>
      </div>
    );
  };

  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <div className="modal strays-modal check-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Check people</h2>
          <button className="btn small ghost" disabled={!check || !!busy} onClick={() => setRun((n) => n + 1)}>
            <RefreshCw size={14} /> Check again
          </button>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={!!busy}>
            <X size={18} />
          </button>
        </div>
        {!check && !error && <p className="muted">Comparing every named person's faces…</p>}
        {check && (
          <p className="muted">
            {sections.length
              ? `Likely mix-ups among ${fmtNum(check.people)} named people (${fmtNum(check.faces)} faces). Untick a face to leave it where it is; each change has Undo.`
              : `No likely mix-ups among ${fmtNum(check.people)} named people (${fmtNum(check.faces)} faces).`}
          </p>
        )}
        {sections.length > 0 && (
          <div className="check-list">
            {sections.map((s) => (
              <section key={s.title} className="check-section">
                <h3>
                  {s.title} <span className="muted">{fmtNum(s.cards.length)}</span>
                </h3>
                {s.cards.map(card)}
              </section>
            ))}
          </div>
        )}
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          <button className="btn" onClick={onClose} disabled={!!busy}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
