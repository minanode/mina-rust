/** Time for the UI: a ticking clock and ages as people write them. */

import * as React from "react";

/** `45 s`, `6 min 30 s`, `2 h 5 min`, `3 d 4 h`; `–` when unknown. */
export function formatAge(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "–";
  const s = Math.floor(Math.max(0, ms) / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

/** The current time, every `everyMs`: ages on the page keep counting. */
export function useNow(everyMs = 1000): number {
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(id);
  }, [everyMs]);
  return now;
}
