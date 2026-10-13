import { Plus, Tag as TagIcon, X } from "lucide-react";
import { forwardRef, useEffect, useState, type Ref } from "react";
import { fmtNum, tagsApi, type Tag } from "../api";

/** Type a tag: suggestions from the tags that exist, Enter adds, commas add several. */
export const TagInput = forwardRef<HTMLInputElement, { all: Tag[]; onAdd: (names: string[]) => void; autoFocus?: boolean }>(
  function TagInput({ all, onAdd, autoFocus }, ref) {
    const [text, setText] = useState("");
    const [hi, setHi] = useState(-1);
    const q = text.trim().toLowerCase();
    const suggestions = q ? all.filter((t) => t.name.toLowerCase().includes(q)).slice(0, 8) : [];
    const exact = all.some((t) => t.name.toLowerCase() === q);

    const submit = (names: string[]) => {
      const clean = names.map((n) => n.trim()).filter(Boolean);
      if (clean.length) onAdd(clean);
      setText("");
      setHi(-1);
    };

    return (
      <div className="tag-input">
        <TagIcon size={13} className="muted" />
        <input
          ref={ref}
          autoFocus={autoFocus}
          value={text}
          placeholder="Add a tag…"
          spellCheck={false}
          onChange={(e) => {
            setText(e.target.value);
            setHi(-1);
          }}
          onKeyDown={(e) => {
            // Typing here never drives the viewer or the grid (arrows, Delete, Z...).
            e.stopPropagation();
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setHi(Math.min(suggestions.length - 1, hi + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setHi(Math.max(-1, hi - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              submit(hi >= 0 && suggestions[hi] ? [suggestions[hi].name] : text.split(","));
            } else if (e.key === "Tab" && suggestions.length && q) {
              e.preventDefault();
              setText(suggestions[Math.max(0, hi)].name);
            } else if (e.key === "Escape") {
              if (text) setText("");
              else e.currentTarget.blur();
            }
          }}
        />
        {q && (
          <div className="tag-suggest">
            {suggestions.map((t, i) => (
              <button
                key={t.id}
                className={i === hi ? "hi" : ""}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => submit([t.name])}
              >
                <span className="grow">{t.name}</span>
                <span className="count">{fmtNum(t.count)}</span>
              </button>
            ))}
            {!exact && (
              <button className={hi === -1 ? "hi" : ""} onMouseDown={(e) => e.preventDefault()} onClick={() => submit(text.split(","))}>
                <Plus size={13} /> <span className="grow">Add “{text.trim()}”</span>
              </button>
            )}
          </div>
        )}
      </div>
    );
  },
);

/** Tags on one or more photos: chips with × to take off; a box to add. With
 *  several photos, a tag only some have shows "2 of 5"; clicking it puts it on all. */
export function TagEditor({
  photoIds,
  onChanged,
  autoFocus,
  inputRef,
}: {
  photoIds: number[];
  onChanged?: () => void;
  autoFocus?: boolean;
  inputRef?: Ref<HTMLInputElement>;
}) {
  const [on, setOn] = useState<Tag[]>([]);
  const [all, setAll] = useState<Tag[]>([]);
  const [error, setError] = useState<string | null>(null);
  const key = photoIds.join(",");

  const load = () => {
    tagsApi.on(photoIds).then(setOn).catch((e) => setError(String(e)));
    tagsApi.all().then(setAll).catch(() => {});
  };
  useEffect(load, [key]);

  const act = (p: Promise<unknown>) =>
    p
      .then(() => {
        setError(null);
        load();
        onChanged?.();
      })
      .catch((e) => setError(String(e)));

  const n = photoIds.length;
  return (
    <div className="tag-editor">
      {on.length > 0 && (
        <div className="tag-chips">
          {on.map((t) => {
            const some = n > 1 && t.count < n;
            return (
              <span key={t.id} className={`tag-chip ${some ? "some" : ""}`}>
                <button
                  className="tag-name"
                  disabled={!some}
                  onClick={() => act(tagsApi.add(photoIds, [t.name]))}
                  title={some ? `On ${t.count} of ${n} - click to put it on all ${n}` : undefined}
                >
                  {t.name}
                  {some && <span className="muted"> {t.count} of {n}</span>}
                </button>
                <button
                  className="tag-x"
                  onClick={() => act(tagsApi.remove(photoIds, [t.id]))}
                  title={n > 1 ? `Take “${t.name}” off all ${n}` : `Take “${t.name}” off`}
                >
                  <X size={11} />
                </button>
              </span>
            );
          })}
        </div>
      )}
      <TagInput ref={inputRef} all={all} autoFocus={autoFocus} onAdd={(names) => act(tagsApi.add(photoIds, names))} />
      {error && <p className="note warn">{error}</p>}
    </div>
  );
}
