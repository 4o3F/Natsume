import { configuration, type Experiment } from "./config.ts";
import { remote } from "./run.ts";

/** Select the symbol-bearing build for CPU profiling only; keep the B0 executable. */
export async function symbols(
  e: Experiment,
  enable: boolean,
  framePointers = false,
) {
  const A = e.hosts.server;
  const binary = framePointers
    ? "natsume-server-frame-pointers"
    : "natsume-server-symbols";
  await remote(
    A,
    `test "$(systemctl show natsume-bench -p MainPID --value)" = 0\nsudo -n bash -se <<'SYMBOLS'\nset -euo pipefail\ndirectory=/run/systemd/system/natsume-bench.service.d\n${
      enable
        ? `test -x /opt/natsume-bench/${e.version}/${binary}\ninstall -d -m 755 "$directory"\ntest ! -e "$directory/profile.conf"\ncat > "$directory/profile.conf" <<'UNIT'\n[Service]\nExecStart=\nExecStart=/opt/natsume-bench/${e.version}/${binary} serve\nUNIT`
        : `if test -d "$directory"; then\n  test "$(stat -c %u "$directory")" = 0\n  mode=$(stat -c %a "$directory")\n  test $((8#$mode & 0022)) -eq 0\n  rm -f "$directory/profile.conf"\nfi`
    }\nsystemctl daemon-reload\nsystemctl show natsume-bench -p ExecStart\nSYMBOLS`,
  );
}
if (import.meta.main) {
  if (
    Deno.args.length !== 2 || !["on", "off", "on-fp"].includes(Deno.args[0])
  ) {
    throw new Error("Usage: symbols.ts on|off|on-fp experiment.toml");
  }
  await symbols(
    configuration(await Deno.readTextFile(Deno.args[1])),
    Deno.args[0] !== "off",
    Deno.args[0] === "on-fp",
  );
}
