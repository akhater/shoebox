import { listen } from "@tauri-apps/api/event";
import { ScanEye, Settings2, Smile } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { fmtBytes, modelsApi, type ModelProgress, type ModelsOverview } from "../api";

/** What opening Cull without the closed-eyes model downloads: the model, and an AI runtime when none is here. */
export function eyesDownloads(o: ModelsOverview): string[] {
  const runtime = o.models.some((m) => m.id === "runtime-gpu") ? "runtime-gpu" : "runtime-cpu";
  return [...(o.runtimeReady ? [] : [runtime]), "expressions"];
}

/** Whether Cull should ask for the closed-eyes model: not here, and not on its way already. */
export function eyesMissing(o: ModelsOverview, progress: ModelProgress | null): boolean {
  const here = o.models.some((m) => m.id === "expressions" && m.state === "ready");
  const coming = progress != null && (progress.id === "expressions" || progress.queued.includes("expressions"));
  return !here && !coming;
}

/** Opening Cull without the closed-eyes model: download it (with its progress), not now, or never ask again. */
export function EyesModelAsk({
  models,
  onDone,
}: {
  models: ModelsOverview;
  /** Closes it; `quiet`: never asked again. */
  onDone: (quiet: boolean) => void;
}) {
  const ids = eyesDownloads(models);
  const bytes = models.models.filter((m) => ids.includes(m.id)).reduce((n, m) => n + m.downloadBytes, 0);
  const [quiet, setQuiet] = useState(false);
  const [state, setState] = useState<"ask" | "downloading" | "done">("ask");
  const [current, setCurrent] = useState<ModelProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const first = useRef<HTMLButtonElement>(null);
  const done = useRef(() => onDone(quiet));
  done.current = () => onDone(quiet);

  useEffect(() => {
    first.current?.focus();
    // Escape is "Not now" here, not Cull's own Done.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        done.current();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // The downloads Settings > AI models makes, followed here until the model is in. Listening
  // starts first, so a download never finishes unseen.
  const listening = useRef<Promise<() => void>>(null);
  useEffect(() => {
    const un = listen<ModelProgress>("model-download", (e) => {
      const p = e.payload;
      if (!ids.includes(p.id)) return;
      if (!p.finished) return setCurrent(p);
      setCurrent(null);
      if (p.error) setError(p.error);
      else if (p.id === "expressions") setState("done");
    });
    listening.current = un;
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const download = () => {
    setState("downloading");
    listening.current!.then(() => {
      for (const id of ids) modelsApi.download(id).catch((e) => setError(String(e)));
    });
  };

  const pct = current?.total ? (current.done / current.total) * 100 : 0;
  return (
    <div className="modal-back">
      <div className="modal eyes-ask" role="alertdialog" aria-labelledby="eyes-ask-title">
        <div className="modal-head">
          <h2 id="eyes-ask-title">
            <ScanEye size={16} /> Check eyes in Cull?
          </h2>
        </div>
        <div className="form">
          <ul className="notice-points">
            <li>
              <ScanEye size={16} />
              <div>
                <strong>Closed eyes are flagged.</strong> Of the frames of a burst, Cull keeps one where eyes are open
                and suggests rejecting the blinks. A single photo is only flagged, left to you.
              </div>
            </li>
            <li>
              <Smile size={16} />
              <div>
                <strong>Smiles break ties.</strong> Of frames as sharp as each other, the one smiling more is kept.
              </div>
            </li>
            <li>
              <Settings2 size={16} />
              <div>
                <strong>It needs Google's MediaPipe face model</strong>
                {ids.length > 1 ? " and an AI runtime" : ""}, {fmtBytes(bytes)}, downloaded once and checked. Cull works
                without it meanwhile. You can download it any time from Settings &gt; AI models.
              </div>
            </li>
          </ul>
          {state === "downloading" && !error && (
            <div className="model-progress">
              <span className="bar wide">
                <span style={{ width: `${pct}%` }} />
              </span>
              <small className="muted">
                {current?.total ? `${fmtBytes(current.done)} of ${fmtBytes(current.total)}` : "Starting…"}
              </small>
            </div>
          )}
          {state === "done" && (
            <p className="ok-text">Downloaded and checked. Cull checks eyes the next time it opens photos.</p>
          )}
          {error && <p className="note warn">{error}</p>}
          {state === "ask" && (
            <div className="field">
              <label className="check">
                <input type="checkbox" checked={quiet} onChange={(e) => setQuiet(e.target.checked)} />
                Don't ask again
              </label>
            </div>
          )}
        </div>
        <div className="modal-foot">
          {state === "ask" ? (
            <>
              <button type="button" className="btn ghost" onClick={() => onDone(quiet)}>
                Not now
              </button>
              <button ref={first} type="button" className="btn primary" onClick={download}>
                Download ({fmtBytes(bytes)})
              </button>
            </>
          ) : (
            <button type="button" className="btn primary" onClick={() => onDone(quiet)}>
              {state === "done" || error ? "Close" : "Continue in the background"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
