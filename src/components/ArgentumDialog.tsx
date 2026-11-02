import { open } from "@tauri-apps/plugin-dialog";
import { SlidersHorizontal, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../api";

interface Props {
  /** The program, shown where it is. */
  onFound: (path: string) => void;
  /** "Not now", Escape, or a click outside. */
  onClose: () => void;
}

/** Argentum isn't where Shoebox looks: where to get it, or where it is. */
export function ArgentumDialog({ onFound, onClose }: Props) {
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const find = async () => {
    setProblem(null);
    const path = await open({ multiple: false, title: "Where is Argentum?" });
    if (typeof path !== "string") return;
    try {
      const program = await api.argentumChosen(path);
      if (program) onFound(program);
      else setProblem("That isn't Argentum: look for the program named Argentum.");
    } catch (e) {
      setProblem(String(e));
    }
  };

  return (
    <div className="modal-back" onClick={onClose}>
      <div
        className="modal repoint-modal argentum-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="argentum-title"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2 id="argentum-title">
            <SlidersHorizontal size={18} /> Argentum isn't on this computer
          </h2>
          <button className="icon-btn" onClick={onClose} aria-label="Close">
            <X size={18} />
          </button>
        </div>
        <p className="lead">Shoebox keeps your photos; Argentum, a RAW editor, is where they get edited.</p>
        <p className="repoint-note">
          Installed somewhere else? Find it, and Shoebox remembers. Get Argentum opens its download page in your browser;
          nothing is installed by itself.
        </p>
        {problem && <p className="note warn">{problem}</p>}
        <div className="modal-foot">
          <button className="btn" onClick={onClose}>
            Not now
          </button>
          <button className="btn" onClick={find}>
            Find it…
          </button>
          <button className="btn primary" autoFocus onClick={() => api.openArgentumPage().catch((e) => setProblem(String(e)))}>
            Get Argentum
          </button>
        </div>
      </div>
    </div>
  );
}
