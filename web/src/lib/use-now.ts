import { useEffect, useState } from "react";

/** Current time that re-renders every `intervalMs` while `enabled`. */
export function useNow(intervalMs = 1_000, enabled = true) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!enabled) return;
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs, enabled]);
  return now;
}
