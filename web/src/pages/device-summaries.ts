import type {
  ConnectionStateResponse,
  DeviceSummary,
} from "../api/generated/types";

/** Window for the device table's journal columns and flapping tile. */
export const SUMMARY_WINDOW_MS = 24 * 3_600_000;
/**
 * A device is flapping when the journal holds at least this many disconnects
 * for it inside `SUMMARY_WINDOW_MS`. Three separates repeated instability
 * from a single planned restart plus one incidental network drop.
 */
export const FLAPPING_DISCONNECTS = 3;

export function isFlapping(summary: DeviceSummary | undefined) {
  return (summary?.disconnects ?? 0) >= FLAPPING_DISCONNECTS;
}

/**
 * Last time the device was known connected: now while a lease exists,
 * otherwise the latest retained connect or disconnect fact.
 */
export function lastSeen(
  summary: DeviceSummary | undefined,
  connection: ConnectionStateResponse,
): "now" | number | null {
  if (connection !== "offline") return "now";
  if (!summary) return null;
  const times = [
    summary.last_connected_at,
    summary.last_disconnected_at,
  ].filter((value): value is number => value !== null);
  return times.length ? Math.max(...times) : null;
}
