/** Streaming, allowlisted journald export and dataset-only HTTP analysis. */
import { dirname, join } from "@std/path";
import { Cell, csvRows, CsvWriter, lines } from "./io.ts";

export type Bounds = readonly [number, number];
export interface AnalysisOptions {
  windows: Record<string, Bounds>;
  analysisEnd: number | null;
}

function parseTimestamp(value: string): number {
  const match = value.match(
    /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,6}))?)?(Z|[+-]\d{2}:\d{2})$/,
  );
  if (match) {
    const [
      ,
      year,
      month,
      day,
      hour,
      minute,
      second = "00",
      fraction = "",
      zone,
    ] = match;
    const y = Number(year), m = Number(month), d = Number(day);
    const leap = y % 4 === 0 && (y % 100 !== 0 || y % 400 === 0);
    const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    const ts = Date.parse(
          `${year}-${month}-${day}T${hour}:${minute}:${second}${zone}`,
        ) * 1000 + Number(fraction.padEnd(6, "0"));
    if (
      m >= 1 && m <= 12 && d >= 1 && d <= days[m - 1] &&
      Number(hour) < 24 && Number(minute) < 60 && Number(second) < 60 &&
      (zone === "Z" ||
        (Number(zone.slice(1, 3)) < 24 && Number(zone.slice(4)) < 60)) &&
      Number.isSafeInteger(ts)
    ) return ts;
  }
  throw new Error(
    `Invalid timestamp "${value}": expected ISO 8601 with Z or ±HH:MM offset and at most six fractional digits`,
  );
}
function parseInterval(value: string): Bounds {
  const parts = value.split("/");
  if (parts.length !== 2) {
    throw new Error(`Invalid interval "${value}": expected <start>/<end>`);
  }
  const start = parseTimestamp(parts[0]), end = parseTimestamp(parts[1]);
  if (start >= end) {
    throw new Error(`Invalid interval "${value}": start must be before end`);
  }
  return [start, end];
}
export function parseOptions(args: string[]) {
  const [command, source, ...flags] = args;
  if (
    !["extract", "summarize", "series"].includes(command) || !source ||
    source.startsWith("--")
  ) {
    throw new Error(
      "Usage: journald.ts <extract|summarize|series> <source> --out <output> [--window <name>=<start>/<end>] [--analysis-end <iso>] [--series-range <start>/<end>]",
    );
  }
  const windows: Record<string, Bounds> = Object.create(null);
  let out: string | undefined, analysisEnd: number | null = null;
  let seriesRange: Bounds | undefined;
  const seen = new Set<string>();
  for (let i = 0; i < flags.length; i += 2) {
    const flag = flags[i], value = flags[i + 1];
    if (
      flag !== "--out" &&
      (command === "extract" ||
        !["--window", "--analysis-end", "--series-range"].includes(flag))
    ) throw new Error(`Unsupported option: ${flag}`);
    if (flag === "--series-range" && command !== "series") {
      throw new Error("--series-range is only supported by series");
    }
    if (!value || value.startsWith("--")) {
      throw new Error(`Missing value for ${flag}`);
    }
    if (flag !== "--window" && seen.has(flag)) {
      throw new Error(`Duplicate option: ${flag}`);
    }
    seen.add(flag);
    if (flag === "--out") out = value;
    else if (flag === "--analysis-end") analysisEnd = parseTimestamp(value);
    else if (flag === "--series-range") seriesRange = parseInterval(value);
    else {
      const separator = value.indexOf("="), name = value.slice(0, separator);
      if (separator < 1 || !/^[A-Za-z][A-Za-z0-9_-]*$/.test(name)) {
        throw new Error(
          "Invalid --window: expected <name>=<start>/<end>; name must use letters, digits, _ or - and start with a letter",
        );
      }
      if (name === "all" || /^p[0-9]+$/.test(name)) {
        throw new Error(`Reserved window name: ${name}`);
      }
      if (Object.hasOwn(windows, name)) {
        throw new Error(`Duplicate window name: ${name}`);
      }
      windows[name] = parseInterval(value.slice(separator + 1));
    }
  }
  if (!out) throw new Error("Missing required option: --out");
  if (command === "series" && !seriesRange) {
    throw new Error("Missing required option for series: --series-range");
  }
  return { command, source, out, windows, analysisEnd, seriesRange };
}
export const LOGO = "/api/v2/organizations/{organization_id}/logo";
export const CONTROL = "/api/v2/device/control";
const REQUEST_FIELDS =
  "ts_us process level event method route status duration_us code cause actor"
    .split(" ");
export const LIFECYCLE_FIELDS =
  "process boot start_ts_us first_server_ts_us last_server_ts_us stop_ts_us cpu_seconds wall_seconds memory_peak_bytes end_reason"
    .split(" ");
const NOTE_FIELDS = "ts_us process level event organization port".split(" ");
const FIELD =
  /\b([a-zA-Z_][\w.]*)=("(?:[^"\\]|\\.)*"|(?:\{[a-z_]+\}|[^\s}])+)/g;
const LEVEL = /\b(TRACE|DEBUG|INFO|WARN|ERROR)\b/;
const EVENTS: Record<string, string> = {
  "server mode running": "server_mode_running",
  "database ready": "database_ready",
  "server state ready": "server_state_ready",
  "TLS identity loaded": "tls_identity_loaded",
  "listener bound": "listener_bound",
  "graceful shutdown initiated": "shutdown_initiated",
  "graceful shutdown completed": "shutdown_completed",
  "School logo could not be read": "logo_read_failed",
};
export interface JournalRecord {
  __REALTIME_TIMESTAMP: string;
  SYSLOG_IDENTIFIER?: string;
  _BOOT_ID?: string;
  _PID?: string | number;
  MESSAGE?: string | number[];
  [key: string]: unknown;
}
export function message(record: Pick<JournalRecord, "MESSAGE">): string {
  return Array.isArray(record.MESSAGE)
    ? new TextDecoder().decode(new Uint8Array(record.MESSAGE))
    : record.MESSAGE ?? "";
}
export function fields(text: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const match of text.matchAll(FIELD)) {
    const key = match[1],
      value = match[2].startsWith('"') ? JSON.parse(match[2]) : match[2];
    if (Object.hasOwn(result, key) && result[key] !== value) {
      throw new Error("Conflicting duplicated tracing field");
    }
    result[key] = value;
  }
  return result;
}
function level(text: string) {
  const match = text.match(LEVEL);
  if (!match) throw new Error("Missing tracing level");
  return match[1];
}
const safeToken = (value = "") =>
  /^[A-Za-z][A-Za-z0-9_:-]{0,100}$/.test(value) ? value : "";
export function parseRequest(text: string) {
  const event = text.includes("HTTP request completed")
    ? "completed"
    : text.includes("HTTP request rejected")
    ? "rejected"
    : undefined;
  if (!event) return;
  const f = fields(text), route = f.route ?? f["http.route"] ?? "";
  if (
    !["", "<unknown>", "<unmatched>", "/"].includes(route) &&
    !/^\/api\/v2(?:\/(?:[a-z][a-z-]*|\{[a-z_]+\}))*$/.test(route)
  ) throw new Error("Non-templated or unsupported request route");
  const status = f.status ?? f["http.response.status_code"] ?? "",
    duration = f.duration_us ?? "";
  if (status && !/^[1-5][0-9]{2}$/.test(status)) {
    throw new Error("Invalid HTTP status");
  }
  if (duration && !/^[0-9]+$/.test(duration)) {
    throw new Error("Invalid request duration");
  }
  if (event === "completed" && (!status || !duration)) {
    throw new Error("Completed request missing status or duration");
  }
  const method = f.method ?? f["http.request.method"] ?? "";
  if (
    ![
      "",
      "GET",
      "POST",
      "PUT",
      "PATCH",
      "DELETE",
      "HEAD",
      "OPTIONS",
      "CONNECT",
      "TRACE",
    ].includes(method)
  ) throw new Error("Unsupported HTTP method");
  return {
    level: level(text),
    event,
    method,
    route,
    status,
    duration_us: duration,
    code: safeToken(f.code),
    cause: safeToken(f.cause),
    actor_id: f.actor_id ?? "",
    actor_kind: f.actor_kind ?? "",
  };
}
function roundEven(value: number) {
  const floor = Math.floor(value);
  return value - floor === 0.5 ? floor + floor % 2 : Math.round(value);
}
export function seconds(value: string) {
  const units: Record<string, number> = {
    w: 604800,
    d: 86400,
    h: 3600,
    min: 60,
    s: 1,
    ms: .001,
    us: .000001,
  };
  const pattern = /([0-9]+(?:\.[0-9]+)?)(min|ms|us|w|d|h|s)/g;
  const parts = [...value.matchAll(pattern)];
  if (!parts.length || value.replace(pattern, "").replace(/\s+/g, "")) {
    throw new Error("Unsupported systemd duration");
  }
  return roundEven(
    parts.reduce((sum, part) => sum + Number(part[1]) * units[part[2]], 0) *
      1e6,
  ) / 1e6;
}
export function consumed(text: string): Record<string, Cell> {
  const match = text.match(
    /Consumed (.+?) CPU time(?: over (.+?) wall clock time)?(?:, ([0-9.]+[KMGTPE]?(?:i?B)?) memory peak)?\./,
  );
  if (!match) throw new Error("Unsupported systemd Consumed record");
  let peak: Cell = "";
  if (match[3]) {
    const memory = match[3].match(/^([0-9.]+)([KMGTPE]?)(?:i?B)?$/)!;
    peak = roundEven(
      Number(memory[1]) *
        1024 ** (memory[2] ? "KMGTPE".indexOf(memory[2]) + 1 : 0),
    );
  }
  return {
    cpu_seconds: seconds(match[1]),
    wall_seconds: match[2] ? seconds(match[2]) : "",
    memory_peak_bytes: peak,
  };
}
function alias(
  table: Map<string, string>,
  key: string,
  prefix: string,
  width = 0,
) {
  if (!table.has(key)) {
    table.set(key, prefix + String(table.size + 1).padStart(width, "0"));
  }
  return table.get(key)!;
}
function increment<K>(map: Map<K, number>, key: K, amount = 1) {
  map.set(key, (map.get(key) ?? 0) + amount);
}

export async function extract(log: string, out: string) {
  await Deno.mkdir(out, { recursive: true });
  const processes = new Map<string, Record<string, Cell>>(),
    current = new Map<string, Record<string, Cell>>();
  const boots = new Map<string, string>(),
    actors = new Map<string, string>(),
    organizations = new Map<string, string>();
  const pending = new Map<string, number>(), counts = new Map<string, number>();
  let previousTs = -1;
  const requests = await CsvWriter.open(
    join(out, "requests.csv.gz"),
    REQUEST_FIELDS,
  );
  const notes = await CsvWriter.open(join(out, "notes.csv"), NOTE_FIELDS);
  try {
    for await (const line of lines(log)) {
      const record: JournalRecord = JSON.parse(line),
        ts = Number(record.__REALTIME_TIMESTAMP);
      increment(counts, "journal_records");
      if (ts < previousTs) {
        throw new Error(
          "Input must be in timestamp order for stable process aliases",
        );
      }
      previousTs = ts;
      const ident = record.SYSLOG_IDENTIFIER;
      if (ident !== "natsume-server" && ident !== "systemd") continue;
      const boot = record._BOOT_ID ?? "", text = message(record);
      if (ident === "systemd") {
        if (/\bStarted\b/.test(text)) pending.set(boot, ts);
        const p = current.get(boot);
        if (!p) continue;
        if (text.includes("Consumed ")) Object.assign(p, consumed(text));
        else if (/\bStopped\b/.test(text)) {
          p.stop_ts_us = ts;
          if (p.end_reason !== "graceful") p.end_reason = "stopped";
        } else if (text.includes("Deactivated successfully")) {
          p.end_reason = p.end_reason === "graceful"
            ? "graceful"
            : "deactivated";
        } else if (
          text.includes("Main process exited") ||
          text.includes("Failed with result")
        ) {
          p.stop_ts_us = ts;
          p.end_reason = "failed";
        }
        continue;
      }
      const key = JSON.stringify([boot, String(record._PID)]);
      if (!processes.has(key)) {
        for (const [previousBoot, previous] of current) {
          if (previous.end_reason === "unobserved") {
            previous.end_reason = previousBoot !== boot
              ? "boot_changed"
              : "replaced_without_stop";
          }
        }
        const p: Record<string, Cell> = Object.fromEntries(
          LIFECYCLE_FIELDS.map((field) => [field, ""]),
        );
        Object.assign(p, {
          process: `p${String(processes.size + 1).padStart(2, "0")}`,
          boot: alias(boots, boot, "b"),
          start_ts_us: pending.get(boot) ?? ts,
          first_server_ts_us: ts,
          end_reason: "unobserved",
        });
        pending.delete(boot);
        processes.set(key, p);
        current.set(boot, p);
      }
      const p = processes.get(key)!;
      p.last_server_ts_us = ts;
      const request = parseRequest(text);
      if (request) {
        const { actor_id, actor_kind, ...safe } = request;
        if (actor_id && actor_kind !== "operator") {
          throw new Error("Unsupported actor kind");
        }
        const write = requests.row({
          ts_us: ts,
          process: p.process,
          actor: actor_id ? alias(actors, actor_id, "operator-") : "",
          ...safe,
        });
        if (write) await write;
        increment(counts, request.event);
        continue;
      }
      const event = Object.entries(EVENTS).find(([phrase]) =>
          text.includes(phrase)
        )?.[1] ?? "other",
        f = fields(text);
      const organization = Object.hasOwn(f, "organization_id")
        ? alias(organizations, f.organization_id, "org-", 2)
        : "";
      const candidate = (f.listen_address ?? "").split(":").at(-1)!;
      const port = event === "listener_bound" && /^[0-9]+$/.test(candidate) &&
          Number(candidate) > 0 && Number(candidate) <= 65535
        ? candidate
        : "";
      const write = notes.row({
        ts_us: ts,
        process: p.process,
        level: level(text),
        event,
        organization,
        port,
      });
      if (write) await write;
      increment(counts, "notes");
      if (event === "shutdown_completed") {
        p.stop_ts_us = ts;
        p.end_reason = "graceful";
      }
    }
  } finally {
    await Promise.all([requests.close(), notes.close()]);
  }
  const lifecycle = await CsvWriter.open(
    join(out, "lifecycle.csv"),
    LIFECYCLE_FIELDS,
  );
  try {
    for (const p of processes.values()) {
      if (p.end_reason === "unobserved") p.end_reason = "log_end";
      const row = { ...p };
      for (const field of ["cpu_seconds", "wall_seconds"]) {
        if (typeof row[field] === "number" && Number.isInteger(row[field])) {
          row[field] = `${row[field]}.0`;
        }
      }
      await lifecycle.row(row);
    }
  } finally {
    await lifecycle.close();
  }
  return {
    ...Object.fromEntries(counts),
    processes: processes.size,
    actors: actors.size,
  };
}

/** Grow geometrically without retaining boxed latency samples. */
class Samples {
  private values = new Float64Array(16);
  length = 0;
  add(value: number) {
    if (this.length === this.values.length) {
      const next = new Float64Array(this.length * 2);
      next.set(this.values);
      this.values = next;
    }
    this.values[this.length++] = value;
  }
  ordered() {
    return this.values.subarray(0, this.length).sort();
  }
}
export function percentile(ordered: ArrayLike<number>, p: number) {
  return ordered.length
    ? ordered[Math.max(0, Math.ceil(ordered.length * p / 100) - 1)]
    : null;
}
export function distribution(
  values: ArrayLike<number> | Samples,
  percentiles = [50, 95, 99],
) {
  const ordered = values instanceof Samples
    ? values.ordered()
    : Float64Array.from(values).sort();
  return {
    count: ordered.length,
    ...Object.fromEntries(
      percentiles.map((p) => [`p${p}`, percentile(ordered, p)]),
    ),
    max: ordered.length ? ordered[ordered.length - 1] : null,
  };
}
export const inWindow = (ts: number, bounds: Bounds) =>
  bounds[0] <= ts && ts < bounds[1];
export async function readProcesses(dataset: string) {
  return await Array.fromAsync(csvRows(join(dataset, "lifecycle.csv")));
}
export interface RequestRow {
  ts_us: number;
  duration_us: number | null;
  process: string;
  event: string;
  route: string;
  status: string;
  method: string;
  actor: string;
}
export async function* readRequests(
  dataset: string,
): AsyncGenerator<RequestRow> {
  for await (const row of csvRows(join(dataset, "requests.csv.gz"))) {
    yield {
      ...row,
      ts_us: Number(row.ts_us),
      duration_us: row.duration_us ? Number(row.duration_us) : null,
    } as RequestRow;
  }
}
export function localIso(ts: number) {
  const date = new Date(Math.floor(ts / 1e6) * 1000 + 8 * 3600000).toISOString()
    .slice(0, 19);
  const fraction = ts % 1000000;
  return date + (fraction ? `.${String(fraction).padStart(6, "0")}` : "") +
    "+08:00";
}
// JSON tuple keys retain exact grouping; sorting compares tuple elements as Python does.
const tuple = (...parts: (string | number)[]) => JSON.stringify(parts);
function sorted<V>(map: Map<string, V>): [string, V][] {
  return [...map].sort(([a], [b]) => {
    const left = JSON.parse(a), right = JSON.parse(b);
    for (let i = 0; i < left.length; i++) {
      if (left[i] < right[i]) return -1;
      if (left[i] > right[i]) return 1;
    }
    return 0;
  });
}
function samples(map: Map<string, Samples>, key: string) {
  if (!map.has(key)) map.set(key, new Samples());
  return map.get(key)!;
}
class Window {
  events = new Map<string, number>();
  counts = new Map<string, number>();
  latencies = new Map<string, Samples>();
  logoMinutes = new Map<number, number>();
  hours = new Map<string, number>();
  start: number | undefined;
  end: number | undefined;
  add(row: RequestRow) {
    const ts = row.ts_us;
    this.start = Math.min(this.start ?? ts, ts);
    this.end = Math.max(this.end ?? ts, ts);
    increment(this.events, row.event);
    increment(this.counts, tuple(row.event, row.route, row.method, row.status));
    if (row.event !== "completed") return;
    samples(this.latencies, tuple(row.route, row.status)).add(row.duration_us!);
    increment(this.hours, tuple(Math.floor(ts / 3600000000), row.route));
    if (row.route === LOGO) {
      increment(this.logoMinutes, Math.floor(ts / 60000000));
    }
  }
  result(bounds?: Bounds) {
    bounds ??= this.start === undefined
      ? undefined
      : [this.start, this.end! + 1];
    const minutes: number[] = [];
    if (bounds) {
      for (
        let t = Math.floor(bounds[0] / 60000000);
        t <= Math.floor((bounds[1] - 1) / 60000000);
        t++
      ) minutes.push(this.logoMinutes.get(t) ?? 0);
    }
    const marginal: Record<string, unknown> = {};
    for (
      const [dimension, index] of [["route", 1], ["method", 2], [
        "status",
        3,
      ]] as const
    ) {
      const totals = new Map<string, number>();
      for (const [key, count] of this.counts) {
        const parts = JSON.parse(key);
        increment(totals, tuple(parts[0], parts[index]), count);
      }
      marginal[`counts_by_${dimension}`] = sorted(totals).map(
        ([key, count]) => {
          const [event, value] = JSON.parse(key);
          return {
            event,
            [dimension]: value,
            count,
            share: count / this.events.get(event)!,
          };
        },
      );
    }
    const secondsObserved = bounds ? (bounds[1] - bounds[0]) / 1e6 : 0;
    return {
      bounds_ts_us: bounds ?? null,
      events: Object.fromEntries(this.events),
      ...marginal,
      completed_requests_per_second: secondsObserved
        ? (this.events.get("completed") ?? 0) / secondsObserved
        : null,
      requests: sorted(this.counts).map(([key, count]) => {
        const [event, route, method, status] = JSON.parse(key);
        return {
          event,
          route,
          method,
          status,
          count,
          share: count / this.events.get(event)!,
        };
      }),
      latency_us: sorted(this.latencies).map(([key, values]) => {
        const [route, status] = JSON.parse(key);
        return { route, status, ...distribution(values) };
      }),
      logo_requests_per_minute: distribution(minutes, [1, 50, 99]),
      hourly_completed_requests: sorted(this.hours).map(([key, count]) => {
        const [hour, route] = JSON.parse(key);
        return { hour: localIso(hour * 3600000000), route, count };
      }),
    };
  }
}
export async function summarize(
  dataset: string,
  out: string,
  { windows: namedWindows, analysisEnd }: AnalysisOptions,
) {
  const processes = await readProcesses(dataset),
    starts = new Map(processes.map((p) => [p.process, Number(p.start_ts_us)]));
  const windows = new Map(
    ["all", ...Object.keys(namedWindows), ...starts.keys()].map((
      name,
    ) => [name, new Window()]),
  );
  const storms = new Map(
    [...starts.keys()].map((p) => [p, new Map<number, number>()]),
  );
  const delays = new Map([...starts.keys()].map((p) => [p, new Samples()]));
  const walls = new Map(
    [...starts.keys()].map((p) => [p, new Map<string, number>()]),
  );
  for await (const row of readRequests(dataset)) {
    const ts = row.ts_us, process = row.process;
    for (
      const name of [
        "all",
        process,
        ...Object.entries(namedWindows).filter(([, b]) => inWindow(ts, b)).map((
          [n],
        ) => n),
      ]
    ) windows.get(name)!.add(row);
    if (row.event === "completed") {
      increment(walls.get(process)!, tuple(row.route), row.duration_us!);
      const delay = ts - starts.get(process)!;
      if (
        row.route === CONTROL && row.status === "101" && delay >= 0 &&
        delay < 600000000
      ) {
        increment(storms.get(process)!, Math.floor(delay / 1e6));
        delays.get(process)!.add(delay);
      }
    }
  }
  const processTable = processes.map((original) => {
    const p: Record<string, string | number | boolean | null | unknown[]> = {
      ...original,
    };
    for (const name of LIFECYCLE_FIELDS.slice(2, -1)) {
      p[name] = original[name] ? Number(original[name]) : null;
    }
    p.start_local = localIso(Number(p.start_ts_us));
    p.before_analysis_cutoff = analysisEnd === null ||
      Number(p.start_ts_us) < analysisEnd;
    const wall = walls.get(original.process)!,
      total = [...wall.values()].reduce((a, b) => a + b, 0);
    p.handler_wall_time_us = total;
    p.handler_wall_time = sorted(wall).map(([key, duration_us]) => ({
      route: JSON.parse(key)[0],
      duration_us,
      share: total ? duration_us / total : 0,
    }));
    return p;
  });
  const result = {
    timezone: "UTC+08:00",
    window_convention: "start inclusive, end exclusive",
    named_windows: Object.fromEntries(
      Object.entries(namedWindows).map((
        [n, b],
      ) => [n, { start: localIso(b[0]), end: localIso(b[1]) }]),
    ),
    analysis_cutoff: analysisEnd === null ? null : localIso(analysisEnd),
    processes: processTable,
    windows: Object.fromEntries(
      [...windows].map(([n, w]) => [n, w.result(namedWindows[n])]),
    ),
    reconnect_storms: [...starts].map(([process, start]) => ({
      process,
      before_analysis_cutoff: analysisEnd === null || start < analysisEnd,
      horizon_seconds: 600,
      counts_per_second: Array.from(
        { length: 600 },
        (_, s) => storms.get(process)!.get(s) ?? 0,
      ),
      delay_us: distribution(delays.get(process)!, [50, 90, 99]),
      peak_per_second: Math.max(0, ...storms.get(process)!.values()),
    })),
  };
  await Deno.mkdir(dirname(out), { recursive: true });
  await Deno.writeTextFile(out, JSON.stringify(result, null, 2) + "\n");
}
export async function series(dataset: string, out: string, range: Bounds) {
  await Deno.mkdir(out, { recursive: true });
  const processes = await readProcesses(dataset),
    starts = new Map(processes.map((p) => [p.process, Number(p.start_ts_us)]));
  const minutes = new Map<string, number>(),
    reconnect = new Map<string, number>(),
    latencies = new Map<string, Samples>();
  for await (const row of readRequests(dataset)) {
    if (row.event !== "completed") continue;
    const { ts_us: ts, route, status } = row;
    if (inWindow(ts, range)) {
      increment(minutes, tuple(Math.floor(ts / 60000000), route));
    }
    const delay = ts - starts.get(row.process)!;
    if (
      route === CONTROL && status === "101" && delay >= 0 && delay < 600000000
    ) increment(reconnect, tuple(row.process, Math.floor(delay / 1e6)));
    if ((route === LOGO && status === "304") || route === "/api/v2/devices") {
      samples(latencies, tuple(route, status)).add(row.duration_us!);
    }
  }
  const minuteWriter = await CsvWriter.open(join(out, "minute_routes.csv"), [
    "minute",
    "route",
    "count",
  ]);
  try {
    for (const [key, count] of sorted(minutes)) {
      const [minute, route] = JSON.parse(key);
      await minuteWriter.row({
        minute: localIso(minute * 60000000),
        route,
        count,
      });
    }
  } finally {
    await minuteWriter.close();
  }
  const reconnectWriter = await CsvWriter.open(join(out, "reconnect.csv"), [
    "process",
    "second",
    "count",
  ]);
  try {
    for (const process of starts.keys()) {
      for (let second = 0; second < 600; second++) {
        await reconnectWriter.row({
          process,
          second,
          count: reconnect.get(tuple(process, second)) ?? 0,
        });
      }
    }
  } finally {
    await reconnectWriter.close();
  }
  const latencyWriter = await CsvWriter.open(join(out, "latency_cdf.csv"), [
    "route",
    "status",
    "quantile",
    "duration_us",
    "sample_count",
  ]);
  try {
    for (const [key, values] of sorted(latencies)) {
      const [route, status] = JSON.parse(key), ordered = values.ordered();
      for (let point = 0; point <= 100; point++) {
        await latencyWriter.row({
          route,
          status,
          quantile: point === 0 || point === 100
            ? `${point / 100}.0`
            : point / 100,
          duration_us: percentile(ordered, point)!,
          sample_count: values.length,
        });
      }
    }
  } finally {
    await latencyWriter.close();
  }
}
if (import.meta.main) {
  let options: ReturnType<typeof parseOptions>;
  try {
    options = parseOptions(Deno.args);
  } catch (error) {
    console.error(error instanceof Error ? error.message : "Invalid arguments");
    Deno.exit(1);
  }
  try {
    const { command, source, out, seriesRange } = options;
    if (command === "extract") {
      console.log(JSON.stringify(await extract(source, out)));
    } else if (command === "summarize") await summarize(source, out, options);
    else await series(source, out, seriesRange!);
  } catch {
    console.error(
      "Analysis failed; check arguments, permissions and input format. Regenerate the command's outputs before use.",
    );
    Deno.exit(1);
  }
}
