import {
  ArchiveX,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  FolderInput,
  FolderOpen,
  ImageOff,
  Trash2,
  Undo2,
  UserX,
  X,
} from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
} from "react";
import {
  api,
  BIN,
  BLUR_PROBLEMS,
  faceUrl,
  fmtBytes,
  fmtDay,
  fmtDeleted,
  fmtNum,
  fullUrl,
  people,
  previewUrl,
  thumbUrl,
  type FileLoc,
  type Focus,
  type MissingPhoto,
  type PhotoDetail,
  type Person,
  type PhotoFace,
  type PhotoList,
} from "../api";
import { useRemembered } from "../remembered";
import { FocusIcon, FocusPoints, FocusSummary, hasFocusPoints } from "./FocusPoints";
import { TagEditor } from "./Tags";
import { TurnControl, turnedStyle, usePreviewHandlers } from "./TurnControl";
import { WhoIsIt } from "./WhoIsIt";

/** Where in the photo a zoom is centred, 0..1 of its width and height. */
interface Zoom {
  fx: number;
  fy: number;
}

interface Props {
  list: PhotoList;
  index: number;
  onIndex: (i: number) => void;
  /** `picked`: the photos selected in the filmstrip, to keep selected in the grid. */
  onClose: (picked?: number[]) => void;
  /** Marks the photo for deletion, or unmarks it. */
  onToggleMark: () => void;
  /** A missing photo: takes it out of the catalog (asks first). */
  onForget: () => void;
  /** Show a person's photos. */
  onPerson: (personId: number) => void;
  /** Show the folder a copy of the photo is in. */
  onFolder: (copy: FileLoc) => void;
  /** Tags were added or taken off (filter counts follow). */
  onTagsChanged: () => void;
  /** Everyone in People, to say who a face is. */
  everyone: Person[];
  /** A face went to someone else. */
  onFacesChanged: () => void;
  /** Turns photos (those selected in the filmstrip, the one shown among them):
   *  quarter turns clockwise (-1 counter-clockwise), back to how their files show
   *  them, or the way the upright model suggests. Shoebox's copies only, never a file. */
  onTurn: (ids: number[], by: number | "back" | "suggested") => Promise<boolean>;
  /** In Sideways: each photo's suggested turn (quarter turns clockwise), to turn or leave. */
  suggested?: Map<number, number> | null;
  /** Sideways: photos shown as they would be turned (else as they are), as in the grid, and its switch. */
  showTurned?: boolean;
  onShowTurned?: (on: boolean) => void;
  /** Sideways: these photos are fine as they are. */
  onLeave?: (ids: number[]) => void;
}

/** The button that makes a turn: "Turn it left", "Turn it upside down"... */
export const turnItWords = (turn: number) =>
  ["Leave it", "Turn it right", "Turn it upside down", "Turn it left"][((turn % 4) + 4) % 4];

/** "a quarter turn right", "upside down"... for quarter turns clockwise. */
export const turnWords = (turn: number) =>
  ["", "a quarter turn right", "upside down", "a quarter turn left"][((turn % 4) + 4) % 4];

const STATE_LABEL = {
  online: "Plugged in",
  offline: "Unplugged",
  unreachable: "Folder not found",
  missing: "Missing",
} as const;

export function Viewer({
  list,
  index,
  onIndex,
  onClose,
  onToggleMark,
  onForget,
  onPerson,
  onFolder,
  onTagsChanged,
  everyone,
  onFacesChanged,
  onTurn,
  suggested,
  showTurned = true,
  onShowTurned,
  onLeave,
}: Props) {
  const tagBox = useRef<HTMLInputElement>(null);
  const id = list.ids[index];
  const rev = list.rev[index];
  const marked = list.marked[index] === 1;
  const missing = list.state[index] === 0;
  // A missing photo: is its file in the system's bin (deleted, not moved)?
  const [gone, setGone] = useState<{ id: number; photo: MissingPhoto | null } | null>(null);
  useEffect(() => {
    if (!missing) return;
    let left = false;
    api
      .missingPhotos([id])
      .then((r) => !left && setGone({ id, photo: r.photos[0] ?? null }))
      .catch(() => {});
    return () => {
      left = true;
    };
  }, [id, missing]);
  const binned = gone?.id === id && gone.photo?.inBin ? gone.photo : null;
  const [detail, setDetail] = useState<PhotoDetail | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [faces, setFaces] = useState<PhotoFace[]>([]);
  // The face being said who it is ("Who is this?").
  const [who, setWho] = useState<PhotoFace | null>(null);
  // The face whose name is pointed at in People: boxed on the photo.
  const [spot, setSpot] = useState<number | null>(null);
  // Plugged in: a sharp picture from the original, sized to the screen, swaps in
  // over the preview; a click zooms to 100% from the full-size original.
  const reachable = list.state[index] === 2;
  const stage = useRef<HTMLDivElement>(null);
  const [previewLong, setPreviewLong] = useState(0);
  const [previewSize, setPreviewSize] = useState<{ w: number; h: number } | null>(null);
  const [sharp, setSharp] = useState<string | null>(null);
  const [zoom, setZoom] = useState<Zoom | null>(null);
  const [full, setFull] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  // Pointing at a turn shows the photo turned that way, before the click; after
  // it, the photo stays so until its turned picture is in (`waitingFor`: the
  // revision it had then). `instant`: no animation (the new picture replacing it).
  const [turnView, setTurnView] = useState<{ q: number; transform: string; instant?: boolean } | null>(null);
  const waitingFor = useRef<number | null>(null);
  // Where the camera focused, over the photo: off until asked for, then
  // remembered for the next photos and the next time.
  const [showFocus, setShowFocus] = useRemembered("shoebox.focusPoints", false);

  // In Sideways the photo is shown as Shoebox would turn it (or as it is), as in the grid.
  const proposal = suggested?.get(id) ?? 0;
  const proposing = proposal !== 0 && showTurned;

  // Photos selected in the filmstrip (Shift+click a range, Ctrl+click one,
  // Shift+arrows): turning acts on all of them. The one shown is always among them.
  const [picked, setPicked] = useState<Set<number>>(() => new Set([list.ids[index]]));
  const anchor = useRef(index);
  const selection = useMemo(() => list.ids.filter((x) => x === id || picked.has(x)), [list.ids, picked, id]);
  const many = selection.length > 1;
  /** Goes to photo `i`: alone, extending a range from the anchor, or adding or removing it. */
  const go = (i: number, how: "only" | "range" | "toggle" = "only") => {
    if (i < 0 || i >= list.ids.length) return;
    const to = list.ids[i];
    if (how === "range") {
      const [a, b] = [Math.min(anchor.current, i), Math.max(anchor.current, i)];
      setPicked(new Set(list.ids.slice(a, b + 1)));
      onIndex(i);
    } else if (how === "toggle") {
      if (!selection.includes(to)) {
        setPicked(new Set([...selection, to]));
        anchor.current = i;
        onIndex(i);
      } else if (many) {
        const rest = selection.filter((x) => x !== to);
        setPicked(new Set(rest));
        // The one shown left the selection: show the nearest one still in it.
        if (to === id) {
          const near = rest.map((x) => list.ids.indexOf(x)).sort((a, b) => Math.abs(a - i) - Math.abs(b - i))[0];
          onIndex(near);
        }
      }
    } else {
      setPicked(new Set([to]));
      anchor.current = i;
      onIndex(i);
    }
  };
  const close = () => onClose(selection);
  // Pointing at a turn in a normal view: the selected photos in the filmstrip are drawn turned too.
  const [hoverQ, setHoverQ] = useState(0);

  /** The photo as it would stand after `q` quarter turns clockwise, fitted to the stage. */
  const turnedLook = (q: number) => {
    const el = stage.current;
    // Its preview's shape; until that is in, its thumbnail's (the same).
    const size = previewSize ?? (list.w[index] && list.h[index] ? { w: list.w[index], h: list.h[index] } : null);
    if (!el || !size || q % 4 === 0) return null;
    let k = 1;
    if (Math.abs(q) % 2 === 1) {
      const [bw, bh] = [el.clientWidth - 48, el.clientHeight - 48];
      const s0 = Math.min(bw / size.w, bh / size.h);
      k = Math.min(bw / (size.h * s0), bh / (size.w * s0));
    }
    return { q, transform: `rotate(${q * 90}deg) scale(${k.toFixed(4)})` };
  };
  /** How the photo is shown when no turn is pointed at: as it is, or (Sideways) as suggested. */
  const restingLook = () => (proposing ? { ...turnedLook(proposal)!, instant: true } : null);

  // Another photo: shown as it is, or as suggested - before it is drawn, never
  // the other way first. The photo being turned stays as it is shown until its
  // turned picture is in.
  const shownFor = useRef<number | null>(null);
  useLayoutEffect(() => {
    if (shownFor.current === id && waitingFor.current != null) return;
    shownFor.current = id;
    waitingFor.current = null;
    setTurnView(restingLook());
    setHoverQ(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, proposal, showTurned]);

  /** Pointing at a turn (0: pointing away). */
  const previewTurn = (q: number) => {
    if (waitingFor.current != null) return;
    setTurnView(q ? turnedLook(q) : restingLook());
    if (!suggested) setHoverQ(q);
  };
  /** How many quarter turns a turn is. */
  const quarters = (by: number | "back" | "suggested") =>
    by === "back" ? -(detail?.turn ?? 0) : by === "suggested" ? (detail?.sideways ?? 0) : by;
  const turnNow = (by: number | "back" | "suggested") => {
    if (zoom) setZoom(null);
    waitingFor.current = rev;
    setTurnView(turnedLook(quarters(by)));
    setHoverQ(0);
    onTurn(selection, by).then((ok) => {
      if (ok) return;
      waitingFor.current = null;
      setTurnView(restingLook());
    });
  };
  /** Hover and focus handlers that show a turn before it is made. */
  const showing = usePreviewHandlers(previewTurn);

  // Another photo, or this one turned (a new revision): its pictures again.
  useEffect(() => {
    setSharp(null);
    setZoom(null);
    setFull(null);
    setPreviewLong(0);
    setPreviewSize(null);
    setSpot(null);
  }, [id, rev]);

  // Once the preview shows, fetch the sharp one if the screen deserves more.
  useEffect(() => {
    const el = stage.current;
    if (!reachable || !previewLong || !el) return;
    const want = Math.round(Math.max(el.clientWidth, el.clientHeight) * window.devicePixelRatio);
    if (want <= previewLong * 1.1) return;
    let live = true;
    const url = fullUrl(id, Math.min(want, 4096), rev);
    const img = new Image();
    // A short wait, so arrowing through a shoot doesn't read every original.
    const t = window.setTimeout(() => {
      img.onload = () => live && setSharp(url);
      img.src = url;
    }, 250);
    return () => {
      live = false;
      window.clearTimeout(t);
      img.onload = null;
    };
  }, [id, rev, reachable, previewLong]);

  // Zoomed in: the full-size original.
  useEffect(() => {
    if (!zoom || !reachable || full) return;
    let live = true;
    const url = fullUrl(id, 0, rev);
    const img = new Image();
    img.onload = () => live && setFull(url);
    img.src = url;
    return () => {
      live = false;
      img.onload = null;
    };
  }, [zoom, reachable, full, id, rev]);

  useEffect(() => {
    if (!hint) return;
    const t = window.setTimeout(() => setHint(null), 3500);
    return () => window.clearTimeout(t);
  }, [hint]);

  const zoomAt = (z: Zoom) => {
    if (!reachable) {
      setHint(`Shoebox's own copy is ${previewLong || "1080"} px. Plug the disk in to zoom into the original.`);
      return;
    }
    setZoom(z);
  };

  /** Click on the fitted photo: zoom in where it was clicked. */
  const onPhotoClick = (e: ReactMouseEvent<HTMLImageElement>) => {
    e.stopPropagation();
    // Shown as it would be turned (Sideways): 100% is of the photo as it is.
    if (proposing) return;
    const img = e.currentTarget;
    const r = img.getBoundingClientRect();
    const s = Math.min(r.width / img.naturalWidth, r.height / img.naturalHeight);
    const [dw, dh] = [img.naturalWidth * s, img.naturalHeight * s];
    const clamp = (v: number) => Math.min(1, Math.max(0, v));
    zoomAt({ fx: clamp((e.clientX - r.left - (r.width - dw) / 2) / dw), fy: clamp((e.clientY - r.top - (r.height - dh) / 2) / dh) });
  };

  // A turned photo's faces are new ones, their boxes turned with it.
  useEffect(() => {
    let live = true;
    setFaces([]);
    people
      .inPhoto(id)
      .then((f) => live && setFaces(f))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [id, rev]);

  useEffect(() => {
    let live = true;
    setLoaded(false);
    setError(null);
    api
      .photo(id)
      .then((d) => live && setDetail(d))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [id, rev]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (who) return; // "Who is this?" is open
      if (e.key === "Escape") {
        if (zoom) setZoom(null);
        else if (many) setPicked(new Set([id])); // first the selection, then the viewer
        else close();
      } else if ((e.key === "v" || e.key === "V") && suggested && onShowTurned) {
        onShowTurned(!showTurned);
      } else if (e.key === "z" || e.key === "Z") {
        if (zoom) setZoom(null);
        else zoomAt(focusCentre ?? { fx: 0.5, fy: 0.5 });
      } else if (e.key === "t" || e.key === "T") {
        e.preventDefault(); // don't type the "t"
        tagBox.current?.focus();
      } else if ((e.key === "r" || e.key === "R") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        turnNow(e.shiftKey ? -1 : 1);
      } else if ((e.key === "f" || e.key === "F") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        toggleFocus();
      } else if (e.key === "ArrowLeft") go(index - 1, e.shiftKey ? "range" : "only");
      else if (e.key === "ArrowRight") go(index + 1, e.shiftKey ? "range" : "only");
      else if (e.key === "Delete") {
        // Culling: mark and move on; Delete on a marked photo unmarks it and stays.
        onToggleMark();
        if (!marked) go(index + 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const act = (p: Promise<void>) => p.catch((e) => setError(String(e)));
  const spotted = faces.find((f) => f.id === spot) ?? null;
  const d = detail?.id === id ? detail : null;
  // Only over the picture they were read for (a turn makes a new one).
  const focus = d && d.previewRev === rev ? d.focus : null;
  const pointsHere = hasFocusPoints(focus);
  const drawFocus = showFocus && loaded && pointsHere && focus ? focus : null;
  const noFocusWords = d?.focus?.mode === "Manual focus" ? "Manual focus" : "Focus not recorded";
  // With the points shown, Z zooms where the camera focused (where sharpness is checked first).
  const focusPoint = drawFocus?.points.find((p) => p.state === "inFocus") ?? drawFocus?.points.find((p) => p.state === "chosen");
  const focusCentre = focusPoint && !proposing ? { fx: focusPoint.x + focusPoint.w / 2, fy: focusPoint.y + focusPoint.h / 2 } : null;
  const toggleFocus = () => {
    const on = !showFocus;
    setShowFocus(on);
    // Nothing changes on this photo: say what happened.
    if (d && !pointsHere) {
      setHint(
        on
          ? d.focus?.mode === "Manual focus"
            ? "Focus points on. This photo was focused by hand: the camera recorded no point."
            : "Focus points on. This photo's file doesn't say where the camera focused."
          : "Focus points off.",
      );
    }
  };
  const exposure = d
    ? [
        d.fnumber && `f/${+d.fnumber.toFixed(1)}`,
        d.exposure,
        d.iso && `ISO ${d.iso}`,
        d.focal && `${Math.round(d.focal)} mm`,
      ].filter(Boolean)
    : [];
  // What Cull measured: a camera that confirmed focus says nothing of shake or motion.
  const blur = d?.problems.filter((f) => BLUR_PROBLEMS.includes(f.problem)) ?? [];

  return (
    <div className="viewer" role="dialog" aria-label="Photo">
      <div className="viewer-main">
      <div className="viewer-stage" ref={stage} onClick={close}>
        {list.preview[index] === 1 && zoom ? (
          <ZoomedPhoto
            src={full ?? sharp ?? previewUrl(id, rev)}
            fullLong={d ? Math.max(d.width ?? 0, d.height ?? 0) : 0}
            zoom={zoom}
            sharp={!!full}
            spot={spotted}
            focus={drawFocus}
            onExit={() => setZoom(null)}
          />
        ) : list.preview[index] === 1 ? (
          <div className={`viewer-turn ${turnView?.instant ? "instant" : ""}`} style={turnView ? { transform: turnView.transform } : undefined}>
            <img
              className="viewer-under"
              src={thumbUrl(id, rev)}
              alt=""
              style={{ opacity: loaded || waitingFor.current != null ? 0 : 1 }}
            />
            <img
              className="viewer-img"
              src={previewUrl(id, rev)}
              alt=""
              onLoad={(e) => {
                const { naturalWidth: w, naturalHeight: h } = e.currentTarget;
                setLoaded(true);
                setPreviewLong(Math.max(w, h));
                setPreviewSize({ w, h });
                // The turned picture is in: it replaces the one shown turned.
                if (waitingFor.current != null && waitingFor.current !== rev) {
                  waitingFor.current = null;
                  setTurnView({ q: 0, transform: "none", instant: true });
                }
              }}
              onClick={onPhotoClick}
              title={reachable ? "Click to zoom to 100% (Z)" : undefined}
            />
            {sharp && <img className="viewer-img sharp" src={sharp} alt="" onClick={onPhotoClick} title="Click to zoom to 100% (Z)" />}
            {drawFocus && previewSize && (
              <FittedFrame stage={stage} size={previewSize}>
                <FocusPoints focus={drawFocus} faded={!!spotted} />
              </FittedFrame>
            )}
            {spotted && previewSize && (
              <FittedFrame stage={stage} size={previewSize}>
                <FaceSpot face={spotted} />
              </FittedFrame>
            )}
          </div>
        ) : (
          <div className="viewer-empty">
            <ImageOff size={40} />
            <span>{list.preview[index] === 2 ? "Shoebox couldn't read this photo" : "Not read yet"}</span>
          </div>
        )}
        {marked && (
          <span className="viewer-marked">
            <Trash2 size={14} /> Marked for deletion
          </span>
        )}
        {index > 0 && (
          <button className="nav prev" onClick={(e) => (e.stopPropagation(), go(index - 1))} aria-label="Previous">
            <ChevronLeft size={28} />
          </button>
        )}
        {index < list.ids.length - 1 && (
          <button className="nav next" onClick={(e) => (e.stopPropagation(), go(index + 1))} aria-label="Next">
            <ChevronRight size={28} />
          </button>
        )}
        {list.preview[index] === 1 && !zoom && (
          proposal ? (
            <div className="turn-decision" onClick={(e) => e.stopPropagation()}>
              <button className="btn small primary" onClick={() => turnNow("suggested")}>
                {many ? `Turn ${fmtNum(selection.length)} as Shoebox suggests` : turnItWords(proposal)}
              </button>
              <button
                className="btn small"
                onClick={() => onLeave?.(selection)}
                title={many ? "They stand right as they are: never suggested again" : "It stands right as it is: never suggested again"}
              >
                {many ? `Leave ${fmtNum(selection.length)} as they are` : "Leave it as it is"}
              </button>
            </div>
          ) : (
            <TurnControl
              on="photo"
              what={many ? `the ${fmtNum(selection.length)} selected photos` : "this photo"}
              onTurn={turnNow}
              onPreview={previewTurn}
            />
          )
        )}
        {list.preview[index] === 1 && (
          <button
            className={`focus-toggle ${showFocus && d && !pointsHere ? "unrecorded" : ""}`}
            aria-label="Focus points"
            aria-pressed={showFocus}
            onClick={(e) => {
              e.stopPropagation();
              toggleFocus();
            }}
            title={
              !showFocus
                ? "Show where the camera focused (F)"
                : d && !pointsHere
                  ? d.focus?.mode === "Manual focus"
                    ? "Focused by hand: the camera recorded no point. Click to stop showing focus points (F)"
                    : "The file doesn't say where the camera focused. Click to stop showing focus points (F)"
                  : "Hide the focus points (F)"
            }
          >
            <FocusIcon on={showFocus} />
            {showFocus && d && !pointsHere ? noFocusWords : "Focus points"}
          </button>
        )}
        {proposal !== 0 && onShowTurned && !zoom && (
          <div className="view-switch" role="group" aria-label="Show" onClick={(e) => e.stopPropagation()}>
            <button className={showTurned ? "" : "on"} onClick={() => onShowTurned(false)} title="Show it as it is now (V)">
              As it is
            </button>
            <button
              className={showTurned ? "on" : ""}
              onClick={() => onShowTurned(true)}
              title="Show it as Shoebox would turn it; nothing is turned until you say so (V)"
            >
              As Shoebox would turn it
            </button>
          </div>
        )}
        {turnView && turnView.q !== 0 && waitingFor.current == null && !(proposing && turnView.q === proposal) && (
          <span className="viewer-preview" style={proposal !== 0 ? { top: 56 } : undefined}>
            Preview: {turnWords(turnView.q)}. Click to turn it; nothing is turned yet.
          </span>
        )}
        {zoom && <span className="viewer-badge">{full ? "100%" : "Loading full size…"}</span>}
        {hint && <span className="viewer-hint">{hint}</span>}
      </div>
      <Filmstrip
        list={list}
        index={index}
        picked={selection}
        onPick={(i, e) => go(i, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "only")}
        turnOf={(x) => (showTurned ? (suggested?.get(x) ?? 0) : 0) + (hoverQ && selection.includes(x) ? hoverQ : 0)}
      />
      </div>

      <aside className="viewer-info">
        <div className="viewer-top">
          <span className="muted">
            {index + 1} of {list.ids.length}
            {many && ` · ${fmtNum(selection.length)} selected`}
          </span>
          <span className="viewer-top-actions">
            <button
              className={`btn small ${marked ? "" : "ghost"}`}
              onClick={onToggleMark}
              title={marked ? "Keep this photo (Delete)" : "Mark for deletion (Delete). Nothing is deleted until you confirm."}
            >
              {marked ? <Undo2 size={14} /> : <Trash2 size={14} />} {marked ? "Unmark" : "Mark for deletion"}
            </button>
            <button className="icon-btn" onClick={close} aria-label="Close" title="Close (Esc)">
              <X size={18} />
            </button>
          </span>
        </div>
        {d && (
          <>
            <h2>{d.files[0]?.name ?? "Photo"}</h2>
            <dl>
              <dt>Taken</dt>
              <dd>
                {d.takenTs ? fmtDay(d.takenTs) : "Unknown"}
                {!d.takenFromCamera && d.takenTs ? <span className="muted"> (file date)</span> : null}
              </dd>
              {d.camera && (
                <>
                  <dt>Camera</dt>
                  <dd>{d.camera}</dd>
                </>
              )}
              {d.lens && (
                <>
                  <dt>Lens</dt>
                  <dd title={d.lensInFile && d.lensInFile !== d.lens ? `In the file: ${d.lensInFile}` : undefined}>
                    {d.lens}
                  </dd>
                </>
              )}
              {exposure.length > 0 && (
                <>
                  <dt>Exposure</dt>
                  <dd>{exposure.join("  ·  ")}</dd>
                </>
              )}
              {d.focus && (
                <>
                  <dt>Focus</dt>
                  <dd>
                    <FocusSummary focus={d.focus} legend={!!drawFocus} />
                  </dd>
                </>
              )}
              {blur.length > 0 && (
                <>
                  <dt>Sharpness</dt>
                  <dd className="warn-text">
                    {blur.map((f) => (
                      <div key={f.problem}>{f.text}</div>
                    ))}
                  </dd>
                </>
              )}
              {d.width && d.height && (
                <>
                  <dt>Resolution</dt>
                  <dd>
                    {d.width} × {d.height} <span className="muted">({Math.round((d.width * d.height) / 1e6)} MP)</span>
                  </dd>
                </>
              )}
              {d.files[0] && (
                <>
                  <dt>Size</dt>
                  <dd>
                    {fmtBytes(d.files[0].size)} · {d.files[0].name.split(".").pop()?.toUpperCase()}
                    {d.kind === "raw" ? " RAW" : ""}
                  </dd>
                </>
              )}
            </dl>
            {d.turn !== 0 && (
              <p className="note turn-note">
                Turned {turnWords(d.turn)} in Shoebox; the file is unchanged.{" "}
                <button className="link" onClick={() => turnNow("back")} {...showing(-d.turn)}>
                  Show it as the file does
                </button>
              </p>
            )}
            {d.sideways > 0 && (
              <p className="note turn-note">
                It looks sideways: Shoebox would turn it {turnWords(d.sideways)}.{" "}
                <button className="link" onClick={() => turnNow("suggested")} {...showing(d.sideways)}>
                  {turnItWords(d.sideways)}
                </button>
              </p>
            )}
            {d.error && <p className="note warn">{d.error}</p>}

            <h3>
              Tags <span className="muted shortcut-hint">T</span>
            </h3>
            <TagEditor photoIds={[id]} onChanged={onTagsChanged} inputRef={tagBox} />

            {faces.length > 0 && (
              <>
                <h3>People</h3>
                <div className="faces-row">
                  {faces.map((f) => (
                    <span
                      key={f.id}
                      className={`face-chip ${spot === f.id ? "spotted" : ""}`}
                      onMouseEnter={() => setSpot(f.id)}
                      onMouseLeave={() => setSpot(null)}
                      onFocus={() => setSpot(f.id)}
                      onBlur={() => setSpot(null)}
                    >
                      <img src={faceUrl(f.id)} alt="" />
                      {f.personId != null ? (
                        <button
                          className={`name ${f.name ? "" : "unnamed"}`}
                          onClick={() => onPerson(f.personId!)}
                          title="Show their photos"
                        >
                          {f.name ?? "Unnamed"}
                        </button>
                      ) : (
                        <button
                          className="name who"
                          title="Too small for Shoebox to recognise: say who it is"
                          onClick={() => setWho(f)}
                        >
                          Who is this?
                        </button>
                      )}
                      {f.personId != null && (
                        <button
                          className="not-them"
                          title={`Not ${f.name ?? "this person"}? Say who it is`}
                          onClick={() => setWho(f)}
                        >
                          <UserX size={13} />
                        </button>
                      )}
                    </span>
                  ))}
                </div>
              </>
            )}

            <h3>{d.files.length > 1 ? `${d.files.length} copies` : "Location"}</h3>
            <ul className="locations">
              {d.files.map((f) => (
                <li key={f.id}>
                  <div className="loc-head">
                    <span className={`pill ${f.state}`}>{STATE_LABEL[f.state]}</span>
                    <span className="muted">{f.volumeLabel}</span>
                    <span className="muted right">{fmtBytes(f.size)}</span>
                  </div>
                  <div className="loc-path" title={f.path}>
                    {f.path}
                  </div>
                  <div className="loc-actions">
                    <button
                      className="btn small"
                      onClick={() => onFolder(f)}
                      title={`Show “${f.folder.split("\\").pop() || f.volumeLabel}” here in Shoebox, with this photo selected`}
                    >
                      <FolderInput size={14} /> Folder
                    </button>
                    {f.state === "online" && (
                      <>
                        <button
                          className="btn small"
                          onClick={() => act(api.revealFile(f.id))}
                          title="Show in Explorer"
                        >
                          <FolderOpen size={14} /> Explorer
                        </button>
                        <button
                          className="btn small"
                          onClick={() => act(api.openFile(f.id))}
                          title="Open in its default app"
                        >
                          <ExternalLink size={14} /> Open
                        </button>
                      </>
                    )}
                  </div>
                </li>
              ))}
            </ul>
            {missing && (
              <div className="gone-note">
                <p className="note warn">
                  {binned
                    ? `Deleted: in the ${BIN}${binned.deletedAt ? ` since ${fmtDeleted(binned.deletedAt)}` : ""}. Restore it from there and the next scan finds it again, faces and tags kept.`
                    : "Not found where it was, on a disk that was plugged in. If it was deleted outside Shoebox, take it out of the catalog."}
                </p>
                <button
                  className="btn small danger"
                  onClick={onForget}
                  title="Take this photo out of the catalog. Nothing on any disk is touched."
                >
                  <ArchiveX size={14} /> Remove from catalog…
                </button>
              </div>
            )}
          </>
        )}
        {error && <p className="note warn">{error}</p>}
      </aside>
      {who && (
        <WhoIsIt
          faceIds={[who.id]}
          from={who.personId != null ? { id: who.personId, name: who.name } : null}
          everyone={everyone}
          onClose={() => setWho(null)}
          onDone={() => {
            setWho(null);
            onFacesChanged();
            people
              .inPhoto(id)
              .then(setFaces)
              .catch((e) => setError(String(e)));
          }}
        />
      )}
    </div>
  );
}

/** The photo at 100% (one pixel of the original per screen pixel), centred where
 *  it was clicked. Drag or scroll to look around; a click without dragging fits it again. */
function ZoomedPhoto({
  src,
  fullLong,
  zoom,
  sharp,
  spot,
  focus,
  onExit,
}: {
  src: string;
  /** The original's long edge in pixels (0 if not known yet). */
  fullLong: number;
  zoom: Zoom;
  /** The full-size original has arrived (until then a smaller picture is stretched). */
  sharp: boolean;
  /** The face to box (its name is pointed at). */
  spot: PhotoFace | null;
  /** Where the camera focused, when shown. */
  focus: Focus | null;
  onExit: () => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  const spotEl = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  const drag = useRef<{ x: number; y: number; left: number; top: number; moved: boolean } | null>(null);
  const [dragging, setDragging] = useState(false);

  // The size at 100%: the original's long edge in screen pixels, the shape from the picture loaded.
  const onLoad = (img: HTMLImageElement) => {
    const nl = Math.max(img.naturalWidth, img.naturalHeight);
    const s = (fullLong || nl) / window.devicePixelRatio / nl;
    setSize((old) => {
      const next = { w: Math.round(img.naturalWidth * s), h: Math.round(img.naturalHeight * s) };
      return old && old.w === next.w && old.h === next.h ? old : next;
    });
  };

  // Centre on the clicked point once the size is known.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || !size) return;
    el.scrollLeft = zoom.fx * size.w - el.clientWidth / 2;
    el.scrollTop = zoom.fy * size.h - el.clientHeight / 2;
  }, [size, zoom]);

  // A face out of sight at 100% is scrolled to.
  useEffect(() => {
    spotEl.current?.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  }, [spot?.id, size]);

  useEffect(() => {
    if (!dragging) return;
    const move = (e: MouseEvent) => {
      const d = drag.current;
      const el = box.current;
      if (!d || !el) return;
      const [dx, dy] = [e.clientX - d.x, e.clientY - d.y];
      if (Math.abs(dx) + Math.abs(dy) > 4) d.moved = true;
      el.scrollLeft = d.left - dx;
      el.scrollTop = d.top - dy;
    };
    const up = () => {
      if (drag.current && !drag.current.moved) onExit();
      drag.current = null;
      setDragging(false);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [dragging, onExit]);

  return (
    <div
      className={`viewer-zoom ${dragging ? "dragging" : ""}`}
      ref={box}
      onClick={(e) => e.stopPropagation()}
      onMouseDown={(e) => {
        if (e.button !== 0 || !box.current) return;
        e.preventDefault();
        drag.current = { x: e.clientX, y: e.clientY, left: box.current.scrollLeft, top: box.current.scrollTop, moved: false };
        setDragging(true);
      }}
    >
      <div className="viewer-zoom-canvas">
        <div className="viewer-zoom-photo">
          <img
            src={src}
            alt=""
            draggable={false}
            className={sharp ? "" : "soft"}
            ref={(el) => {
              if (el?.complete && el.naturalWidth) onLoad(el);
            }}
            onLoad={(e) => onLoad(e.currentTarget)}
            style={size ? { width: size.w, height: size.h } : { visibility: "hidden" }}
          />
          {focus && size && <FocusPoints focus={focus} faded={!!spot} />}
          {spot && size && <FaceSpot face={spot} ref={spotEl} />}
        </div>
      </div>
    </div>
  );
}

/** A box where the fitted photo is drawn in the stage (as .viewer-img: contained in
 *  the stage less 24 px each side), so what is drawn in it (a face's box, focus
 *  points) lands on the photo. */
function FittedFrame({
  stage,
  size,
  children,
}: {
  stage: RefObject<HTMLDivElement | null>;
  size: { w: number; h: number };
  children: ReactNode;
}) {
  const [room, setRoom] = useState<{ w: number; h: number } | null>(null);
  useLayoutEffect(() => {
    const el = stage.current;
    if (!el) return;
    const measure = () => setRoom({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const watch = new ResizeObserver(measure);
    watch.observe(el);
    return () => watch.disconnect();
  }, [stage]);
  if (!room) return null;
  const s = Math.min((room.w - 48) / size.w, (room.h - 48) / size.h);
  const [w, h] = [size.w * s, size.h * s];
  return (
    <div className="fitted-frame" style={{ left: (room.w - w) / 2, top: (room.h - h) / 2, width: w, height: h }}>
      {children}
    </div>
  );
}

/** A box around one face, the rest of the photo dimmed, with the name under it.
 *  Placed in a box the size of the photo. */
function FaceSpot({ face, ref }: { face: PhotoFace; ref?: RefObject<HTMLDivElement | null> }) {
  const [x1, y1, x2, y2] = face.bbox;
  return (
    <div
      className="face-spot"
      ref={ref}
      style={{ left: `${x1 * 100}%`, top: `${y1 * 100}%`, width: `${(x2 - x1) * 100}%`, height: `${(y2 - y1) * 100}%` }}
    >
      {face.name && <span>{face.name}</span>}
    </div>
  );
}

/** Thumbnails under the photo; the current one stays in the middle. Only a
 *  window of them is drawn, so a list of 100k costs nothing. */
function Filmstrip({
  list,
  index,
  picked,
  onPick,
  turnOf,
}: {
  list: PhotoList;
  index: number;
  /** Selected: Shift+click a range, Ctrl+click one. */
  picked: number[];
  onPick: (i: number, e: ReactMouseEvent) => void;
  /** Quarter turns each is drawn turned (Sideways: as suggested, as in the grid; a turn pointed at). */
  turnOf: (id: number) => number;
}) {
  const isPicked = useMemo(() => new Set(picked), [picked]);
  const strip = useRef<HTMLDivElement>(null);
  const n = list.ids.length;
  // The window moves in steps of 40, so it doesn't jump on every photo.
  const from = Math.max(0, Math.floor(index / 40) * 40 - 40);
  const to = Math.min(n, from + 120);

  useEffect(() => {
    strip.current?.querySelector(".film.current")?.scrollIntoView({ inline: "center", block: "nearest", behavior: "smooth" });
  }, [index, from]);

  const items = [];
  for (let i = from; i < to; i++) {
    const q = (((turnOf(list.ids[i]) % 4) + 4) % 4);
    const [w, h] = q % 2 ? [list.h[i], list.w[i]] : [list.w[i], list.h[i]];
    const aspect = w && h ? Math.min(2.5, Math.max(0.5, w / h)) : 1.5;
    const width = Math.round(60 * aspect);
    items.push(
      <button
        key={list.ids[i]}
        className={`film ${i === index ? "current" : isPicked.has(list.ids[i]) ? "picked" : ""} ${list.marked[i] === 1 ? "is-marked" : ""}`}
        style={{ width }}
        onClick={(e) => onPick(i, e)}
        title={i === index ? undefined : "Shift+click: select up to here · Ctrl+click: add or remove"}
        tabIndex={-1}
      >
        {list.preview[i] === 1 ? (
          <img
            src={thumbUrl(list.ids[i], list.rev[i])}
            alt=""
            loading="lazy"
            draggable={false}
            className={q ? "turned" : undefined}
            style={q ? turnedStyle(q, width - 4, 56) : undefined}
          />
        ) : (
          <span className="tile-empty" />
        )}
      </button>,
    );
  }
  return (
    <div className="filmstrip" ref={strip}>
      {from > 0 && <span className="film-more">{fmtNum(from)} before</span>}
      {items}
      {to < n && <span className="film-more">{fmtNum(n - to)} after</span>}
    </div>
  );
}
