import { useEffect, useState } from "react";

/** A per-viewer on/off remembered between sessions (convenience only). */
export function useRemembered(key: string, initial: boolean) {
  const [on, setOn] = useState(() => {
    try {
      const v = localStorage.getItem(key);
      return v == null ? initial : v === "1";
    } catch {
      return initial;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(key, on ? "1" : "0");
    } catch {
      /* per-viewer convenience only */
    }
  }, [key, on]);
  return [on, setOn] as const;
}
