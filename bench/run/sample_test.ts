import { resolve } from "@std/path";

Deno.test("sampler survives a stopped/restarting unit and keeps the requested cadence", async () => {
  const parent = "target/bench-run-tests";
  await Deno.mkdir(parent, { recursive: true });
  const directory = await Deno.makeTempDir({ dir: parent });
  let child: Deno.ChildProcess | undefined;
  try {
    for (
      const [name, body] of [[
        "systemctl",
        'case "$*" in *MainPID*) echo 0;; *ControlGroup*) echo /;; esac',
      ], ["ss", 'test "${@: -1}" = :9443 || exit 1; printf "one\\ntwo\\n"']]
    ) {
      await Deno.writeTextFile(
        `${directory}/${name}`,
        `#!/bin/bash\n${body}\n`,
      );
      await Deno.chmod(`${directory}/${name}`, 0o700);
    }
    // Only systemctl and ss are fixtures; timestamps and scheduling use the local clock.
    child = new Deno.Command("bash", {
      args: [
        "-c",
        'export PATH="$1:$PATH"; exec bash bench/run/sample.sh 0.05 9443',
        "fixture",
        resolve(directory),
      ],
      stdout: "piped",
      stderr: "piped",
    }).spawn();
    const stdout = child.stdout.text(), stderr = child.stderr.text();
    let finished = false;
    const status = child.status.then((result) => {
      finished = true;
      return result;
    });
    await new Promise((r) => setTimeout(r, 400));
    try {
      if (!finished) child.kill("SIGTERM");
    } catch (error) {
      if (!(error instanceof Deno.errors.NotFound)) throw error;
    }
    await status;
    const output = await stdout,
      error = await stderr,
      rows = output.trim().split("\n");
    if (error || rows.length < 5) {
      throw new Error(`Sampler stopped early: ${error}; ${rows.length} rows`);
    }
    const header = rows[0].split(","),
      connections = header.indexOf("tcp_8443_established"),
      cpu = header.indexOf("cpu_user_percent");
    for (const row of rows.slice(1)) {
      const cells = row.split(",");
      if (cells[connections] !== "2" || cells[cpu] !== "") {
        throw new Error("Stopped-unit sample has incorrect values");
      }
    }
    const elapsed = (+rows.at(-1)!.split(",")[0] - +rows[1].split(",")[0]) /
      1e6;
    if (elapsed < .1 || elapsed > 1) {
      throw new Error(`Unexpected sample cadence ${elapsed}`);
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("restart action records its actual time after the scheduled deadline", async () => {
  const { restartScript } = await import("./actions.ts");
  await Deno.mkdir("target/bench-run-tests", { recursive: true });
  const directory = await Deno.makeTempDir({ dir: "target/bench-run-tests" });
  try {
    await Deno.writeTextFile(`${directory}/systemctl`, "#!/bin/bash\nexit 0\n");
    await Deno.chmod(`${directory}/systemctl`, 0o700);
    const start = (Date.now() + 200) / 1000;
    const child = new Deno.Command("bash", {
      args: [
        "-c",
        'export PATH="$1:$PATH"; exec bash -s',
        "fixture",
        resolve(directory),
      ],
      stdin: "piped",
      stdout: "piped",
      stderr: "piped",
    }).spawn();
    const writer = child.stdin.getWriter();
    await writer.write(
      new TextEncoder().encode(
        restartScript([{ at: 0, kind: "restart" }], start),
      ),
    );
    await writer.close();
    const result = await child.output();
    if (!result.success) {
      throw new Error(new TextDecoder().decode(result.stderr));
    }
    const row = new TextDecoder().decode(result.stdout).trim().split("\n")[1]
      .split(",");
    if (
      row[0] !== "restart" || +row[2] < +row[1] - 1000 || +row[3] < +row[2] ||
      +row[2] - +row[1] > 2e6
    ) throw new Error(`Incorrect action timing ${row}`);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});
