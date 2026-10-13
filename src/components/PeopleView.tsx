import { Check, Eye, EyeOff, Merge, Pencil, X } from "lucide-react";
import { useState } from "react";
import { faceUrl, fmtNum, people as peopleApi, type Person } from "../api";

interface Props {
  people: Person[];
  onOpen: (p: Person) => void;
  onChanged: () => void;
  onError: (e: string) => void;
}

/** Everyone the face recogniser found: open, name, merge, ignore strangers. */
export function PeopleView({ people, onOpen, onChanged, onError }: Props) {
  const [selected, setSelected] = useState<number[]>([]);
  const [editing, setEditing] = useState<number | null>(null);
  const [showOnce, setShowOnce] = useState(false);
  const [showIgnored, setShowIgnored] = useState(false);

  const active = people.filter((p) => !p.ignored);
  const ignored = people.filter((p) => p.ignored);
  // People seen in one photo only are mostly passers-by: hidden unless asked.
  const seenOnce = active.filter((p) => !p.name && p.photos < 2);
  const shown = showIgnored ? ignored : showOnce ? active : active.filter((p) => p.name || p.photos >= 2);
  const selecting = selected.length > 0;

  const toggle = (id: number) => setSelected((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]));

  const act = async (f: () => Promise<void>) => {
    try {
      await f();
      setSelected([]);
      onChanged();
    } catch (e) {
      onError(String(e));
    }
  };

  const rename = (p: Person, name: string) => {
    setEditing(null);
    if (name.trim() !== (p.name ?? "")) act(() => peopleApi.rename(p.id, name));
  };

  const merge = () => {
    // Keep a named person if there is one, else the one with most photos.
    const chosen = people.filter((p) => selected.includes(p.id));
    const into = (chosen.find((p) => p.name) ?? [...chosen].sort((a, b) => b.photos - a.photos)[0]).id;
    act(() => peopleApi.merge(selected, into));
  };

  if (!people.length) {
    return (
      <div className="empty">
        <div className="empty-card">
          <h2>No people yet</h2>
          <p>Faces are found in the background after each scan. They'll appear here as they're grouped.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="people-wrap">
      <div className="people-bar">
        {selecting ? (
          <>
            <span>
              <b>{selected.length}</b> selected
            </span>
            {!showIgnored && (
              <button className="btn small primary" disabled={selected.length < 2} onClick={merge}>
                <Merge size={14} /> Merge into one person
              </button>
            )}
            <button className="btn small" onClick={() => act(() => peopleApi.ignore(selected, !showIgnored))}>
              {showIgnored ? <Eye size={14} /> : <EyeOff size={14} />} {showIgnored ? "Show again" : "Ignore"}
            </button>
            <button className="btn small ghost" onClick={() => setSelected([])}>
              <X size={14} /> Cancel
            </button>
          </>
        ) : showIgnored ? (
          <span className="muted">Ignored people: hidden from People, their new faces still join them.</span>
        ) : (
          <span className="muted">Click a face for their photos · click the name to rename · tick to merge or ignore</span>
        )}
        <span className="grow" />
        {!showIgnored && seenOnce.length > 0 && (
          <>
            <button className="btn small ghost" onClick={() => setShowOnce(!showOnce)}>
              {showOnce ? "Hide" : "Show"} {fmtNum(seenOnce.length)} seen only once
            </button>
            <button
              className="btn small ghost"
              title="Street photography: strangers you'll never name"
              onClick={() => act(() => peopleApi.ignore(seenOnce.map((p) => p.id), true))}
            >
              <EyeOff size={14} /> Ignore everyone seen only once
            </button>
          </>
        )}
        {!showIgnored && (
          <button
            className="btn small ghost"
            title="Sort again every face that isn't named, ignored or placed by hand"
            onClick={() => act(() => peopleApi.regroup().then(() => {}))}
          >
            Regroup faces
          </button>
        )}
        {(ignored.length > 0 || showIgnored) && (
          <button
            className="btn small ghost"
            onClick={() => {
              setSelected([]);
              setShowIgnored(!showIgnored);
            }}
          >
            {showIgnored ? "Back to people" : `Ignored (${fmtNum(ignored.length)})`}
          </button>
        )}
      </div>
      <div className="people-grid">
        {shown.map((p) => {
          const isSel = selected.includes(p.id);
          return (
            <div key={p.id} className={`person ${isSel ? "selected" : ""} ${p.ignored ? "ignored" : ""}`}>
              <button
                className="person-face"
                onClick={() => (selecting ? toggle(p.id) : onOpen(p))}
                title={selecting ? "Select" : "Show their photos"}
              >
                <img src={faceUrl(p.cover)} alt="" loading="lazy" draggable={false} />
                <span
                  className={`tick ${isSel ? "on" : ""}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggle(p.id);
                  }}
                  title="Select"
                >
                  <Check size={13} />
                </span>
                <span
                  className="eye"
                  onClick={(e) => {
                    e.stopPropagation();
                    act(() => peopleApi.ignore([p.id], !p.ignored));
                  }}
                  title={p.ignored ? "Show again" : "Ignore (a stranger)"}
                >
                  {p.ignored ? <Eye size={13} /> : <EyeOff size={13} />}
                </span>
              </button>
              {editing === p.id ? (
                <input
                  className="person-name-input"
                  autoFocus
                  defaultValue={p.name ?? ""}
                  placeholder="Name"
                  onBlur={(e) => rename(p, e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    if (e.key === "Escape") setEditing(null);
                  }}
                />
              ) : (
                <button className={`person-name ${p.name ? "" : "unnamed"}`} onClick={() => setEditing(p.id)}>
                  {p.name ?? "Add a name"} <Pencil size={11} />
                </button>
              )}
              <span className="muted small">{fmtNum(p.photos)} photos</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
