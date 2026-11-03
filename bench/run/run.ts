import { restartScript } from "./actions.ts";
import { configuration, type Experiment, quote as q } from "./config.ts";
import { summarize } from "./summary.ts";

export const sshOptions = [
  "-o",
  "BatchMode=yes",
  "-o",
  "StrictHostKeyChecking=yes",
  "-o",
  "UpdateHostKeys=no",
  "-o",
  "ForwardAgent=no",
  "-o",
  "ConnectTimeout=10",
];
export const input = "/home/icpc/natsume-bench/B0";
const encoder = new TextEncoder(), decoder = new TextDecoder();

export async function command(
  program: string,
  args: string[],
  stdin?: string,
): Promise<string> {
  const child = new Deno.Command(program, {
    args,
    stdin: stdin === undefined ? "null" : "piped",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  if (stdin !== undefined) {
    const writer = child.stdin.getWriter();
    await writer.write(encoder.encode(stdin));
    await writer.close();
  }
  const result = await child.output();
  if (!result.success) {
    throw new Error(
      `${program} exited ${result.code}: ${
        decoder.decode(result.stderr).slice(-2000)
      }`,
    );
  }
  return decoder.decode(result.stdout).trim();
}
export function remote(host: string, script: string) {
  return command(
    "ssh",
    [...sshOptions, host, "bash -se"],
    `set -euo pipefail\n${script}`,
  );
}
async function collect(host: string, script: string, path: string) {
  const child = new Deno.Command("ssh", {
    args: [...sshOptions, host, script],
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  const file = await Deno.open(path, { write: true, createNew: true });
  const [, status, stderr] = await Promise.all([
    child.stdout.pipeTo(file.writable),
    child.status,
    child.stderr.text(),
  ]);
  if (!status.success) throw new Error(`Collection failed: ${stderr}`);
}
export async function clockOffset(host: string) {
  const child = new Deno.Command("ssh", {
    args: [
      ...sshOptions,
      host,
      'printf "ready\\n"; while read -r tick; do date +%s%6N; done',
    ],
    stdin: "piped",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  const errors = child.stderr.text(),
    writer = child.stdin.getWriter(),
    reader = child.stdout.pipeThrough(new TextDecoderStream()).getReader();
  let pending = "";
  const line = async () => {
    while (!pending.includes("\n")) {
      const next = await reader.read();
      if (next.done) throw new Error("Clock probe ended");
      pending += next.value;
    }
    const index = pending.indexOf("\n"), value = pending.slice(0, index);
    pending = pending.slice(index + 1);
    return value;
  };
  const samples = [];
  let clockExit = 0, clockError = "";
  try {
    if (await line() !== "ready") throw new Error("Invalid clock probe");
    for (let i = 0; i < 5; i++) {
      const before = (performance.timeOrigin + performance.now()) * 1000;
      await writer.write(encoder.encode("tick\n"));
      const stamp = Number(await line());
      const after = (performance.timeOrigin + performance.now()) * 1000;
      samples.push({
        offset_us: stamp - (before + after) / 2,
        rtt_us: after - before,
        local_ts_us: (before + after) / 2,
      });
    }
  } finally {
    await writer.close();
    await reader.cancel();
    clockExit = (await child.status).code;
    clockError = await errors;
  }
  if (clockExit) throw new Error(`Clock probe failed: ${clockError}`);
  return samples.sort((a, b) => a.rtt_us - b.rtt_us)[0];
}
const delay = (ms: number) =>
  new Promise<void>((resolve) => setTimeout(resolve, Math.max(0, ms)));
const iso = (epoch: number) => new Date(epoch * 1000).toISOString();
export function scenario(e: Experiment, total: number, exportAt?: number) {
  return `devices = ${e.devices}\noperators = ${e.tabs}\nramp = "${e.ramp_seconds}s"\nduration = "${total}s"\nlogo_interval = "${e.logo_interval_seconds}s"\n` +
    (exportAt === undefined
      ? ""
      : `[[events]]\nat = "${exportAt}s"\naction = "export"\n`) +
    `[panel_rates]\n${
      Object.entries(e.panel_rates).map(([route, rate]) =>
        `${JSON.stringify(route)} = ${rate}`
      ).join("\n")
    }\n`;
}

export async function run(path: string) {
  const encoded = await Deno.readTextFile(path), e = configuration(encoded);
  const { server: A, loadgen: B, server_url: serverUrl } = e.hosts;
  const port = new URL(serverUrl).port || "443";
  const out = `context/bench/data/results/${e.date}-${e.name}`;
  await Deno.mkdir(out);
  await Deno.mkdir(`${out}/raw`);
  await Deno.writeTextFile(
    `${out}/summary.md`,
    `# ${e.name}\n\nExperiment starting.\n`,
  );
  await Deno.writeTextFile(`${out}/experiment.toml`, encoded);
  const base = `/srv/natsume-bench/${e.version}`,
    work = `${input}/experiments/${e.name}`;
  const state: Record<string, unknown> = {
    status: "starting",
    experiment: e,
    actions: [],
  };
  const save = () =>
    Deno.writeTextFile(
      `${out}/window.json`,
      JSON.stringify(state, null, 2) + "\n",
    );
  const stop = async () => {
    const results = await Promise.allSettled([
      remote(
        B,
        'for unit in natsume-experiment-loadgen natsume-experiment-probe natsume-experiment-export; do if test "$(systemctl show "$unit" -p LoadState --value)" = loaded; then sudo -n systemctl stop "$unit"; fi; done',
      ),
      remote(
        A,
        'for unit in natsume-experiment-actions natsume-bench natsume-experiment-sampler natsume-experiment-perf; do if test "$(systemctl show "$unit" -p LoadState --value)" = loaded; then sudo -n systemctl stop "$unit"; fi; done',
      ),
    ]);
    return results.filter((r) => r.status === "rejected").map((r) =>
      String((r as PromiseRejectedResult).reason)
    );
  };
  let interrupted = false;
  const interrupt = () => {
    interrupted = true;
  };
  Deno.addSignalListener("SIGINT", interrupt);
  Deno.addSignalListener("SIGTERM", interrupt);
  try {
    const clocks = await Promise.all([clockOffset(A), clockOffset(B)]);
    state.clock_start = { A: clocks[0], B: clocks[1] };
    state.clock_B_minus_A_us = clocks[1].offset_us - clocks[0].offset_us;
    await save();
    await remote(
      A,
      `sudo -n test ! -e ${base}/runs/${e.name}\nsystemctl show natsume-bench -p ExecStart --value | grep -F /opt/natsume-bench/${e.version}/`,
    );
    const fingerprint = await remote(
      A,
      `test "$(systemctl is-enabled natsume-server)" = disabled\ntest "$(systemctl show natsume-server -p MainPID --value)" = 0\nsha256sum /opt/natsume-bench/${e.version}/natsume-server\ndate -u +%FT%TZ`,
    );
    await Deno.writeTextFile(
      `${out}/server-fingerprint.txt`,
      fingerprint + "\n",
    );
    await Deno.writeTextFile(
      `${out}/loadgen-fingerprint.txt`,
      await remote(B, `sha256sum ${input}/natsume-loadgen`) + "\n",
    );
    await remote(
      B,
      `test ! -e ${q(work)}\nmkdir -m 700 -p ${q(work)}\ncp -a ${
        q(`${input}/${e.fleet_baseline}`)
      } ${q(`${work}/fleet`)}\n${
        e.cold_etag
          ? `find ${
            q(`${work}/fleet`)
          } -maxdepth 1 -name 'logos-*.json' -delete`
          : ":"
      }`,
    );
    state.export_since_epoch = Number(await remote(A, "date +%s"));
    await remote(
      A,
      `sudo -n bash -se -- ${q(e.version)} ${
        q(e.baseline)
      } <<'RESTORE'\n${await Deno.readTextFile(
        "bench/run/restore.sh",
      )}\nRESTORE`,
    );
    await Deno.writeTextFile(
      `${out}/running-server-fingerprint.txt`,
      await remote(
        A,
        `sudo -n sha256sum /proc/"$(systemctl show natsume-bench -p MainPID --value)"/exe\nsystemctl show natsume-bench -p ExecStart`,
      ) + "\n",
    );
    await remote(
      A,
      `sudo -n install -d -m 700 ${q(`${base}/runs/${e.name}`)}\nsudo -n tee ${
        q(`${base}/runs/${e.name}/sample.sh`)
      } >/dev/null <<'SAMPLER'\n${await Deno.readTextFile(
        "bench/run/sample.sh",
      )}\nSAMPLER\nsudo -n systemd-run --collect --unit=natsume-experiment-sampler -p StandardError=journal -p StandardOutput=append:${base}/runs/${e.name}/resources.csv /bin/bash ${base}/runs/${e.name}/sample.sh ${e.sample_seconds} ${port}`,
    );
    // Actions use a fixed epoch after ramp and warmup, shared by both hosts.
    const loadStart = Number(await remote(A, "date +%s")) + 15;
    const loadStartB = Math.round(
      loadStart * 1e6 + Number(state.clock_B_minus_A_us),
    );
    const start = loadStart + e.ramp_seconds + e.warmup_seconds,
      end = start + e.steady_seconds;
    state.steady_start_epoch = start;
    state.steady_end_epoch = end;
    state.steady_start = iso(start);
    state.steady_end = iso(end);
    const exports = e.actions.filter((a) => a.kind === "export");
    if (exports.length > 1) {
      throw new Error("One export per experiment is supported");
    }
    await remote(
      B,
      `cat > ${q(`${work}/scenario.toml`)} <<'SCENARIO'\n${
        scenario(e, e.warmup_seconds + e.steady_seconds + 30)
      }SCENARIO\ncat > ${
        q(`${work}/launch.sh`)
      } <<'LAUNCH'\n#!/bin/bash\nset -euo pipefail\nexec /usr/bin/numactl --cpunodebind=0 --membind=0 ${input}/natsume-loadgen run --server ${
        q(serverUrl)
      } --trust-root ${input}/control-ca.crt --state-dir ${work}/fleet --seat-plan ${input}/${e.seats} --output-dir ${work}/run --seed 418 --start-at-unix-us ${loadStartB} --arrival uniform --scenario ${work}/scenario.toml --operator-login synthetic-admin --operator-password-file ${input}/operator-password\nLAUNCH\nsudo -n systemd-run --collect --unit=natsume-experiment-loadgen -p User=icpc -p LimitNOFILE=65536 -p KillSignal=SIGINT -p TimeoutStopSec=30 -p StandardOutput=append:${work}/loadgen.log -p StandardError=inherit /bin/bash ${work}/launch.sh`,
    );
    if (e.actions.some((a) => a.kind === "restart")) {
      await remote(
        A,
        `sudo -n tee ${base}/runs/${e.name}/actions.sh >/dev/null <<'ACTIONS'\n${
          restartScript(e.actions, start)
        }\nACTIONS\nsudo -n systemd-run --collect --unit=natsume-experiment-actions -p StandardError=journal -p StandardOutput=append:${base}/runs/${e.name}/actions.csv /bin/bash ${base}/runs/${e.name}/actions.sh`,
      );
    }
    state.status = "running";
    await save();
    console.log(`${e.name}: measurement ${iso(start)} / ${iso(end)}`);
    const due = (action: Experiment["actions"][number]) =>
      start + action.at - (action.kind === "export" ? 10 : 0);
    const pending = e.actions.filter((a) => a.kind !== "restart").sort((a, b) =>
      due(a) - due(b)
    );
    let activeChecked = false;
    while (true) {
      if (interrupted) throw new Error("Interrupted");
      const now = Number(await remote(A, "date +%s"));
      if (!activeChecked && now >= start - 10) {
        if (e.require_active) {
          const log = await remote(B, `tail -n 3 ${q(`${work}/loadgen.log`)}`);
          if (!log.includes(`connected=${e.devices} devices=${e.devices}`)) {
            throw new Error(
              `Fleet not fully Active before measurement: ${log}`,
            );
          }
        }
        await remote(
          A,
          "systemctl is-active --quiet natsume-experiment-sampler",
        );
        activeChecked = true;
      }
      while (pending.length && now >= due(pending[0])) {
        const action = pending.shift()!,
          before = Number(await remote(A, "date +%s%6N"));
        if (action.kind === "export") {
          const [a, b] = await Promise.all([clockOffset(A), clockOffset(B)]);
          const atB = Math.round(
            (start + action.at) * 1e6 + b.offset_us - a.offset_us,
          );
          await remote(
            B,
            `sudo -n systemd-run --collect --unit=natsume-experiment-export -p User=icpc -p StandardOutput=append:${work}/export.log /usr/bin/numactl --cpunodebind=0 --membind=0 ${input}/natsume-loadgen export --server ${
              q(serverUrl)
            } --trust-root ${input}/control-ca.crt --operator-login synthetic-admin --operator-password-file ${input}/operator-password --output-dir ${work}/export --at-unix-us ${atB}`,
          );
        }
        if (action.kind === "perf") {
          await remote(
            A,
            `sudo -n systemd-run --collect --unit=natsume-experiment-perf /usr/bin/perf record -F 99 --call-graph ${
              action.call_graph === "dwarf" ? "dwarf,16384" : "fp"
            } -p "$(systemctl show natsume-bench -p MainPID --value)" -o ${base}/runs/${e.name}/perf.data -- sleep ${action.seconds}`,
          );
        }
        if (action.kind === "probe") {
          await remote(
            B,
            `sudo -n systemd-run --collect --unit=natsume-experiment-probe -p User=icpc -p StandardOutput=append:${work}/probe.log /usr/bin/numactl --cpunodebind=0 --membind=0 ${input}/natsume-loadgen probe --server ${
              q(serverUrl)
            } --trust-root ${input}/control-ca.crt --operator-login synthetic-admin --operator-password-file ${input}/operator-password --output-dir ${work}/probe`,
          );
        }
        (state.actions as unknown[]).push({
          ...action,
          before_ts_us: before,
          after_ts_us: Number(await remote(A, "date +%s%6N")),
        });
        await save();
      }
      if (now >= end) break;
      await delay(Math.min(1000, (end - now) * 1000));
    }
    await remote(A, "systemctl is-active --quiet natsume-experiment-sampler");
    state.clock_end = { A: await clockOffset(A), B: await clockOffset(B) };
    state.status = "completed";
  } catch (error) {
    state.status = "failed";
    state.error = String(error);
    console.error(error);
  } finally {
    const cleanup = await stop();
    state.cleanup_errors = cleanup;
    state.export_until_epoch = Number(await remote(A, "date +%s")) + 1;
    await save();
    Deno.removeSignalListener("SIGINT", interrupt);
    Deno.removeSignalListener("SIGTERM", interrupt);
    const since = state.export_since_epoch as number,
      until = state.export_until_epoch as number;
    if (since) {
      await collect(
        A,
        `sudo -n journalctl -u natsume-bench.service -o json --no-pager --since=@${since} --until=@${until}`,
        `${out}/raw/natsume-bench.jsonl`,
      );
    }
    for (
      const [host, source, dest] of [
        [
          A,
          `sudo -n cat ${base}/runs/${e.name}/resources.csv`,
          "resources.csv",
        ],
        [B, `cat ${work}/run/events.csv.gz`, "loadgen-events.csv.gz"],
        [B, `cat ${work}/run/run.json`, "loadgen-run.json"],
        [B, `cat ${work}/loadgen.log`, "loadgen.log"],
      ]
    ) {
      try {
        await collect(host, source, `${out}/${dest}`);
      } catch (error) {
        console.error(String(error));
        state.collection_error = true;
      }
    }
    if (e.actions.some((a) => a.kind === "perf" && a.call_graph !== "dwarf")) {
      await collect(
        A,
        `sudo -n cat ${base}/runs/${e.name}/perf.data`,
        `${out}/perf.data`,
      );
    }
    if (e.actions.some((a) => a.kind === "export")) {
      await collect(
        B,
        `cat ${work}/export/events.csv.gz`,
        `${out}/export-events.csv.gz`,
      );
    }
    if (e.actions.some((a) => a.kind === "probe")) {
      await collect(
        B,
        `cat ${work}/probe/events.csv.gz`,
        `${out}/probe-events.csv.gz`,
      );
    }
    if (e.actions.some((a) => a.kind === "restart")) {
      await collect(
        A,
        `sudo -n cat ${base}/runs/${e.name}/actions.csv`,
        `${out}/actions.csv`,
      );
      for (
        const line of (await Deno.readTextFile(`${out}/actions.csv`)).trim()
          .split("\n").slice(1)
      ) {
        const [kind, planned, before, after] = line.split(",");
        (state.actions as unknown[]).push({
          kind,
          planned_ts_us: +planned,
          before_ts_us: +before,
          after_ts_us: +after,
        });
      }
    }
    if (
      (state.actions as { kind: string }[]).filter((a) => a.kind === "restart")
        .length !== e.actions.filter((a) => a.kind === "restart").length
    ) {
      state.status = "failed";
      state.error = "Restart action did not complete";
    }
    await save();
    await Deno.writeTextFile(
      `${out}/summary.md`,
      `# ${e.name}\n\nStatus: ${state.status}. ${
        state.error ?? ""
      } Raw data and window.json retained.\n`,
    );
  }
  if (state.status === "completed" && state.steady_start && state.steady_end) {
    const analysis = `${out}/analysis`,
      window = `${state.steady_start}/${state.steady_end}`;
    const common = [
      "run",
      "--frozen",
      "--cached-only",
      "--no-prompt",
      `--allow-read=${out}`,
      `--allow-write=${out}`,
      "bench/analyze/journald.ts",
    ];
    for (
      const args of [[
        "extract",
        `${out}/raw/natsume-bench.jsonl`,
        "--out",
        analysis,
      ], [
        "summarize",
        analysis,
        "--out",
        `${analysis}/summary.json`,
        "--window",
        `steady=${window}`,
      ], [
        "series",
        analysis,
        "--out",
        `${analysis}/series`,
        "--series-range",
        window,
      ]]
    ) await command(Deno.execPath(), [...common, ...args]);
    await summarize(out);
  }
  if (
    state.status !== "completed" || (state.cleanup_errors as string[]).length ||
    state.collection_error
  ) throw new Error(`Experiment ${e.name} did not complete cleanly`);
  console.log(`${e.name}: collected and analyzed in ${out}`);
}
if (import.meta.main) {
  if (Deno.args.length !== 1) throw new Error("Usage: run.ts experiment.toml");
  await run(Deno.args[0]);
}
