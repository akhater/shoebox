import { UserPlus, UserX, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { faceUrl, fmtNum, people, type FaceTarget, type Moved, type Person } from "../api";

/** How many face pictures the dialog shows before "+N". */
const SHOWN = 24;

interface Props {
  faceIds: number[];
  /** The person they leave (null: faces nobody is in yet, too small to group). */
  from: Pick<Person, "id" | "name"> | null;
  everyone: Person[];
  /** `to`: who they went to, in words, for a message. */
  onDone: (moved: Moved, to: string) => void;
  onClose: () => void;
}

/** One row of the list: someone in People, or someone new by the name typed. */
type Row = { person: Person; hint?: string } | { name: string };

/** "Who is this?": faces go to someone in People (suggested by likeness, or
 *  found by name), to someone new by the name typed, or - not them, don't
 *  know who - to whoever they resemble most. Placed by hand: regrouping never
 *  moves them back. ↑↓ and Enter pick, Esc closes. */
export function WhoIsIt({ faceIds, from, everyone, onDone, onClose }: Props) {
  const [text, setText] = useState("");
  const [found, setFound] = useState<Person[] | null>(null);
  const [alike, setAlike] = useState<Person[]>([]);
  const [active, setActive] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);
  const fromName = from ? (from.name ?? "this person") : null;
  const one = faceIds.length === 1;

  // Before anything is typed: who these faces look like - named people
  // first, then groups People shows (seen in 2+ photos), most alike first.
  useEffect(() => {
    people
      .lookAlikes(faceIds, from ? [from.id] : [])
      .then((l) => {
        const shown = l
          .map((a) => everyone.find((p) => p.id === a.personId))
          .filter((p): p is Person => !!p && !p.ignored && (!!p.name || p.photos > 1));
        setAlike([...shown.filter((p) => p.name), ...shown.filter((p) => !p.name)].slice(0, 5));
      })
      .catch(() => {});
  }, []); // the faces and person don't change while the dialog is open

  // Typing: everyone whose name matches (as People's search box finds them).
  useEffect(() => {
    const q = text.trim();
    const n = ++seq.current;
    if (!q) {
      setFound(null);
      return;
    }
    people
      .find(q)
      .then((f) => n === seq.current && setFound(f.filter((p) => p.id !== from?.id).slice(0, 6)))
      .catch(() => {});
  }, [text, from?.id]);
  useEffect(() => setActive(0), [found, alike]);

  const typed = text.trim();
  const rows: Row[] = [
    ...(found ?? alike).map((p) => ({ person: p, hint: found ? undefined : "looks like" })),
    ...(typed ? [{ name: typed }] : []),
  ];

  const go = async (to: FaceTarget, label: string) => {
    if (busy) return;
    setBusy(true);
    try {
      onDone(await people.reassign(faceIds, to), label);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  const pick = (r: Row) =>
    "person" in r
      ? go({ kind: "person", id: r.person.id }, r.person.name ?? "an unnamed person")
      : go({ kind: "new", name: r.name }, r.name);
  const elsewhere = () => go({ kind: "elsewhere" }, "whoever they look like");

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) {
        e.stopPropagation(); // not also the viewer or Back
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [busy, onClose]);

  return (
    <div className="modal-back" onClick={busy ? undefined : onClose}>
      <div className="modal who-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>{one ? "Who is this?" : `Who are these ${fmtNum(faceIds.length)} faces?`}</h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
            <X size={18} />
          </button>
        </div>
        <div className={`who-faces ${one ? "one" : ""}`}>
          {faceIds.slice(0, SHOWN).map((id) => (
            <img key={id} src={faceUrl(id)} alt="" draggable={false} />
          ))}
          {faceIds.length > SHOWN && <span className="muted">+{fmtNum(faceIds.length - SHOWN)}</span>}
        </div>
        <p className="muted small">
          {fromName ? `Not ${fromName}? ` : ""}Pick who {one ? "it is" : "they are"}, or type a new name. Shoebox
          won't move {one ? "it" : "them"} again.
        </p>
        <input
          className="who-input"
          autoFocus
          value={text}
          placeholder="Name…"
          spellCheck={false}
          disabled={busy}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Typing here never drives the viewer or the grid (arrows, Delete, Z...).
            e.stopPropagation();
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => Math.max(0, Math.min(rows.length - 1, a + (e.key === "ArrowDown" ? 1 : -1))));
            } else if (e.key === "Enter" && rows[active]) pick(rows[active]);
          }}
          role="combobox"
          aria-expanded={rows.length > 0}
          aria-autocomplete="list"
        />
        <div className="who-list" role="listbox">
          {rows.map((r, i) => (
            <button
              key={"person" in r ? r.person.id : "new"}
              role="option"
              aria-selected={i === active}
              className={`name-suggest-row ${i === active ? "active" : ""}`}
              onMouseEnter={() => setActive(i)}
              onClick={() => pick(r)}
              disabled={busy}
            >
              {"person" in r ? (
                <>
                  <img src={faceUrl(r.person.cover)} alt="" />
                  <span className={`grow ellipsis ${r.person.name ? "" : "unnamed"}`}>{r.person.name ?? "Unnamed person"}</span>
                  <span className="muted small">
                    {r.hint && `${r.hint} · `}
                    {r.person.ignored && "ignored · "}
                    {fmtNum(r.person.photos)} photos
                  </span>
                </>
              ) : (
                <>
                  <span className="who-new">
                    <UserPlus size={15} />
                  </span>
                  <span className="grow ellipsis">
                    New person “<b>{r.name}</b>”
                  </span>
                </>
              )}
            </button>
          ))}
        </div>
        {error && <p className="note warn">{error}</p>}
        <div className="modal-foot">
          <button className="btn" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          {fromName && (
            <button
              className="btn"
              onClick={elsewhere}
              disabled={busy}
              title={`Each face joins whoever it looks like most (never ${fromName}), or becomes someone new`}
            >
              <UserX size={14} /> Not {fromName}, don't know who
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
