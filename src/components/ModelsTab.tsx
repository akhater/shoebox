import { listen } from "@tauri-apps/api/event";
import { CircleStop, Cpu, Download, FolderOpen, Gpu, Package, RotateCwSquare, ScanFace, Search, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { fmtBytes, modelsApi, type ModelProgress, type ModelStatus, type ModelsOverview } from "../api";

const ICONS: Record<string, typeof Search> = {
  clip: Search,
  faces: ScanFace,
  upright: RotateCwSquare,
  "runtime-gpu": Gpu,
  "runtime-cpu": Cpu,
};

/** What the backend says when Stop was clicked. */
const STOPPED = "Download stopped.";

/** The line under a model after a download or delete. */
interface Note {
  id: string;
  text: string;
  tone: "ok" | "warn" | "muted";
}

/** Settings > AI models: what Shoebox can use, what is here; download or delete. */
export function ModelsTab() {
  const [o, setO] = useState<ModelsOverview | null>(null);
  const [current, setCurrent] = useState<ModelProgress | null>(null);
  const [queued, setQueued] = useState<string[]>([]);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [note, setNote] = useState<Note | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () =>
    modelsApi
      .list()
      .then(setO)
      .catch((e) => setError(String(e)));

  useEffect(() => {
    refresh();
    modelsApi.progress().then((p) => {
      if (p) {
        setCurrent(p);
        setQueued(p.queued);
      }
    });
    const un = listen<ModelProgress>("model-download", (e) => {
      const p = e.payload;
      setQueued(p.queued);
      if (!p.finished) {
        setCurrent(p);
        return;
      }
      setCurrent(null);
      if (p.error) setNote({ id: p.id, text: p.error, tone: p.error === STOPPED ? "muted" : "warn" });
      else
        setNote({
          id: p.id,
          text: p.done ? `Downloaded and checked (${fmtBytes(p.done)}).` : "Already here.",
          tone: "ok",
        });
      refresh();
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const download = (m: ModelStatus) => {
    setNote(null);
    setQueued((q) => (current ? [...q, m.id] : q));
    modelsApi.download(m.id).catch((e) => setNote({ id: m.id, text: String(e), tone: "warn" }));
  };

  const remove = async (m: ModelStatus) => {
    setConfirming(null);
    try {
      const freed = await modelsApi.delete(m.id);
      setNote({ id: m.id, text: `Deleted, ${fmtBytes(freed)} freed.`, tone: "ok" });
    } catch (e) {
      setNote({ id: m.id, text: String(e), tone: "warn" });
    }
    refresh();
  };

  if (!o) return <div className="models">{error ? <p className="note warn">{error}</p> : <small>Looking…</small>}</div>;

  const ready = (id: string) => o.models.some((m) => m.id === id && m.state === "ready");
  const other = (id: string) => (id === "runtime-gpu" ? "runtime-cpu" : "runtime-gpu");
  const hasTwoRuntimes = o.models.some((m) => m.id === "runtime-gpu");
  const afterDelete = (m: ModelStatus) => {
    if (m.later) return "Shoebox doesn't use it yet. If a feature needs it one day, Shoebox fetches it then.";
    if (m.id === "clip")
      return "Word search stops until it's downloaded again. Photos already described keep their descriptions: nothing is redone.";
    if (m.id === "faces")
      return "New photos won't get faces until it's downloaded again. Your people and their names stay.";
    if (m.id === "upright")
      return "New photos aren't looked at for being sideways until it's downloaded again. Photos you turned stay turned.";
    if (ready(other(m.id)))
      return m.id === "runtime-gpu"
        ? "Shoebox then runs the models on the processor, which is slower."
        : "Nothing changes: the graphics-card runtime is used while it's here.";
    return "Word search, faces and sideways photos stop until an AI runtime is downloaded again.";
  };

  const row = (m: ModelStatus) => {
    const Icon = ICONS[m.id] ?? Package;
    const downloading = current?.id === m.id;
    const waiting = !downloading && queued.includes(m.id);
    const pct = downloading && current.total ? (current.done / current.total) * 100 : 0;
    const state = m.later ? null : m.state === "ready" ? (
      <span className="model-state ok">Here</span>
    ) : m.state === "partial" ? (
      <span className="model-state warn-text">Incomplete</span>
    ) : (
      <span className="model-state muted">Not downloaded</span>
    );
    return (
      <div key={m.id} className={`model ${m.state}`}>
        <Icon size={18} className="model-icon" />
        <div className="model-main">
          <div className="model-title">
            <b>{m.feature}</b>
            {state}
            {m.inUse && <span className="model-tag">In use</span>}
          </div>
          <small>{m.about}</small>
          <small className="muted">
            {m.name} · {m.publisher} · {m.licence}
          </small>
          {downloading && (
            <div className="model-progress">
              <span className="bar wide">
                <span style={{ width: `${pct}%` }} />
              </span>
              <small className="muted">
                {current.total ? `${fmtBytes(current.done)} of ${fmtBytes(current.total)}` : "Starting…"}
              </small>
            </div>
          )}
          {confirming === m.id && (
            <div className="confirm bad">
              <p>
                Delete {m.name} ({fmtBytes(m.bytesHere)})? {afterDelete(m)}
              </p>
              <div className="row end">
                <button className="btn small ghost" onClick={() => setConfirming(null)}>
                  Cancel
                </button>
                <button className="btn small danger" onClick={() => remove(m)}>
                  Delete
                </button>
              </div>
            </div>
          )}
          {note?.id === m.id && <small className={`${note.tone}-text`}>{note.text}</small>}
        </div>
        <div className="model-act">
          <span className="muted small">{fmtBytes(m.state === "missing" ? m.size : m.bytesHere)}</span>
          {downloading ? (
            <button className="btn small" onClick={() => modelsApi.stop(m.id)}>
              <CircleStop size={14} /> Stop
            </button>
          ) : waiting ? (
            <button
              className="btn small ghost"
              onClick={() => modelsApi.stop(m.id).then(() => setQueued((q) => q.filter((x) => x !== m.id)))}
            >
              Waiting… cancel
            </button>
          ) : m.state !== "ready" && !m.later ? (
            <button className="btn small primary" onClick={() => download(m)}>
              <Download size={14} /> Download {fmtBytes(m.downloadBytes)}
            </button>
          ) : m.state === "ready" || m.bytesHere > 0 ? (
            <button
              className="btn small ghost"
              onClick={() => setConfirming(m.id)}
              disabled={m.inUse}
              title={m.inUse ? "Shoebox has it loaded: it can be deleted after a restart." : undefined}
            >
              <Trash2 size={14} /> Delete
            </button>
          ) : null}
        </div>
      </div>
    );
  };

  const used = o.models.filter((m) => !m.later && !m.id.startsWith("runtime-"));
  const runtimes = o.models.filter((m) => m.id.startsWith("runtime-"));
  const later = o.models.filter((m) => m.later && m.bytesHere > 0);
  const here = o.models.reduce((n, m) => n + m.bytesHere, 0);

  return (
    <div className="models">
      <small className="muted">
        Shoebox's AI runs on this computer, never in the cloud. The models don't come with the app: download the ones
        you want from their publishers. Each file is checked (SHA-256) before it's used.
      </small>
      {!o.runtimeReady && <p className="note warn">Word search, faces and sideways photos need an AI runtime: download one below.</p>}

      <div className="model-group">
        <span className="field-label">Features</span>
        {used.map(row)}
      </div>

      <div className="model-group">
        <span className="field-label">AI runtime</span>
        {hasTwoRuntimes && (
          <small className="muted">One is enough. The graphics-card one is used whenever it's here.</small>
        )}
        {runtimes.map(row)}
      </div>

      {later.length > 0 && (
        <div className="model-group">
          <span className="field-label">Not used by Shoebox yet</span>
          <small className="muted">Copied here for features that may come later. Delete them to free the space.</small>
          {later.map(row)}
        </div>
      )}

      <div className="field">
        <span className="field-label">Models folder</span>
        <div className="row">
          <code className="grow">{o.dir}</code>
          <button className="btn small" onClick={() => modelsApi.showFolder().catch((e) => setError(String(e)))}>
            <FolderOpen size={14} /> Show
          </button>
        </div>
        <small className="muted">{fmtBytes(here)} here.</small>
      </div>
      {error && <p className="note warn">{error}</p>}
    </div>
  );
}
