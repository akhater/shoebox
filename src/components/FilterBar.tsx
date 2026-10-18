import { Aperture, Calendar, Camera, ChevronDown, ChevronRight, FileImage, HardDrive, Tag as TagIcon, Users, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  faceUrl,
  filtersOn,
  fmtDateKey,
  fmtDayOfMonth,
  fmtMonthName,
  fmtNum,
  NO_FILTERS,
  type FacetValue,
  type Facets,
  type Filters,
  type Match,
  type Person,
} from "../api";

/** The filters that tick values (not the any / all switches). */
type ListKey = Exclude<keyof Filters, "peopleMatch" | "tagsMatch">;

interface Props {
  facets: Facets | null;
  filters: Filters;
  onChange: (f: Filters) => void;
  /** Everyone, for the names of people ticked but not in this view. */
  people: Person[];
  /** Before the filters: a switch of the view that narrows it too (a person's "Only new"). */
  lead?: ReactNode;
}

const STATUS_LABEL: Record<string, string> = { online: "Plugged in", offline: "Unplugged", missing: "Missing" };

/** Ticked values first stay listed even when nothing matches them any more. */
function withPicked(values: FacetValue[], picked: string[]): FacetValue[] {
  const missing = picked.filter((p) => !values.some((v) => v.value === p)).map((value) => ({ value, count: 0 }));
  return [...values, ...missing];
}

const toggled = (list: string[], v: string) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);

/** One bar above the grid: People, Date, Camera, Lens, File type, Status. Each opens a list to tick. */
export function FilterBar({ facets, filters, onChange, people, lead }: Props) {
  const [open, setOpen] = useState<ListKey | null>(null);
  const bar = useRef<HTMLDivElement>(null);
  const set = <K extends keyof Filters>(k: K, v: Filters[K]) => onChange({ ...filters, [k]: v });

  // Close on a click elsewhere or Escape (which then doesn't also mean Back).
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!bar.current?.contains(e.target as Node)) setOpen(null);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setOpen(null);
    };
    window.addEventListener("mousedown", away);
    window.addEventListener("keydown", esc, true);
    return () => {
      window.removeEventListener("mousedown", away);
      window.removeEventListener("keydown", esc, true);
    };
  }, [open]);

  const types = facets?.types ?? [];
  const rawExts = types.filter((t) => t.raw).map((t) => t.value);
  const imageExts = types.filter((t) => !t.raw).map((t) => t.value);
  const same = (a: string[], b: string[]) => a.length === b.length && a.every((x) => b.includes(x));

  const personName = (id: number) =>
    facets?.people.find((p) => p.id === id)?.name ?? people.find((p) => p.id === id)?.name ?? "Unnamed person";

  // Tag names seen so far, so a ticked tag keeps its name when nothing matches.
  const tagNames = useRef(new Map<number, string>());
  for (const t of facets?.tags ?? []) tagNames.current.set(t.id, t.name);
  const tagName = (id: number) => tagNames.current.get(id) ?? "Tag";

  // Two names read as "Ana + Bob" (together) or "Ana or Bob"; more as a count.
  const several = (names: string[], m: Match, what: string) =>
    names.length <= 2
      ? names.join(m === "all" ? " + " : " or ")
      : m === "all"
        ? `All of ${names.length} ${what}`
        : `Any of ${names.length} ${what}`;
  const summary = {
    people: (v: number[]) =>
      v.length === 1 ? personName(v[0]) : several(v.map(personName), filters.peopleMatch, "people"),
    tags: (v: number[]) => (v.length === 1 ? tagName(v[0]) : several(v.map(tagName), filters.tagsMatch, "tags")),
    dates: (v: string[]) => (v.length === 1 ? fmtDateKey(v[0]) : `${v.length} dates`),
    cameras: (v: string[]) => (v.length === 1 ? v[0] : `${v.length} cameras`),
    lenses: (v: string[]) => (v.length === 1 ? v[0] : `${v.length} lenses`),
    types: (v: string[]) =>
      rawExts.length && same(v, rawExts)
        ? "RAW"
        : imageExts.length && same(v, imageExts)
          ? "Images"
          : v.length <= 2
            ? v.map((x) => x.toUpperCase()).join(", ")
            : `${v.length} types`,
    status: (v: string[]) => v.map((x) => STATUS_LABEL[x] ?? x).join(", "),
  };

  const chip = (id: ListKey, label: string, icon: ReactNode, panel: ReactNode) => {
    const picked = filters[id];
    const on = picked.length > 0;
    const text =
      id === "people" ? summary.people(filters.people) : id === "tags" ? summary.tags(filters.tags) : summary[id](picked as string[]);
    return (
      <div className="filter-chip-wrap">
        <button className={`filter-chip ${on ? "on" : ""} ${open === id ? "open" : ""}`} onClick={() => setOpen(open === id ? null : id)}>
          {icon}
          <span className="ellipsis">{on ? text : label}</span>
          {on ? (
            <span
              className="chip-x"
              title={`Clear ${label.toLowerCase()}`}
              onClick={(e) => {
                e.stopPropagation();
                set(id, []);
              }}
            >
              <X size={12} />
            </span>
          ) : (
            <ChevronDown size={13} className="muted" />
          )}
        </button>
        {open === id && <div className="filter-panel">{panel}</div>}
      </div>
    );
  };

  return (
    <div className="filterbar" ref={bar}>
      {lead && (
        <>
          {lead}
          <span className="vsep" />
        </>
      )}
      {chip(
        "people",
        "People",
        <Users size={14} />,
        <PeoplePanel
          values={facets?.people ?? []}
          picked={filters.people}
          name={personName}
          onPick={(v) => set("people", v)}
          match={filters.peopleMatch}
          onMatch={(m) => set("peopleMatch", m)}
        />,
      )}
      {chip(
        "tags",
        "Tags",
        <TagIcon size={14} />,
        <TagsPanel
          values={facets?.tags ?? []}
          picked={filters.tags}
          name={tagName}
          onPick={(v) => set("tags", v)}
          match={filters.tagsMatch}
          onMatch={(m) => set("tagsMatch", m)}
        />,
      )}
      {chip("dates", "Date", <Calendar size={14} />, <DatePanel values={facets?.dates ?? []} picked={filters.dates} onPick={(v) => set("dates", v)} />)}
      {chip(
        "cameras",
        "Camera",
        <Camera size={14} />,
        <Checklist values={withPicked(facets?.cameras ?? [], filters.cameras)} picked={filters.cameras} onPick={(v) => set("cameras", v)} />,
      )}
      {chip(
        "lenses",
        "Lens",
        <Aperture size={14} />,
        <Checklist values={withPicked(facets?.lenses ?? [], filters.lenses)} picked={filters.lenses} onPick={(v) => set("lenses", v)} />,
      )}
      {chip(
        "types",
        "File type",
        <FileImage size={14} />,
        <div className="filter-list">
          {[
            ["RAW", types.filter((t) => t.raw)],
            ["Images", types.filter((t) => !t.raw)],
          ].map(([group, list]) => {
            const exts = (list as FacetValue[]).map((t) => t.value);
            if (!exts.length) return null;
            const all = exts.every((x) => filters.types.includes(x));
            const some = !all && exts.some((x) => filters.types.includes(x));
            return (
              <div key={group as string} className="filter-group">
                <label className="filter-row group">
                  <input
                    type="checkbox"
                    checked={all}
                    ref={(el) => {
                      if (el) el.indeterminate = some;
                    }}
                    onChange={() =>
                      set("types", all ? filters.types.filter((x) => !exts.includes(x)) : [...new Set([...filters.types, ...exts])])
                    }
                  />
                  <span className="grow">{group as string}</span>
                  <span className="count">{fmtNum((list as FacetValue[]).reduce((n, t) => n + t.count, 0))}</span>
                </label>
                {(list as FacetValue[]).map((t) => (
                  <label key={t.value} className="filter-row sub">
                    <input type="checkbox" checked={filters.types.includes(t.value)} onChange={() => set("types", toggled(filters.types, t.value))} />
                    <span className="grow">{t.value.toUpperCase()}</span>
                    <span className="count">{fmtNum(t.count)}</span>
                  </label>
                ))}
              </div>
            );
          })}
          {!types.length && <p className="muted small">No photos here.</p>}
        </div>,
      )}
      {chip(
        "status",
        "Status",
        <HardDrive size={14} />,
        <Checklist
          values={withPicked(facets?.status ?? [], filters.status)}
          picked={filters.status}
          label={(v) => STATUS_LABEL[v] ?? v}
          onPick={(v) => set("status", v as Filters["status"])}
        />,
      )}
      {filtersOn(filters) && (
        <button className="btn small ghost" onClick={() => onChange(NO_FILTERS)}>
          Clear filters
        </button>
      )}
    </div>
  );
}

/** Tick values; a search box when the list is long. */
function Checklist({
  values,
  picked,
  onPick,
  label = (v) => v,
}: {
  values: FacetValue[];
  picked: string[];
  onPick: (v: string[]) => void;
  label?: (v: string) => string;
}) {
  const [q, setQ] = useState("");
  const shown = q ? values.filter((v) => label(v.value).toLowerCase().includes(q.toLowerCase())) : values;
  return (
    <div className="filter-list">
      {values.length > 8 && (
        <input className="filter-search" autoFocus placeholder="Find…" value={q} onChange={(e) => setQ(e.target.value)} />
      )}
      {shown.map((v) => (
        <label key={v.value} className="filter-row">
          <input type="checkbox" checked={picked.includes(v.value)} onChange={() => onPick(toggled(picked, v.value))} />
          <span className="grow ellipsis" title={label(v.value)}>
            {label(v.value)}
          </span>
          <span className="count">{fmtNum(v.count)}</span>
        </label>
      ))}
      {!values.length && <p className="muted small">No photos here.</p>}
    </div>
  );
}

/**
 * Any or all of the ticked values. With "all", counts follow the ticks: only what is in a
 * photo with them is left to tick.
 */
function MatchSwitch({ value, onChange, all }: { value: Match; onChange: (m: Match) => void; all: string }) {
  return (
    <div className="match-switch" role="radiogroup" aria-label="Photos with">
      {(["any", "all"] as const).map((m) => (
        <button key={m} role="radio" aria-checked={value === m} className={value === m ? "on" : ""} onClick={() => onChange(m)}>
          {m === "any" ? "Any of them" : all}
        </button>
      ))}
    </div>
  );
}

/** Tags. Photos with every ticked tag show (or any, on the switch). */
function TagsPanel({
  values,
  picked,
  name,
  onPick,
  match,
  onMatch,
}: {
  values: Facets["tags"];
  picked: number[];
  name: (id: number) => string;
  onPick: (v: number[]) => void;
  match: Match;
  onMatch: (m: Match) => void;
}) {
  const [q, setQ] = useState("");
  const all = [...values, ...picked.filter((id) => !values.some((v) => v.id === id)).map((id) => ({ id, name: name(id), count: 0 }))];
  const shown = q ? all.filter((t) => t.name.toLowerCase().includes(q.toLowerCase())) : all;
  return (
    <div className="filter-list">
      {(all.length > 1 || picked.length > 0) && <MatchSwitch value={match} onChange={onMatch} all="All of them" />}
      {all.length > 8 && <input className="filter-search" autoFocus placeholder="Find a tag…" value={q} onChange={(e) => setQ(e.target.value)} />}
      {shown.map((t) => (
        <label key={t.id} className="filter-row">
          <input
            type="checkbox"
            checked={picked.includes(t.id)}
            onChange={() => onPick(picked.includes(t.id) ? picked.filter((x) => x !== t.id) : [...picked, t.id])}
          />
          <span className="grow ellipsis">{t.name}</span>
          <span className="count">{fmtNum(t.count)}</span>
        </label>
      ))}
      {!all.length && <p className="muted small">No tags on these photos yet. Select photos and press T to tag them.</p>}
    </div>
  );
}

/** People, with their face. Any ticked person's photos show (or only those with all of them together). */
function PeoplePanel({
  values,
  picked,
  name,
  onPick,
  match,
  onMatch,
}: {
  values: Facets["people"];
  picked: number[];
  name: (id: number) => string;
  onPick: (v: number[]) => void;
  match: Match;
  onMatch: (m: Match) => void;
}) {
  const [q, setQ] = useState("");
  // Ticked people stay listed even when this view has none of them.
  const all = [...values, ...picked.filter((id) => !values.some((v) => v.id === id)).map((id) => ({ id, name: name(id), cover: 0, count: 0 }))];
  const shown = q ? all.filter((p) => (p.name ?? "").toLowerCase().includes(q.toLowerCase())) : all;
  const flip = (id: number) => onPick(picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id]);
  return (
    <div className="filter-list people-list">
      {(all.length > 1 || picked.length > 0) && <MatchSwitch value={match} onChange={onMatch} all="All together" />}
      {all.length > 8 &&<input className="filter-search" autoFocus placeholder="Find a name…" value={q} onChange={(e) => setQ(e.target.value)} />}
      {shown.map((p) => (
        <label key={p.id} className="filter-row">
          <input type="checkbox" checked={picked.includes(p.id)} onChange={() => flip(p.id)} />
          {p.cover ? <img className="filter-face" src={faceUrl(p.cover)} alt="" loading="lazy" /> : <span className="filter-face" />}
          <span className={`grow ellipsis ${p.name ? "" : "muted"}`}>{p.name ?? "Unnamed"}</span>
          <span className="count">{fmtNum(p.count)}</span>
        </label>
      ))}
      {!all.length && <p className="muted small">Nobody found in these photos yet.</p>}
    </div>
  );
}

interface DateNode {
  key: string;
  label: string;
  count: number;
  children: DateNode[];
}

/** Days rolled up into months and years. */
function dateTree(values: FacetValue[]): DateNode[] {
  const years: DateNode[] = [];
  for (const v of values) {
    if (v.value === "unknown") continue;
    const [y, m] = [v.value.slice(0, 4), v.value.slice(0, 7)];
    let yn = years.find((n) => n.key === y);
    if (!yn) years.push((yn = { key: y, label: y, count: 0, children: [] }));
    let mn = yn.children.find((n) => n.key === m);
    if (!mn) yn.children.push((mn = { key: m, label: fmtMonthName(+m.slice(5)), count: 0, children: [] }));
    mn.children.push({ key: v.value, label: fmtDayOfMonth(v.value), count: v.count, children: [] });
    mn.count += v.count;
    yn.count += v.count;
  }
  return years;
}

/** `k` is `key` or contains it ("2026" contains "2026-09-26"). */
const covers = (k: string, key: string) => key === k || key.startsWith(`${k}-`);

/** Years, months, days. Ticking one takes all of it; unticking a day inside a ticked month keeps the rest of the month. */
function DatePanel({ values, picked, onPick }: { values: FacetValue[]; picked: string[]; onPick: (v: string[]) => void }) {
  const tree = useMemo(() => dateTree(values), [values]);
  const unknown = values.find((v) => v.value === "unknown");
  const [openKeys, setOpenKeys] = useState<string[]>(() => (tree.length <= 2 ? tree.map((n) => n.key) : []));

  const covered = (key: string) => picked.some((k) => covers(k, key));
  const partial = (key: string) => !covered(key) && picked.some((k) => k.startsWith(`${key}-`));
  const toggle = (path: DateNode[]) => {
    const key = path[path.length - 1].key;
    const above = picked.find((k) => k !== key && covers(k, key));
    if (above) {
      // Split what's ticked above: keep everything under it except this.
      const next = picked.filter((k) => k !== above);
      for (let i = path.findIndex((n) => n.key === above); i >= 0 && i < path.length - 1; i++) {
        for (const c of path[i].children) if (c.key !== path[i + 1].key) next.push(c.key);
      }
      onPick(next);
    } else if (covered(key)) {
      onPick(picked.filter((k) => !covers(key, k)));
    } else {
      onPick([...picked.filter((k) => !covers(key, k)), key]);
    }
  };

  const row = (path: DateNode[]): ReactNode => {
    const n = path[path.length - 1];
    const depth = path.length - 1;
    const isOpen = openKeys.includes(n.key);
    return (
      <div key={n.key}>
        <div className={`filter-row ${depth === 0 ? "group" : ""}`} style={{ paddingLeft: 8 + depth * 18 }}>
          <input
            type="checkbox"
            checked={covered(n.key)}
            ref={(el) => {
              if (el) el.indeterminate = partial(n.key);
            }}
            onChange={() => toggle(path)}
          />
          {n.children.length ? (
            <button className="twisty-btn" onClick={() => setOpenKeys(toggled(openKeys, n.key))} title={isOpen ? "Close" : "Open"}>
              <ChevronRight size={12} className={`chev ${isOpen ? "open" : ""}`} />
            </button>
          ) : (
            <span className="twisty-gap" />
          )}
          <span className="grow" onClick={() => toggle(path)}>
            {n.label}
          </span>
          <span className="count">{fmtNum(n.count)}</span>
        </div>
        {isOpen && n.children.map((c) => row([...path, c]))}
      </div>
    );
  };

  return (
    <div className="filter-list">
      {tree.map((y) => row([y]))}
      {unknown && (
        <label className="filter-row">
          <input type="checkbox" checked={picked.includes("unknown")} onChange={() => onPick(toggled(picked, "unknown"))} />
          <span className="grow muted">Unknown date</span>
          <span className="count">{fmtNum(unknown.count)}</span>
        </label>
      )}
      {!values.length && <p className="muted small">No photos here.</p>}
    </div>
  );
}
