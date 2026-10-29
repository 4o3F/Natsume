import type { Category, Severity } from "../api/generated/types";
import {
  categoryLabels,
  eventKinds,
  isEventKind,
  severityLabels,
  type EventKind,
} from "../components/event-labels";
import { formatOffset, formatTimestamp, startOfDay } from "../lib/time";

/** Category preset; `all` is administrator-only. */
type CategoryPreset = Category | "all";
/** Relative presets follow the newest events; `custom` is a fixed range. */
export type TimeRange = "any" | "15m" | "1h" | "today" | "custom";

export interface EventFilters {
  preset: CategoryPreset;
  range: TimeRange;
  /** Inclusive lower bound in Unix milliseconds; `custom` only. */
  from: number | null;
  /** Inclusive upper bound in Unix milliseconds; `custom` only. */
  to: number | null;
  /** Exact canonical device ID. */
  deviceId: string | null;
  /** Seat code prefix, matched against committed seat snapshots. */
  seatPrefix: string | null;
  /** Empty means every kind in the preset. */
  kinds: EventKind[];
  /** Empty means every severity. */
  severities: Severity[];
  /** Actor operator ID; administrator-only. */
  operatorId: string | null;
}

/** Resolved wall-clock bounds sent to the server. */
export interface EventRange {
  from: number | null;
  to: number | null;
}

export const defaultEventFilters: EventFilters = {
  preset: "device",
  range: "any",
  from: null,
  to: null,
  deviceId: null,
  seatPrefix: null,
  kinds: [],
  severities: [],
  operatorId: null,
};

const timeRanges = ["any", "15m", "1h", "today", "custom"] as const;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SEAT_PREFIX_MAX = 64;

export const timeRangeLabels = {
  any: "Any time",
  "15m": "15 min",
  "1h": "1 hour",
  today: "Today",
  custom: "Custom",
} as const satisfies Record<TimeRange, string>;

/** Presets offered to the current role; viewers only receive device events. */
export function categoryPresets(isAdmin: boolean) {
  const presets: { value: CategoryPreset; label: string }[] = [
    { value: "device", label: categoryLabels.device },
  ];
  if (isAdmin)
    presets.push(
      { value: "audit", label: categoryLabels.audit },
      { value: "auth", label: categoryLabels.auth },
      { value: "journal", label: categoryLabels.journal },
      { value: "all", label: "All" },
    );
  return presets;
}

/** Event kinds selectable within a preset. */
export function kindsFor(preset: CategoryPreset): EventKind[] {
  return (Object.keys(eventKinds) as EventKind[]).filter(
    (kind) => preset === "all" || eventKinds[kind].category === preset,
  );
}

export const severityOrder = Object.keys(severityLabels) as Severity[];

function isDeviceId(value: string) {
  return UUID.test(value);
}

function timestamp(value: string | null) {
  if (value === null || !/^\d{1,15}$/.test(value)) return null;
  return Number(value);
}

/**
 * Reads filters from the URL, dropping malformed values and anything the role
 * may not request so a shared link never yields a forbidden query.
 */
export function parseEventFilters(
  params: URLSearchParams,
  isAdmin: boolean,
): EventFilters {
  const presets = categoryPresets(isAdmin).map(({ value }) => value);
  const category = params.get("category");
  const preset = presets.find((value) => value === category) ?? "device";
  const range =
    timeRanges.find((value) => value === params.get("range")) ?? "any";
  const allowedKinds = new Set(kindsFor(preset));
  const kinds = [
    ...new Set(
      params
        .getAll("kind")
        .filter((kind) => isEventKind(kind) && allowedKinds.has(kind)),
    ),
  ] as EventKind[];
  const severities = severityOrder.filter((severity) =>
    params.getAll("severity").includes(severity),
  );
  let from = range === "custom" ? timestamp(params.get("from")) : null;
  let to = range === "custom" ? timestamp(params.get("to")) : null;
  if (from !== null && to !== null && from > to) [from, to] = [to, from];
  const deviceId = params.get("device_id");
  const seatPrefix = params.get("seat_prefix")?.trim() ?? "";
  const operatorId = params.get("operator");
  return {
    preset,
    range,
    from,
    to,
    deviceId: deviceId && isDeviceId(deviceId) ? deviceId : null,
    seatPrefix:
      seatPrefix &&
      seatPrefix.length <= SEAT_PREFIX_MAX &&
      // eslint-disable-next-line no-control-regex
      !/[\u0000-\u001f\u007f]/.test(seatPrefix)
        ? seatPrefix
        : null,
    kinds,
    severities,
    operatorId:
      isAdmin && operatorId && isDeviceId(operatorId) ? operatorId : null,
  };
}

/** Writes non-default filters back to the URL. */
export function serializeEventFilters(filters: EventFilters) {
  const params = new URLSearchParams();
  if (filters.preset !== defaultEventFilters.preset)
    params.set("category", filters.preset);
  if (filters.range !== "any") params.set("range", filters.range);
  if (filters.range === "custom") {
    if (filters.from !== null) params.set("from", String(filters.from));
    if (filters.to !== null) params.set("to", String(filters.to));
  }
  if (filters.deviceId) params.set("device_id", filters.deviceId);
  if (filters.seatPrefix) params.set("seat_prefix", filters.seatPrefix);
  for (const kind of filters.kinds) params.append("kind", kind);
  for (const severity of filters.severities)
    params.append("severity", severity);
  if (filters.operatorId) params.set("operator", filters.operatorId);
  return params;
}

/** Interprets the free-text device field as an exact ID or a seat prefix. */
export function deviceFilterFromText(
  text: string,
): Pick<EventFilters, "deviceId" | "seatPrefix"> {
  const value = text.trim();
  if (!value) return { deviceId: null, seatPrefix: null };
  const lower = value.toLowerCase();
  return isDeviceId(lower)
    ? { deviceId: lower, seatPrefix: null }
    : { deviceId: null, seatPrefix: value.slice(0, SEAT_PREFIX_MAX) };
}

const RELATIVE_MS = { "15m": 15 * 60_000, "1h": 60 * 60_000 } as const;

/** Resolves a range preset to bounds; relative presets are anchored at `now`. */
export function resolveEventRange(
  filters: Pick<EventFilters, "range" | "from" | "to">,
  now: number,
  offsetMinutes: number,
): EventRange {
  switch (filters.range) {
    case "any":
      return { from: null, to: null };
    case "15m":
    case "1h":
      return { from: now - RELATIVE_MS[filters.range], to: null };
    case "today":
      return { from: startOfDay(now, offsetMinutes), to: null };
    case "custom":
      return { from: filters.from, to: filters.to };
  }
}

/**
 * Bounds actually queried. Relative presets stay anchored where they were
 * resolved, so the caption names that start rather than "last 15 min".
 */
export function rangeCaption(range: EventRange, offsetMinutes: number) {
  const at = (ms: number) => formatTimestamp(ms, offsetMinutes);
  const zone = formatOffset(offsetMinutes);
  if (range.from !== null && range.to !== null)
    return `${at(range.from)} – ${at(range.to)} ${zone}`;
  if (range.from !== null) return `Since ${at(range.from)} ${zone}`;
  if (range.to !== null) return `Until ${at(range.to)} ${zone}`;
  return null;
}

/** A fixed range cannot gain new events, so head polling stops. */
export function isLiveRange(range: TimeRange) {
  return range !== "custom";
}

/** Server query for the filters, without cursors. */
export function eventQuery(filters: EventFilters, range: EventRange) {
  const params = new URLSearchParams();
  if (filters.preset !== "all") params.set("category", filters.preset);
  for (const kind of filters.kinds) params.append("kind", kind);
  for (const severity of filters.severities)
    params.append("severity", severity);
  if (filters.deviceId) params.set("device_id", filters.deviceId);
  if (filters.seatPrefix) params.set("seat_prefix", filters.seatPrefix);
  if (filters.operatorId) params.set("actor_operator_id", filters.operatorId);
  if (range.from !== null) params.set("from", String(range.from));
  if (range.to !== null) params.set("to", String(range.to));
  return params;
}

/** Same filters plus an ID cursor. */
export function withCursor(
  query: URLSearchParams,
  cursor: { before?: number | null; after?: number | null },
) {
  const params = new URLSearchParams(query);
  if (cursor.after != null) params.set("after", String(cursor.after));
  if (cursor.before != null) params.set("before", String(cursor.before));
  return params;
}
