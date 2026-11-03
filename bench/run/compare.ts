import { csvRows } from "../analyze/io.ts";
import { loadgenEvents, serverTimestamp, stats } from "./summary.ts";

/** Compare burst HTTP measurements on the Server clock with the field upgrades. */
export async function burst(base: string, field: string) {
  const w = JSON.parse(await Deno.readTextFile(`${base}/window.json`));
  const a = JSON.parse(
    await Deno.readTextFile(`${base}/analysis/summary.json`),
  );
  const reference = JSON.parse(await Deno.readTextFile(field));
  const restart = w.actions.find((x: { kind: string }) => x.kind === "restart");
  const process = restart
    ? a.processes.find((p: { start_ts_us: number }) =>
      p.start_ts_us >= restart.before_ts_us
    )
    : a.processes[0];
  const start = restart ? process.start_ts_us : w.steady_start_epoch * 1e6,
    end = start + 120e6;
  const reconnect: number[] = [],
    serverLogos: Record<string, number[]> = {},
    serverIntervals: number[][] = [],
    serverExport: number[] = [];
  let exportStart: number | null = null;
  for await (const row of csvRows(`${base}/analysis/requests.csv.gz`)) {
    const ts = +row.ts_us, duration = +row.duration_us;
    if (row.event !== "completed" || ts < start || ts >= end) continue;
    if (row.route === "/api/v2/device/control" && row.status === "101") {
      reconnect.push(ts - start);
    }
    if (row.route === "/api/v2/organizations/{organization_id}/logo") {
      (serverLogos[row.status] ??= []).push(duration);
      serverIntervals.push([ts - duration, ts]);
    }
    if (row.route === "/api/v2/exports/domjudge") {
      serverExport.push(duration);
      exportStart = ts - duration;
    }
  }
  const handshakes = new Map<string, number>(),
    clientLogos: Record<string, number[]> = {},
    clientIntervals: number[][] = [],
    clientExport: number[] = [];
  let applicationCount = 0;
  for await (const row of loadgenEvents(base)) {
    const ts = serverTimestamp(+row.ts_us, w), duration = +row.latency_us;
    if (
      row.event_kind === "handshake" &&
      ["RESUME", "ENROLLMENT"].includes(row.detail_code)
    ) {
      const phase = (handshakes.get(row.device) ?? 0) + 1;
      handshakes.set(row.device, phase);
      if (phase === (restart ? 2 : 1)) applicationCount++;
    }
    if (ts >= end) continue;
    if (
      row.event_kind === "logo" &&
      (handshakes.get(row.device) ?? 0) >= (restart ? 2 : 1)
    ) {
      (clientLogos[row.detail_code] ??= []).push(duration);
      clientIntervals.push([ts - duration, ts]);
    }
    if (row.event_kind === "operator_export" && ts >= start) {
      clientExport.push(duration);
    }
  }
  const curve = Array.from(
    { length: 120 },
    (_, second) => ({
      second,
      count: reconnect.filter((t) => Math.floor(t / 1e6) === second).length,
    }),
  );
  const latency = (rows: Record<string, number[]>) =>
    Object.fromEntries(Object.entries(rows).map(([k, v]) => [k, stats(v)]));
  const result = {
    start_ts_us: start,
    end_ts_us: end,
    full_120_seconds_observed: w.status === "completed" &&
      w.steady_end_epoch * 1e6 >= end,
    reconnect_us: stats(reconnect),
    application_handshake_count: applicationCount,
    server_logo_latency_us: latency(serverLogos),
    logo_latency_us: latency(clientLogos),
    peak_logo_in_flight: peak(clientIntervals),
    peak_server_logo_in_flight: peak(serverIntervals),
    export_duration_us: stats(clientExport),
    server_export_duration_us: stats(serverExport),
    server_export_start_ts_us: exportStart,
    export_after_restart_seconds: restart && exportStart !== null
      ? (exportStart - restart.before_ts_us) / 1e6
      : null,
    field: reference.reconnect_storms.filter((x: { process: string }) =>
      ["p05", "p06"].includes(x.process)
    ).map((
      x: { process: string; delay_us: unknown; peak_per_second: number },
    ) => ({
      process: x.process,
      delay_us: x.delay_us,
      peak_per_second: x.peak_per_second,
    })),
    curve,
  };
  await Deno.writeTextFile(
    `${base}/burst.json`,
    JSON.stringify(result, null, 2) + "\n",
  );
  await Deno.writeTextFile(
    `${base}/reconnect-120s.csv`,
    "second,count\n" + curve.map((r) => `${r.second},${r.count}\n`).join(""),
  );
  const summary =
    (await Deno.readTextFile(`${base}/summary.md`)).split("\nFirst 120 s")[0];
  const reconnectStats = result.reconnect_us;
  await Deno.writeTextFile(
    `${base}/summary.md`,
    summary +
      `\nFirst 120 s from ${
        new Date(start / 1000).toISOString()
      }: full window observed=${result.full_120_seconds_observed}; ${reconnectStats.count} HTTP control upgrades; p50/p99/max ${reconnectStats.p50}/${reconnectStats.p99}/${reconnectStats.max} us. Successful application handshakes in this phase: ${applicationCount}. Field upgrades: p05 p50/p99/max 2.659/4.921/5.285 s; p06 5.315/9.976/17.315 s. Peak simultaneous logo requests: client ${result.peak_logo_in_flight}, Server handler intervals ${result.peak_server_logo_in_flight}. Per-status latencies and the second-by-second curve are in burst.json and reconnect-120s.csv. Server statistics use A's timestamps; client statistics use recorded clock corrections.\n`,
  );
  console.log(JSON.stringify({ ...result, curve: undefined }));
}
function peak(intervals: number[][]) {
  const boundaries = intervals.flatMap(([a, b]) => [[a, 1], [b, -1]]).sort((
    a,
    b,
  ) => a[0] - b[0] || a[1] - b[1]);
  let current = 0, result = 0;
  for (const [, delta] of boundaries) {
    current += delta;
    result = Math.max(result, current);
  }
  return result;
}
if (import.meta.main) {
  await burst(
    Deno.args[0],
    Deno.args[1] ?? "context/bench/data/xian-2026-http/summary.json",
  );
}
