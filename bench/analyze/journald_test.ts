import { join } from "@std/path";
import { check } from "./check_anonymized.ts";
import { csvRows } from "./io.ts";
import {
  consumed,
  CONTROL,
  extract,
  fields,
  inWindow,
  JournalRecord,
  LOGO,
  message,
  parseOptions,
  parseRequest,
  percentile,
  readProcesses,
  readRequests,
  seconds,
  series,
  summarize,
} from "./journald.ts";

function assert(
  condition: unknown,
  message = "Assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}
function equal(actual: unknown, expected: unknown) {
  assert(JSON.stringify(actual) === JSON.stringify(expected), "Values differ");
}
function throws(fn: () => unknown) {
  let failed = false;
  try {
    fn();
  } catch {
    failed = true;
  }
  assert(failed, "Expected rejection");
}
const START = "2030-01-02T09:00:00+08:00";
const END = "2030-01-02T14:00:00+08:00";
const INTERVAL = `${START}/${END}`;
const OPTIONS = parseOptions([
  "summarize",
  "synthetic",
  "--out",
  "synthetic.json",
  "--window",
  `contest=${INTERVAL}`,
  "--analysis-end",
  "2030-01-02T09:00:00.900000+08:00",
]);
const BASE = OPTIONS.windows.contest[0];
const UUID = "11111111-2222-4333-8444-555555555555";
const BOOT = "0123456789abcdef0123456789abcdef";
function request(actor = UUID, status = 304, duration = 833, route = LOGO) {
  const actorFields = actor ? ` actor_kind="operator" actor_id="${actor}"` : "";
  return ` INFO http_request{http.request.method="GET" http.route="${route}" http.response.status_code=${status}${actorFields}${actorFields}}: HTTP request completed method=GET route=${route} status=${status} duration_us=${duration}`;
}
async function fixture(
  run: (raw: string, dataset: string, base: number) => Promise<void>,
) {
  await Deno.mkdir("target/bench-analyze-tests", { recursive: true });
  const root = await Deno.makeTempDir({ dir: "target/bench-analyze-tests" });
  try {
    await run(
      join(root, "fixture.jsonl"),
      join(root, "dataset"),
      BASE,
    );
  } finally {
    await Deno.remove(root, { recursive: true });
  }
}
async function populate(raw: string, dataset: string, base: number) {
  function row(
    offset: number,
    text: string | number[],
    pid = "100",
    ident = "natsume-server",
    boot = BOOT,
  ): JournalRecord {
    return {
      __REALTIME_TIMESTAMP: String(base + offset),
      _PID: pid,
      _BOOT_ID: boot,
      _HOSTNAME: "fake-host.example.invalid",
      _MACHINE_ID: "9".repeat(32),
      _STREAM_ID: "8".repeat(32),
      _SYSTEMD_INVOCATION_ID: "7".repeat(32),
      INVOCATION_ID: "6".repeat(32),
      __SEQNUM_ID: "5".repeat(32),
      __CURSOR: "s=abcdef0123456789;i=42;b=" + boot,
      SYSLOG_IDENTIFIER: ident,
      MESSAGE: text,
    };
  }
  const records = [
    row(-1000000, "Started example.service.", "1", "systemd"),
    row(0, ' INFO server_command{mode="server"}: server mode running'),
    row(100000, " INFO listener bound listen_address=192.0.2.42:8443"),
    row(200000, request()),
    row(300000, [
      ...new TextEncoder().encode(request(UUID, 101, 833, CONTROL)),
    ]),
    row(
      400000,
      ' WARN HTTP request rejected method=GET route=/api/v2/session code="AUTHENTICATION_FAILED" cause="missing_session_cookie"',
    ),
    row(
      500000,
      ' WARN School logo could not be read organization_id=45 path=/var/lib/fake/虚构学校.webp error="missing"',
    ),
    row(600000, " INFO graceful shutdown completed"),
    row(700000, "Stopped example.service.", "1", "systemd"),
    row(
      800000,
      "Consumed 7h 10min 1.293s CPU time over 2min 18.034s wall clock time, 696.2M memory peak.",
      "1",
      "systemd",
    ),
    row(900000, "Started example.service.", "1", "systemd"),
    row(1000000, request("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"), "200"),
    row(1100000, request(), "200"),
  ];
  await Deno.writeTextFile(
    raw,
    records.map((r) => JSON.stringify(r) + "\n").join(""),
  );
  await extract(raw, dataset);
}
Deno.test("parser, duplicated fields, byte MESSAGE and rejected unsafe fields", () => {
  const parsed = parseRequest(request())!;
  equal(parsed.actor_id, UUID);
  equal(parsed.duration_us, "833");
  equal(
    message({ MESSAGE: [...new TextEncoder().encode(request())] }),
    request(),
  );
  equal(fields('actor_id="a" actor_id="a"').actor_id, "a");
  throws(() => fields('actor_id="a" actor_id="b"'));
  throws(() => parseRequest(request("", 200, 1, "/api/v2/devices/123")));
  throws(() => parseRequest(request("", 600)));
  throws(() => parseRequest(" INFO HTTP request completed method=GET route=/"));
  throws(() =>
    parseRequest(request().replace("duration_us=833", "duration_us=-1"))
  );
  throws(() => parseRequest(request().replace("method=GET", "method=UNSAFE")));
  equal(
    parseRequest(
      ' WARN HTTP request rejected code="free text" cause="missing_cookie"',
    )!.code,
    "",
  );
});
Deno.test("stable aliases, lifecycle, summaries, series and clean leak scan", () =>
  fixture(async (raw, dataset, base) => {
    await populate(raw, dataset, base);
    const rows = await Array.fromAsync(readRequests(dataset));
    equal(rows.map((r) => r.actor), [
      "operator-1",
      "operator-1",
      "",
      "operator-2",
      "operator-1",
    ]);
    equal(rows.map((r) => r.process), ["p01", "p01", "p01", "p02", "p02"]);
    const processes = await readProcesses(dataset);
    equal(processes.map((p) => p.boot), ["b1", "b1"]);
    equal(processes[0].start_ts_us, String(base - 1000000));
    equal(processes[0].end_reason, "graceful");
    equal(Number(processes[0].cpu_seconds), 25801.293);
    await summarize(dataset, join(dataset, "summary.json"), OPTIONS);
    await series(dataset, join(dataset, "series"), OPTIONS.windows.contest);
    const summary = JSON.parse(
      await Deno.readTextFile(join(dataset, "summary.json")),
    );
    equal(summary.named_windows, { contest: { start: START, end: END } });
    equal(summary.analysis_cutoff, "2030-01-02T09:00:00.900000+08:00");
    equal(
      summary.processes.map((p: { before_analysis_cutoff: boolean }) =>
        p.before_analysis_cutoff
      ),
      [true, false],
    );
    equal(
      summary.reconnect_storms.map((p: { before_analysis_cutoff: boolean }) =>
        p.before_analysis_cutoff
      ),
      [true, false],
    );
    equal(summary.windows.contest.events, { completed: 4, rejected: 1 });
    equal(summary.windows.contest.logo_requests_per_minute.count, 300);
    equal(summary.windows.contest.logo_requests_per_minute.p50, 0);
    equal(summary.reconnect_storms[0].delay_us.p50, 1300000);
    equal(summary.reconnect_storms[0].counts_per_second[1], 1);
    await summarize(dataset, join(dataset, "no-cutoff.json"), {
      windows: {},
      analysisEnd: null,
    });
    const unbounded = JSON.parse(
      await Deno.readTextFile(join(dataset, "no-cutoff.json")),
    );
    equal(unbounded.named_windows, {});
    equal(unbounded.analysis_cutoff, null);
    equal(Object.keys(unbounded.windows), ["all", "p01", "p02"]);
    for (const p of [...unbounded.processes, ...unbounded.reconnect_storms]) {
      equal(p.before_analysis_cutoff, true);
    }
    equal(unbounded.windows.all.events, summary.windows.all.events);
    await series(dataset, join(dataset, "limited-series"), [
      base + 200000,
      base + 300000,
    ]);
    const minuteRows = await Array.fromAsync(
      csvRows(join(dataset, "limited-series/minute_routes.csv")),
    );
    equal(minuteRows.map((r) => [r.route, r.count]), [[LOGO, "1"]]);
    equal(await check(raw, dataset), {});
    const notes = await Array.fromAsync(csvRows(join(dataset, "notes.csv")));
    equal(notes[1].port, "8443");
    equal(notes[2].organization, "org-01");
    const reconnect = await Array.fromAsync(
      csvRows(join(dataset, "series/reconnect.csv")),
    );
    equal(reconnect.length, 1200);
    equal(reconnect[1].count, "1");
    const cdf = await Array.fromAsync(
      csvRows(join(dataset, "series/latency_cdf.csv")),
    );
    equal(cdf.length, 101);
    equal(cdf[0].quantile, "0.0");
    equal(cdf[100].quantile, "1.0");
  }));
Deno.test("PID reuse across boots and missing stops", () =>
  fixture(async (raw, dataset, base) => {
    const records = [[BOOT, "1"], [BOOT, "2"], [
      "abcdefabcdefabcdefabcdefabcdefab",
      "2",
    ]].map(([boot, pid], i) => ({
      __REALTIME_TIMESTAMP: String(base + i),
      _BOOT_ID: boot,
      _PID: pid,
      SYSLOG_IDENTIFIER: "natsume-server",
      MESSAGE: request(""),
    }));
    await Deno.writeTextFile(
      raw,
      records.map((r) => JSON.stringify(r)).join("\n"),
    );
    await extract(raw, dataset);
    const processes = await readProcesses(dataset);
    equal(processes.map((p) => p.boot), ["b1", "b1", "b2"]);
    equal(processes.map((p) => p.end_reason), [
      "replaced_without_stop",
      "boot_changed",
      "log_end",
    ]);
    assert(processes.every((p) => p.stop_ts_us === ""));
    await Deno.writeTextFile(
      raw,
      records.toReversed().map((r) => JSON.stringify(r)).join("\n"),
    );
    let failed = false;
    try {
      await extract(raw, dataset);
    } catch {
      failed = true;
    }
    assert(failed, "Decreasing timestamps must abort");
  }));
Deno.test("systemd Consumed units, optional fields and rounding", () => {
  const data = consumed(
    "Consumed 7h 10min 1.293s CPU time over 2min 18.034s wall clock time, 696.2M memory peak.",
  );
  equal(data.cpu_seconds, 25801.293);
  equal(data.wall_seconds, 138.034);
  equal(data.memory_peak_bytes, Math.round(696.2 * 1024 ** 2));
  equal(seconds("1w 1d 13min 6.521s"), 691986.521);
  equal(seconds("1ms 2us"), .001002);
  equal(consumed("Consumed 15.571s CPU time.").wall_seconds, "");
  equal(
    consumed("Consumed 1s CPU time, 2.5B memory peak.").memory_peak_bytes,
    2,
  );
  throws(() => seconds("1fortnight"));
  throws(() => consumed("Consumed unsupported"));
});
Deno.test("half-open windows and nearest-rank percentiles", () => {
  for (const bounds of Object.values(OPTIONS.windows)) {
    assert(!inWindow(bounds[0] - 1, bounds));
    assert(inWindow(bounds[0], bounds));
    assert(inWindow(bounds[1] - 1, bounds));
    assert(!inWindow(bounds[1], bounds));
  }
  assert(!inWindow(OPTIONS.analysisEnd!, [0, OPTIONS.analysisEnd!]));
  equal(percentile([], 50), null);
  equal(percentile([1, 2, 3, 4], 50), 2);
  equal(percentile([1, 2, 3, 4], 99), 4);
  equal(percentile([1], 1), 1);
});
Deno.test("planted leaks, escaped text, gzip and safe CLI failure", () =>
  fixture(async (raw, dataset, base) => {
    await populate(raw, dataset, base);
    const leak =
      `${UUID}\nfake-host.example.invalid\norganization_id=45\n/var/lib/fake/虚构学校.webp\n192.0.2.42\n{"school":"\\u865a\\u6784\\u5b66\\u6821"}\n${BOOT}\ns=abcdef0123456789;i=42\n` +
      [..."98765"].map((digit) => digit.repeat(32) + "\n").join("");
    const file = await Deno.open(join(dataset, "planted.csv.gz"), {
      write: true,
      create: true,
    });
    await new Blob([leak]).stream().pipeThrough(new CompressionStream("gzip"))
      .pipeTo(file.writable);
    const hits = await check(raw, dataset);
    for (
      const kind of [
        "actor_id",
        "_HOSTNAME",
        "organization_id",
        "filesystem_path",
        "non_ascii",
        "ip_address",
        "_BOOT_ID",
        "_MACHINE_ID",
        "_STREAM_ID",
        "_SYSTEMD_INVOCATION_ID",
        "INVOCATION_ID",
        "__SEQNUM_ID",
        "__CURSOR",
      ]
    ) assert(hits[kind] > 0, `Missing leak category: ${kind}`);
    const output = await new Deno.Command(Deno.execPath(), {
      args: [
        "run",
        "--frozen",
        "--cached-only",
        "--no-prompt",
        `--allow-read=${raw},${dataset}`,
        "bench/analyze/check_anonymized.ts",
        raw,
        dataset,
      ],
      stdout: "piped",
      stderr: "piped",
    }).output();
    equal(output.code, 1);
    const text = new TextDecoder().decode(output.stdout) +
      new TextDecoder().decode(output.stderr);
    assert(
      !text.includes(UUID) && !text.includes("fake-host.example.invalid") &&
        !text.includes(BOOT),
    );
  }));

Deno.test("analysis options: explicit windows, offsets, microseconds and extract independence", () => {
  const parsed = parseOptions([
    "series",
    "synthetic",
    "--window",
    `morning=${INTERVAL}`,
    "--out",
    "charts",
    "--window",
    "afternoon=2030-01-02T14:00+08:00/2030-01-02T15:00+08:00",
    "--analysis-end",
    "2030-01-02T06:00:00.000001Z",
    "--series-range",
    INTERVAL,
  ]);
  equal(Object.keys(parsed.windows), ["morning", "afternoon"]);
  equal(parsed.windows.morning, OPTIONS.windows.contest);
  equal(parsed.seriesRange, OPTIONS.windows.contest);
  equal(parsed.analysisEnd, parsed.windows.morning[1] + 1);
  equal(
    parseOptions(["summarize", "synthetic", "--out", "summary"]).analysisEnd,
    null,
  );
  equal(
    parseOptions(["extract", "synthetic", "--out", "dataset"]).command,
    "extract",
  );
  for (
    const [flag, value] of [
      ["--window", `morning=${INTERVAL}`],
      ["--analysis-end", START],
      ["--series-range", INTERVAL],
    ]
  ) {
    throws(() =>
      parseOptions(["extract", "synthetic", "--out", "dataset", flag, value])
    );
  }
});
Deno.test("analysis options reject reversed, empty or malformed intervals and timestamps", () => {
  for (
    const interval of [
      `${END}/${START}`,
      `${START}/${START}`,
      START,
      `${INTERVAL}/${END}`,
    ]
  ) {
    throws(() =>
      parseOptions([
        "summarize",
        "synthetic",
        "--out",
        "summary",
        "--window",
        `bad=${interval}`,
      ])
    );
    throws(() =>
      parseOptions([
        "series",
        "synthetic",
        "--out",
        "charts",
        "--series-range",
        interval,
      ])
    );
  }
  for (
    const value of [
      "garbage",
      "2030-01-02",
      "2030-01-02T09:00:00",
      "2030-02-30T09:00:00Z",
      "2030-13-02T09:00:00Z",
      "2030-01-02T24:00:00Z",
      "2030-01-02T09:60:00Z",
      "2030-01-02T09:00:60Z",
      "2030-01-02T09:00:00+24:00",
      "2030-01-02T09:00:00+08:60",
    ]
  ) {
    throws(() =>
      parseOptions([
        "summarize",
        "synthetic",
        "--out",
        "summary",
        "--analysis-end",
        value,
      ])
    );
    throws(() =>
      parseOptions([
        "summarize",
        "synthetic",
        "--out",
        "summary",
        "--window",
        `bad=${value}/${END}`,
      ])
    );
  }
});
Deno.test("analysis options reject duplicate or reserved window names and missing options", () => {
  for (const name of ["all", "p01", "p10", "p100"]) {
    throws(() =>
      parseOptions([
        "summarize",
        "synthetic",
        "--out",
        "summary",
        "--window",
        `${name}=${INTERVAL}`,
      ])
    );
  }
  throws(() =>
    parseOptions([
      "summarize",
      "synthetic",
      "--out",
      "summary",
      "--window",
      `a=${INTERVAL}`,
      "--window",
      `a=${INTERVAL}`,
    ])
  );
  for (
    const flags of [
      ["--analysis-end"],
      ["--unknown", START],
      ["--out", "again"],
      ["--window", INTERVAL],
      ["--window", `= ${INTERVAL}`],
      ["--analysis-end", START, "--analysis-end", END],
    ]
  ) {
    throws(() =>
      parseOptions(["summarize", "synthetic", "--out", "summary", ...flags])
    );
  }
  throws(() => parseOptions(["summarize", "synthetic"]));
  throws(() => parseOptions(["series", "synthetic", "--out", "charts"]));
});
Deno.test("invalid analysis CLI options fail clearly before opening the dataset", async () => {
  for (
    const [command, flags, expected] of [
      [
        "summarize",
        ["--window", `bad=${END}/${START}`],
        "start must be before end",
      ],
      ["summarize", ["--analysis-end", "garbage"], "Invalid timestamp"],
      ["summarize", ["--window", `all=${INTERVAL}`], "Reserved window name"],
      [
        "summarize",
        ["--window", `a=${INTERVAL}`, "--window", `a=${INTERVAL}`],
        "Duplicate window name",
      ],
      ["series", [], "--series-range"],
    ] as const
  ) {
    const output = await new Deno.Command(Deno.execPath(), {
      args: [
        "run",
        "--frozen",
        "--cached-only",
        "--no-prompt",
        "bench/analyze/journald.ts",
        command,
        "synthetic",
        "--out",
        "synthetic-output",
        ...flags,
      ],
      stdout: "piped",
      stderr: "piped",
    }).output();
    equal(output.code, 1);
    assert(new TextDecoder().decode(output.stderr).includes(expected));
  }
});
