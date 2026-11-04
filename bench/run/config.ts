import { parse } from "@std/toml";

export interface Experiment {
  hosts: { server: string; loadgen: string; server_url: string };
  name: string;
  date: string;
  version: string;
  devices: number;
  scenario: string;
  steady_seconds: number;
  warmup_seconds: number;
  ramp_seconds: number;
  sample_seconds: number;
  logo_interval_seconds: number;
  tabs: number;
  panel_rates: Record<string, number>;
  baseline: string;
  fleet_baseline: string;
  seats: string;
  cold_etag: boolean;
  require_active: boolean;
  actions: {
    at: number;
    kind: "restart" | "export" | "perf" | "probe";
    seconds?: number;
    call_graph?: "fp" | "dwarf";
  }[];
}

export function configuration(encoded: string): Experiment {
  const raw = parse(encoded);
  const value = {
    hosts: raw.hosts as Experiment["hosts"],
    name: "",
    date: "2026-11-03",
    version: "v2.10.3",
    devices: 418,
    scenario: "steady",
    steady_seconds: 600,
    warmup_seconds: 120,
    ramp_seconds: 5,
    sample_seconds: 5,
    logo_interval_seconds: 60,
    tabs: 2,
    baseline: "baseline.tar.gz",
    fleet_baseline: "fleet-baseline",
    seats: "fixture-validated/seats.csv",
    cold_etag: false,
    require_active: true,
    actions: [],
    panel_rates: {},
    ...raw,
  } as Experiment;
  const known = new Set<string>(["name"]);
  // Explicit schema keeps typoed experiment parameters from silently changing load.
  for (
    const key of [
      "hosts",
      "date",
      "version",
      "devices",
      "scenario",
      "steady_seconds",
      "warmup_seconds",
      "ramp_seconds",
      "sample_seconds",
      "logo_interval_seconds",
      "tabs",
      "baseline",
      "fleet_baseline",
      "seats",
      "cold_etag",
      "require_active",
      "actions",
      "panel_rates",
    ]
  ) known.add(key);
  for (const key of Object.keys(raw)) {
    if (!known.has(key)) throw new Error(`Unknown experiment key ${key}`);
  }
  const hosts = value.hosts;
  if (
    !hosts || typeof hosts !== "object" || Array.isArray(hosts) ||
    Object.keys(hosts).some((key) =>
      !["server", "loadgen", "server_url"].includes(key)
    )
  ) {
    throw new Error("Invalid hosts table");
  }
  for (const key of ["server", "loadgen"] as const) {
    if (
      typeof hosts[key] !== "string" ||
      !/^[A-Za-z_][A-Za-z0-9_.-]*@[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?$/
        .test(hosts[key])
    ) {
      throw new Error(`Invalid hosts.${key}: expected user@hostname`);
    }
  }
  if (hosts.server === hosts.loadgen) {
    throw new Error("Server and loadgen must differ");
  }
  let serverUrl: URL;
  try {
    serverUrl = new URL(hosts.server_url);
  } catch {
    throw new Error("Invalid hosts.server_url");
  }
  if (
    serverUrl.protocol !== "https:" || serverUrl.origin !== hosts.server_url
  ) {
    throw new Error(
      "Invalid hosts.server_url: expected an HTTPS origin without credentials or path",
    );
  }
  for (const key of ["name", "scenario", "version"] as const) {
    if (
      typeof value[key] !== "string" || !/^[A-Za-z0-9_.-]+$/.test(value[key])
    ) throw new Error(`Invalid ${key}`);
  }
  if (!/^[A-Za-z0-9][A-Za-z0-9.-]*$/.test(value.version)) {
    throw new Error("Invalid version");
  }
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value.date)) throw new Error("Invalid date");
  for (
    const key of [
      "devices",
      "steady_seconds",
      "warmup_seconds",
      "ramp_seconds",
      "sample_seconds",
      "logo_interval_seconds",
      "tabs",
    ] as const
  ) {
    if (!Number.isSafeInteger(value[key]) || value[key] < 0) {
      throw new Error(`Invalid ${key}`);
    }
  }
  if (
    !value.devices || value.devices > 100000 || !value.steady_seconds ||
    !value.sample_seconds || value.tabs > 1000
  ) throw new Error("Invalid experiment size");
  for (const key of ["baseline", "fleet_baseline", "seats"] as const) {
    if (
      typeof value[key] !== "string" ||
      !/^[A-Za-z0-9_./-]+$/.test(value[key]) || value[key].includes("..") ||
      value[key].startsWith("/")
    ) throw new Error(`Invalid ${key}`);
  }
  const routes = new Set([
    "/accounts",
    "/bindings",
    "/devices",
    "/seats",
    "/session",
    "/organizations",
    "/provisioning-window",
    "/enrollment-reviews",
  ]);
  for (const [route, rate] of Object.entries(value.panel_rates)) {
    if (
      !routes.has(route) || !Number.isFinite(rate) || rate < 0 ||
      (rate > 0 && (rate < 0.0001 || rate > 60000))
    ) throw new Error("Invalid panel rate");
  }
  for (const action of value.actions) {
    if (
      !Number.isSafeInteger(action.at) || action.at < 0 ||
      action.at >= value.steady_seconds ||
      !["restart", "export", "perf", "probe"].includes(action.kind)
    ) throw new Error("Invalid action");
    if (
      action.call_graph !== undefined &&
      !["fp", "dwarf"].includes(action.call_graph)
    ) throw new Error("Invalid perf call graph");
    if (
      action.kind === "perf" &&
      (!Number.isSafeInteger(action.seconds) || action.seconds! <= 0 ||
        action.at + action.seconds! > value.steady_seconds)
    ) throw new Error("Invalid perf duration");
  }
  if (
    typeof value.cold_etag !== "boolean" ||
    typeof value.require_active !== "boolean"
  ) throw new Error("Invalid boolean");
  return value;
}

export function quote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}
