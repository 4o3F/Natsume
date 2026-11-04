import { csvRows, lines } from "../analyze/io.ts";

interface ClockPair {
  A: { offset_us: number; local_ts_us?: number };
  B: { offset_us: number; local_ts_us?: number };
}
export function serverTimestamp(
  raw: number,
  window: {
    clock_B_minus_A_us?: number;
    clock_start?: ClockPair;
    clock_end?: ClockPair;
  },
) {
  const start = window.clock_start, end = window.clock_end;
  if (start?.A.local_ts_us !== undefined && end?.A.local_ts_us !== undefined) {
    const a = start.A.local_ts_us + start.A.offset_us,
      b = end.A.local_ts_us + end.A.offset_us;
    const initial = start.B.offset_us - start.A.offset_us,
      final = end.B.offset_us - end.A.offset_us;
    if (b > a) {
      const slope = (final - initial) / (b - a);
      return (raw - initial + slope * a) / (1 + slope);
    }
  }
  return raw - Number(window.clock_B_minus_A_us ?? 0);
}

export async function* loadgenEvents(base: string) {
  yield* csvRows(`${base}/loadgen-events.csv.gz`);
  const extra = `${base}/export-events.csv.gz`;
  try {
    await Deno.stat(extra);
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) return;
    throw error;
  }
  yield* csvRows(extra);
}

export function stats(values: number[]) {
  values.sort((a, b) => a - b);
  const at = (p: number) =>
    values.length
      ? values[Math.max(0, Math.ceil(values.length * p) - 1)]
      : null;
  return {
    count: values.length,
    min: at(0),
    p50: at(.5),
    p95: at(.95),
    p99: at(.99),
    max: at(1),
    mean: values.length
      ? values.reduce((a, b) => a + b, 0) / values.length
      : null,
  };
}
export async function summarize(base: string) {
  const window = JSON.parse(await Deno.readTextFile(`${base}/window.json`));
  const start = window.steady_start_epoch * 1e6,
    end = window.steady_end_epoch * 1e6;
  const resources: Record<string, number>[] = [];
  const allResources: Record<string, number>[] = [];
  for await (const row of csvRows(`${base}/resources.csv`)) {
    const r = Object.fromEntries(
      Object.entries(row).filter(([, v]) => v !== "").map(([k, v]) => [k, +v]),
    );
    allResources.push(r);
    if (r.ts_us >= start && r.ts_us < end) resources.push(r);
  }
  if (
    window.status === "completed" &&
    resources.length <
      Math.max(
        1,
        Math.floor(
          window.experiment.steady_seconds / window.experiment.sample_seconds *
            .9,
        ),
      )
  ) throw new Error("Resource sample coverage below 90%");
  const resource: Record<string, ReturnType<typeof stats>> = {};
  for (
    const key of [
      "cpu_user_percent",
      "cpu_sys_percent",
      "rss_bytes",
      "cgroup_memory_current_bytes",
      "cgroup_memory_peak_bytes",
      "threads",
      "open_fds",
      "tcp_8443_established",
    ]
  ) resource[key] = stats(resources.filter((r) => key in r).map((r) => r[key]));
  resource.cpu_total_percent = stats(
    resources.filter((r) => "cpu_user_percent" in r).map((r) =>
      r.cpu_user_percent + r.cpu_sys_percent
    ),
  );
  let cpuSeconds = 0, cpuDuration = 0;
  for (let i = 1; i < allResources.length; i++) {
    const a = allResources[i - 1], b = allResources[i];
    const overlap = Math.max(
      0,
      Math.min(end, b.ts_us) - Math.max(start, a.ts_us),
    );
    if (overlap && a.pid === b.pid && a.pid > 0) {
      const fraction = overlap / (b.ts_us - a.ts_us);
      cpuSeconds +=
        ((b.user_ticks + b.sys_ticks) - (a.user_ticks + a.sys_ticks)) /
        b.ticks_per_second * fraction;
      cpuDuration += overlap / 1e6;
    }
  }
  if (cpuDuration) {
    resource.cpu_total_percent.mean = cpuSeconds / cpuDuration * 100;
  }
  const groups = new Map<string, number[]>(),
    errors: Record<string, number> = {},
    codes: Record<string, number> = {};
  const active = new Set<string>(),
    activeSeries: { ts_us: number; active: number }[] = [],
    handshakes: { device: string; ts_us: number }[] = [];
  const intervals: [number, number][] = [];
  let disconnects = 0, activeDisconnects = 0, reconnectSuccesses = 0;
  const everConnected = new Set<string>();
  for await (const r of csvRows(`${base}/loadgen-events.csv.gz`)) {
    const t = serverTimestamp(+r.ts_us, window),
      latency = +r.latency_us;
    if (
      r.event_kind === "handshake" &&
      ["RESUME", "ENROLLMENT"].includes(r.detail_code)
    ) {
      if (everConnected.has(r.device) && t >= start && t < end) {
        reconnectSuccesses++;
      }
      everConnected.add(r.device);
      active.add(r.device);
      handshakes.push({ device: r.device, ts_us: t });
      activeSeries.push({ ts_us: t, active: active.size });
    }
    if (r.event_kind === "disconnect") {
      const wasActive = active.delete(r.device);
      activeSeries.push({ ts_us: t, active: active.size });
      if (t >= start && t < end) {
        disconnects++;
        if (wasActive) activeDisconnects++;
      }
    }
    if (t < start || t >= end) continue;
    const key = `${r.event_kind}:${r.detail_code}`;
    codes[key] = (codes[key] ?? 0) + 1;
    if (
      [
        "TIMEOUT",
        "TCP_REFUSED",
        "TCP_RESET",
        "TLS_EOF",
        "HTTPS_TIMEOUT",
        "HTTPS_CONNECT",
        "HTTPS",
        "TRANSPORT",
        "TLS_HANDSHAKE",
      ].includes(r.detail_code)
    ) errors[key] = (errors[key] ?? 0) + 1;
    if (r.event_kind === "logo" || r.event_kind.startsWith("operator_")) {
      const group = groups.get(key) ?? [];
      group.push(latency);
      groups.set(key, group);
    }
    if (r.event_kind === "logo") intervals.push([t - latency, t]);
  }
  const activeSamples: number[] = [];
  let cursor = 0, count = 0;
  for (let t = start; t < end; t += window.experiment.sample_seconds * 1e6) {
    while (cursor < activeSeries.length && activeSeries[cursor].ts_us <= t) {
      count = activeSeries[cursor++].active;
    }
    activeSamples.push(count);
  }
  const boundaries = intervals.flatMap(([a, b]) => [[a, 1], [b, -1]]).sort((
    a,
    b,
  ) => a[0] - b[0] || a[1] - b[1]);
  let inFlight = 0, maxInFlight = 0;
  for (const [, d] of boundaries) {
    inFlight += d;
    maxInFlight = Math.max(maxInFlight, inFlight);
  }
  const manifest = JSON.parse(
    await Deno.readTextFile(`${base}/loadgen-run.json`),
  );
  const analysis = JSON.parse(
    await Deno.readTextFile(`${base}/analysis/summary.json`),
  );
  const result = {
    status: window.status,
    resource_samples: resources.length,
    cpu_sampled_seconds: cpuDuration,
    resource,
    active: stats(activeSamples),
    disconnects,
    active_disconnects: activeDisconnects,
    reconnect_successes: reconnectSuccesses,
    connection_failures: Object.entries(codes).filter(([key]) =>
      key.startsWith("tcp_tls:") && key !== "tcp_tls:OK"
    ).reduce((sum, [, count]) => sum + count, 0),
    connection_refused: codes["tcp_tls:TCP_REFUSED"] ?? 0,
    connection_timeouts: codes["tcp_tls:TIMEOUT"] ?? 0,
    errors,
    event_counts: codes,
    latency_us: Object.fromEntries([...groups].map(([k, v]) => [k, stats(v)])),
    max_logo_in_flight: maxInFlight,
    dropped_events: manifest.dropped_events,
    handshakes,
    active_series: activeSeries,
  };
  await Deno.writeTextFile(
    `${base}/metrics.json`,
    JSON.stringify(result, null, 2) + "\n",
  );
  await Deno.writeTextFile(
    `${base}/active.csv`,
    "ts_us,active\n" +
      activeSeries.map((r) => `${r.ts_us},${r.active}\n`).join(""),
  );
  const n = (v: unknown) => typeof v === "number" ? v.toFixed(3) : String(v);
  let markdown =
    `# ${window.experiment.name}\n\nServer ${window.experiment.version}; N=${window.experiment.devices}; scenario=${window.experiment.scenario}; status=${window.status}. Measurement ${window.steady_start} / ${window.steady_end}.\n\n| Metric | Value |\n| --- | ---: |\n| Active min / p50 / max | ${result.active.min} / ${result.active.p50} / ${result.active.max} |\n| Established TCP min / p50 / max | ${resource.tcp_8443_established.min} / ${resource.tcp_8443_established.p50} / ${resource.tcp_8443_established.max} |\n| CPU mean, one core = 100% | ${
      n(resource.cpu_total_percent.mean)
    }% |\n| RSS max MiB | ${
      n((resource.rss_bytes.max ?? 0) / 1048576)
    } |\n| Disconnects | ${disconnects} |\n| Transport errors | ${
      Object.values(errors).reduce((a, b) => a + b, 0)
    } |\n| Dropped events | ${manifest.dropped_events} |\n| Max concurrent logo requests, client intervals | ${maxInFlight} |\n\n| Loadgen event:status | Count | p50 ms | p95 ms | p99 ms | max ms |\n| --- | ---: | ---: | ---: | ---: | ---: |\n`;
  for (const [key, v] of Object.entries(result.latency_us)) {
    markdown += `| ${key} | ${v.count} | ${n((v.p50 ?? 0) / 1000)} | ${
      n((v.p95 ?? 0) / 1000)
    } | ${n((v.p99 ?? 0) / 1000)} | ${n((v.max ?? 0) / 1000)} |\n`;
  }
  markdown +=
    "\nServer handler timings are in analysis/summary.json; loadgen timings include transport and complete response reads. CPU values use process tick deltas for intervals with the same PID. Active counts track successful application handshakes and disconnect events. Request concurrency reconstructs client start/end intervals; image_work waiting is inferred from handler latency and concurrency.\n\nTransport errors: `" +
    JSON.stringify(errors) +
    "`.\n\nRaw data: raw/natsume-bench.jsonl, resources.csv, loadgen-events.csv.gz, loadgen-run.json, loadgen.log. Analysis commands: bench/analyze/journald.ts extract, summarize (--window steady), series (--series-range); parameters are in experiment.toml and window.json.\n";
  const rawCounts = { completed: 0, rejected: 0 };
  for await (const line of lines(`${base}/raw/natsume-bench.jsonl`)) {
    const r = JSON.parse(line);
    const message = Array.isArray(r.MESSAGE)
      ? new TextDecoder().decode(new Uint8Array(r.MESSAGE))
      : String(r.MESSAGE ?? "");
    if (message.includes("HTTP request completed")) rawCounts.completed++;
    if (message.includes("HTTP request rejected")) rawCounts.rejected++;
  }
  const extracted = { completed: 0, rejected: 0 };
  for await (const row of csvRows(`${base}/analysis/requests.csv.gz`)) {
    if (row.event === "completed") extracted.completed++;
    if (row.event === "rejected") extracted.rejected++;
  }
  if (JSON.stringify(rawCounts) !== JSON.stringify(extracted)) {
    throw new Error("Journal extraction count mismatch");
  }
  await Deno.writeTextFile(
    `${base}/analysis-completeness.json`,
    JSON.stringify(
      { rawCounts, extracted, windows: analysis.named_windows },
      null,
      2,
    ) + "\n",
  );
  await Deno.writeTextFile(`${base}/summary.md`, markdown);
}
if (import.meta.main) await summarize(Deno.args[0]);
