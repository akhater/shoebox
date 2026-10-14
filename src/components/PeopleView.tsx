import { Check, CheckCheck, ChevronRight, Eye, EyeOff, Merge, Pencil, Search, Sparkles, Square, X } from "lucide-react";
import { useEffect, useState } from "react";
import { faceUrl, fmtNum, modelsApi, people as peopleApi, type Person } from "../api";

interface Props {
  people: Person[];
  /** Whoever People's search box found, best first; null when it's empty. */
  found: Person[] | null;
  /** What's typed in it. */
  query: string;
  /** "Only new": just the people new or found in new photos, to check them. */
  newOnly: boolean;
  onNewOnly: (on: boolean) => void;
  onOpen: (p: Person) => void;
  onChanged: () => void;
  onError: (e: string) => void;
  /** Opens Settings > AI models (the face model isn't here). */
  onOpenModels: () => void;
}

type Group = "named" | "toName" | "ignored";

/** Which groups are folded, remembered between sessions (convenience only).
 *  Ignored starts folded. */
function useFolded() {
  const [folded, setFolded] = useState<Group[]>(() => {
    try {
      return JSON.parse(localStorage.getItem("shoebox.people.folded") ?? '["ignored"]') as Group[];
    } catch {
      return ["ignored"];
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem("shoebox.people.folded", JSON.stringify(folded));
    } catch {
      /* per-viewer convenience only */
    }
  }, [folded]);
  const toggle = (g: Group) => setFolded((f) => (f.includes(g) ? f.filter((x) => x !== g) : [...f, g]));
  return [folded, toggle] as const;
}

/** Everyone the face recogniser found: open, name, merge, ignore strangers.
 *  Named people and people still to name are two groups that fold away. */
export function PeopleView({ people, found, query, newOnly, onNewOnly, onOpen, onChanged, onError, onOpenModels }: Props) {
  const [selected, setSelected] = useState<number[]>([]);
  // With nobody yet: is the face model (and a runtime) here at all?
  const [facesReady, setFacesReady] = useState<boolean | null>(null);
  const none = people.length === 0;
  useEffect(() => {
    if (!none) return;
    modelsApi
      .list()
      .then((o) => setFacesReady(o.runtimeReady && o.models.some((m) => m.id === "faces" && m.state === "ready")))
      .catch(() => {});
  }, [none]);
  const [editing, setEditing] = useState<number | null>(null);
  const [showOnce, setShowOnce] = useState(false);
  const [folded, fold] = useFolded();

  // New people, and people found in new photos, since the last visit.
  const isFresh = (p: Person) => p.isNew || p.newPhotos > 0;
  const fresh = (list: Person[]) => list.filter(isFresh).length;
  // Narrowed by the search box (whoever's name matches, in its order) and by
  // "Only new". Nobody still to name has a name to match.
  const rank = found && new Map(found.map((p, i) => [p.id, i]));
  const narrowed = !!rank || newOnly;
  const only = (list: Person[]) => {
    const kept = rank ? list.filter((p) => rank.has(p.id)).sort((a, b) => rank.get(a.id)! - rank.get(b.id)!) : list;
    return newOnly ? kept.filter(isFresh) : kept;
  };
  const active = people.filter((p) => !p.ignored);
  const ignored = only(people.filter((p) => p.ignored));
  // People seen in one photo only are mostly passers-by: hidden unless asked.
  const seenOnce = active.filter((p) => !p.name && p.photos < 2);
  const named = only(active.filter((p) => p.name));
  const toName = rank ? [] : only(active.filter((p) => !p.name && (showOnce || p.photos >= 2)));
  // What "Only new" offers: the new among the people shown (as the sidebar counts them).
  const freshShown = fresh(named) + fresh(toName);
  const selecting = selected.length > 0;
  // With only ignored people selected, the bar offers "Show again" instead of "Ignore".
  const allIgnored = selecting && selected.every((id) => people.find((p) => p.id === id)?.ignored);

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

  /** "Same person": this one joins `into`, which keeps its name. */
  const mergeInto = (p: Person, into: Person) => {
    setEditing(null);
    act(() => peopleApi.merge([p.id, into.id], into.id));
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
          {facesReady === false ? (
            <>
              <p>Finding faces needs its AI model, which isn't on this computer yet.</p>
              <button className="btn primary" onClick={onOpenModels}>
                Get it in Settings › AI models
              </button>
            </>
          ) : (
            <p>Faces are found in the background after each scan. They'll appear here as they're grouped.</p>
          )}
        </div>
      </div>
    );
  }

  const card = (p: Person) => {
    const isSel = selected.includes(p.id);
    return (
      <div key={p.id} className={`person ${isSel ? "selected" : ""} ${p.ignored ? "ignored" : ""}`}>
        <button
          className="person-face"
          onClick={() => (selecting ? toggle(p.id) : onOpen(p))}
          title={selecting ? "Select" : "Show their photos"}
        >
          <img src={faceUrl(p.cover)} alt="" loading="lazy" draggable={false} />
          {p.isNew ? (
            <span className="new-badge" title="Someone new: open them to see their photos">
              New
            </span>
          ) : (
            p.newPhotos > 0 && (
              <span className="new-badge" title={`In ${fmtNum(p.newPhotos)} new photos: open them to see which`}>
                +{fmtNum(p.newPhotos)}
              </span>
            )
          )}
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
          <NameEditor
            person={p}
            everyone={people}
            onRename={(name) => rename(p, name)}
            onMerge={(into) => mergeInto(p, into)}
            onCancel={() => setEditing(null)}
          />
        ) : (
          <button className={`person-name ${p.name ? "" : "unnamed"}`} onClick={() => setEditing(p.id)}>
            {p.name ?? "Add a name"} <Pencil size={11} />
          </button>
        )}
        <span className="muted small">{fmtNum(p.photos)} photos</span>
      </div>
    );
  };

  // Narrowed, groups with nobody left are gone; while searching, the others are open.
  const group = (g: Group, title: string, list: Person[], empty: string) => {
    if (narrowed && !list.length) return null;
    const open = !!rank || !folded.includes(g);
    return (
      <section className="people-group">
        <button className="people-group-head" onClick={() => fold(g)} disabled={!!rank} aria-expanded={open}>
          <ChevronRight size={14} className={`chev ${open ? "open" : ""}`} />
          <span>{title}</span>
          <span className="count">{fmtNum(list.length)}</span>
          {fresh(list) > 0 && <span className="new-count">· {fmtNum(fresh(list))} new</span>}
        </button>
        {open &&
          (list.length ? <div className="people-grid">{list.map(card)}</div> : <p className="muted small people-empty">{empty}</p>)}
      </section>
    );
  };

  return (
    <div className="people-wrap">
      <div className="people-bar">
        {selecting ? (
          <>
            <span>
              <b>{selected.length}</b> selected
            </span>
            <button className="btn small primary" disabled={selected.length < 2} onClick={merge}>
              <Merge size={14} /> Merge into one person
            </button>
            <button className="btn small" onClick={() => act(() => peopleApi.ignore(selected, !allIgnored))}>
              {allIgnored ? <Eye size={14} /> : <EyeOff size={14} />} {allIgnored ? "Show again" : "Ignore"}
            </button>
            <button className="btn small ghost" onClick={() => setSelected([])}>
              <X size={14} /> Cancel
            </button>
          </>
        ) : (
          <span className="muted">Click a face for their photos · click the name to rename · tick to merge or ignore</span>
        )}
        <span className="grow" />
        {seenOnce.length > 0 && (
          <>
            <button className="btn small ghost" onClick={() => setShowOnce(!showOnce)}>
              {showOnce ? "Hide" : "Show"} {fmtNum(seenOnce.length)} seen only once
              {!showOnce && fresh(seenOnce) > 0 && ` (${fmtNum(fresh(seenOnce))} new)`}
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
        {(newOnly || freshShown > 0) && (
          <button
            className={`btn small ghost ${newOnly ? "on" : ""}`}
            aria-pressed={newOnly}
            onClick={() => onNewOnly(!newOnly)}
            title={
              newOnly
                ? "Show everyone again"
                : "Only the people who are new or were found in new photos. Open one to see just their new photos; once seen, they leave this list"
            }
          >
            <Sparkles size={14} /> Only new{freshShown > 0 && ` (${fmtNum(freshShown)})`}
          </button>
        )}
        {fresh(people) > 0 && (
          <button
            className="btn small ghost"
            title="Clear every New and +N without opening each person"
            onClick={() => act(() => peopleApi.readAll().then(() => {}))}
          >
            <CheckCheck size={14} /> Mark all as read
          </button>
        )}
        <button
          className="btn small ghost"
          title="Sort again every face that isn't named, ignored or placed by hand"
          onClick={() => act(() => peopleApi.regroup().then(() => {}))}
        >
          Regroup faces
        </button>
      </div>
      {narrowed && !named.length && !toName.length && !ignored.length && (
        <p className="muted people-none">
          {found && !found.length
            ? `No one named “${query.trim()}”. Only people with a name can be found: name them under their face.`
            : found
              ? `No one new named “${query.trim()}”.`
              : "Nothing new left to check."}{" "}
          {newOnly && (
            <button className="btn small ghost" onClick={() => onNewOnly(false)}>
              Show everyone
            </button>
          )}
        </p>
      )}
      {group("named", "Named", named, "Nobody named yet. Click “Add a name” under a face.")}
      {group("toName", "To name", toName, "Everyone found is named.")}
      {group("ignored", "Ignored", ignored, "Nobody ignored. Strangers you ignore go here; their new faces still join them.")}
    </div>
  );
}

/** Typing a name: people with a matching name are suggested (face, name,
 *  photos). Clicking one takes its name - a different person can share a
 *  name; ticking "Same person" merges into them. Enter keeps what's typed. */
export function NameEditor({
  person,
  everyone,
  onRename,
  onMerge,
  onCancel,
  wide,
}: {
  person: Person;
  everyone: Person[];
  onRename: (name: string) => void;
  onMerge: (into: Person) => void;
  onCancel: () => void;
  /** In a page title: bigger box, suggestions aligned left. */
  wide?: boolean;
}) {
  const [text, setText] = useState(person.name ?? "");
  const [done, setDone] = useState(false);
  const q = text.trim().toLowerCase();
  const matches = q
    ? everyone
        .filter((p) => p.id !== person.id && p.name && !p.ignored && p.name.toLowerCase().includes(q))
        .sort((a, b) => Number(!a.name!.toLowerCase().startsWith(q)) - Number(!b.name!.toLowerCase().startsWith(q)) || b.photos - a.photos)
        .slice(0, 6)
    : [];
  const finish = (f: () => void) => {
    if (done) return;
    setDone(true);
    f();
  };

  return (
    <div className={`name-editor ${wide ? "wide" : ""}`}>
      <input
        className={wide ? "title-name-input" : "person-name-input"}
        autoFocus
        value={text}
        placeholder="Name"
        onChange={(e) => setText(e.target.value)}
        onBlur={() => finish(() => onRename(text))}
        onKeyDown={(e) => {
          if (e.key === "Enter") finish(() => onRename(text));
          else if (e.key === "Escape") finish(onCancel);
          else if (e.key === "Tab" && matches.length && q) {
            e.preventDefault();
            setText(matches[0].name!);
          }
        }}
      />
      {matches.length > 0 && (
        <div className="name-suggest" onMouseDown={(e) => e.preventDefault()}>
          {matches.map((m) => (
            <div key={m.id} className="name-suggest-row">
              <img src={faceUrl(m.cover)} alt="" />
              <button className="grow" onClick={() => setText(m.name!)} title="Use this name (a different person)">
                <span className="ellipsis">{m.name}</span>
                <span className="muted small">{fmtNum(m.photos)} photos</span>
              </button>
              {/* A button, not a label: a label hands the keyboard to its box, the name box
                  loses it and saves what's typed as a name before the merge can happen. */}
              <button className="same-person" title={`Same person: merge into ${m.name}`} onClick={() => finish(() => onMerge(m))}>
                <Square size={13} /> Same person
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** People's search box, in the toolbar: names are suggested as you type -
 *  any part of the name, case and accents ignored, found by Shoebox itself
 *  (as `shoebox-cli people <name>`). ↑↓ and Enter open someone; Esc clears. */
export function PersonSearch({
  value,
  found,
  onChange,
  onOpen,
}: {
  value: string;
  /** Best first; null while the box is empty. */
  found: Person[] | null;
  onChange: (text: string) => void;
  onOpen: (p: Person) => void;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  useEffect(() => setActive(0), [found]);
  const shown = (found ?? []).slice(0, 8);
  const pick = (p: Person) => {
    setOpen(false);
    onOpen(p);
  };

  return (
    <div className="search person-search">
      <Search size={15} />
      <input
        value={value}
        onChange={(e) => {
          onChange(e.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            setOpen(true);
            setActive((a) => Math.max(0, Math.min(shown.length - 1, a + (e.key === "ArrowDown" ? 1 : -1))));
          } else if (e.key === "Enter" && open && shown[active]) pick(shown[active]);
          else if (e.key === "Escape") onChange("");
        }}
        placeholder="Find a person by name…"
        spellCheck={false}
        role="combobox"
        aria-expanded={open && !!found}
        aria-autocomplete="list"
      />
      {value && (
        <button className="icon-btn tiny-x" onClick={() => onChange("")} aria-label="Clear search">
          <X size={14} />
        </button>
      )}
      {open && found && (
        <div className="name-suggest person-suggest" role="listbox" onMouseDown={(e) => e.preventDefault()}>
          {shown.map((p, i) => (
            <button
              key={p.id}
              role="option"
              aria-selected={i === active}
              className={`name-suggest-row ${i === active ? "active" : ""}`}
              onMouseEnter={() => setActive(i)}
              onClick={() => pick(p)}
            >
              <img src={faceUrl(p.cover)} alt="" />
              <span className="grow ellipsis">{p.name}</span>
              <span className="muted small">
                {p.ignored && "ignored · "}
                {fmtNum(p.photos)} photos
              </span>
            </button>
          ))}
          {found.length > shown.length && (
            <p className="muted small person-suggest-note">{fmtNum(found.length - shown.length)} more in the list below</p>
          )}
          {!found.length && <p className="muted small person-suggest-note">No one by that name</p>}
        </div>
      )}
    </div>
  );
}
