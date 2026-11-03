import { configuration, quote } from "./config.ts";
import { scenario } from "./run.ts";
import { serverTimestamp, stats } from "./summary.ts";
const hosts =
  '\n[hosts]\nserver="runner@server.example"\nloadgen="runner@load.example"\nserver_url="https://server.example:9443"';
const assert = (condition: unknown) => {
  if (!condition) throw new Error("Assertion failed");
};
Deno.test("experiment rejects invalid paths, unknown fields, timing and rates", () => {
  for (
    const invalid of [
      "name='x'\ndevicez=1",
      "name='x'\nbaseline='../secret.tar.gz'",
      "name='x'\nsteady_seconds=0",
      "name='x'\n[panel_rates]\n'/wrong'=1",
      "name='x'\n[[actions]]\nat=601\nkind='restart'",
    ]
  ) {
    let rejected = false;
    try {
      configuration(invalid + hosts);
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
});
Deno.test("calibration and export schedule retain exact rates", () => {
  const e = configuration(
    "name='synthetic'\n[panel_rates]\n'/devices'=10.616666666666667\n'/session'=2.088888888888889" +
      hosts,
  );
  const encoded = scenario(e, 750, 127);
  assert(encoded.includes('at = "127s"'));
  assert(encoded.includes('"/devices" = 10.616666666666667'));
  assert(quote("a'b") === "'a'\"'\"'b'");
});
Deno.test("nearest rank handles empty and skewed samples", () => {
  assert(stats([]).p50 === null);
  assert(stats([1, 2, 3, 100]).p50 === 2);
  assert(stats([1, 2, 3, 100]).p99 === 100);
});

Deno.test("perf categories count recursive frames once and separate union", async () => {
  const { attribution } = await import("./perf.ts");
  const result = attribution(
    "root;natsume_roster::logos::read_logo;image_webp::lossless::decode 7\nroot;natsume_roster::logos::read_logo;sha2::sha256 2\nroot;unrelated 1",
  );
  assert(result.total_samples === 10);
  assert(result.logo_inclusive_samples === 9);
  assert(result.exclusive_samples.image_decode === 7);
  assert(result.exclusive_samples.sha256 === 2);
});

Deno.test("event alignment corrects clock offset and drift without rewriting raw timestamps", () => {
  assert(
    serverTimestamp(500150, {
      clock_start: {
        A: { offset_us: 0, local_ts_us: 0 },
        B: { offset_us: 100 },
      },
      clock_end: {
        A: { offset_us: 0, local_ts_us: 1000000 },
        B: { offset_us: 200 },
      },
    }) === 500000,
  );
  assert(serverTimestamp(120, { clock_B_minus_A_us: 20 }) === 100);
});

Deno.test("hosts are required and reject unsafe SSH targets and non-origin URLs", () => {
  for (
    const suffix of [
      "",
      hosts.replace("runner@server.example", "-oProxyCommand=bad"),
      hosts.replace("runner@load.example", "runner@server.example"),
      hosts.replace(
        "https://server.example:9443",
        "http://server.example:9443",
      ),
      hosts.replace(
        "https://server.example:9443",
        "https://user:secret@server.example:9443",
      ),
      hosts.replace(
        "https://server.example:9443",
        "https://server.example:9443/path",
      ),
      hosts + "\nport=8443",
      hosts.replace("runner@server.example", "runner@server.example;touch bad"),
    ]
  ) {
    let rejected = false;
    try {
      configuration("name='hosts'" + suffix);
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
  const e = configuration("name='hosts'" + hosts);
  assert(e.hosts.server_url === "https://server.example:9443");
});

Deno.test("all supplied experiments have valid explicit hosts", async () => {
  for await (const entry of Deno.readDir("bench/run/experiments")) {
    if (entry.name.endsWith(".toml")) {
      configuration(
        await Deno.readTextFile(`bench/run/experiments/${entry.name}`),
      );
    }
  }
});
