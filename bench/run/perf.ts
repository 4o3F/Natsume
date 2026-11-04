import { configuration, type Experiment } from "./config.ts";
import { command, remote } from "./run.ts";

export function attribution(folded: string) {
  const groups = {
    image_decode: 0,
    sha256: 0,
    other_logo: 0,
    other: 0,
    unresolved: 0,
  };
  let total = 0, logoInclusive = 0;
  const top = new Map<string, number>();
  for (const line of folded.trim().split("\n")) {
    const match = /^(.*) (\d+)$/.exec(line);
    if (!match) throw new Error("Invalid folded stack");
    const stack = match[1], count = +match[2];
    total += count;
    const decode =
      /image_webp::|image::codecs::|image::images::|image::io::|image::dynimage::|png::|zune_|decode_image|natsume_roster::logos::decode_/
        .test(stack);
    const sha = /sha2::|sha256|sha2_256|sha256_block|sha256_transform/i.test(
      stack,
    );
    const logo =
      /natsume_roster::logos|natsume_server::component::contest::logos|organization_logo|public_logo|read_logo/
        .test(stack);
    if (decode || sha || logo) logoInclusive += count;
    if (decode) groups.image_decode += count;
    else if (sha) groups.sha256 += count;
    else if (logo) groups.other_logo += count;
    else if (
      !stack.includes(";") || /;(?:unknown|\[unknown\])(?:;|$)/.test(stack)
    ) groups.unresolved += count;
    else groups.other += count;
    const leaf = stack.split(";").at(-1)!;
    top.set(leaf, (top.get(leaf) ?? 0) + count);
  }
  return {
    total_samples: total,
    logo_inclusive_samples: logoInclusive,
    logo_inclusive_percent: 100 * logoInclusive / total,
    exclusive_samples: groups,
    exclusive_percent: Object.fromEntries(
      Object.entries(groups).map(([k, v]) => [k, 100 * v / total]),
    ),
    top_leaves: [...top].sort((a, b) => b[1] - a[1]).slice(0, 30),
  };
}
export function profileTitle(
  e: Pick<Experiment, "name" | "version" | "devices">,
) {
  return `${e.name}: ${e.version} N=${e.devices}`;
}

export async function analyzePerf(base: string, flamegraph: string) {
  const e = configuration(await Deno.readTextFile(`${base}/experiment.toml`));
  const A = e.hosts.server;
  const perfAction = e.actions.find((a) => a.kind === "perf");
  if (!perfAction) throw new Error("Experiment has no perf action");
  const script = await remote(
    A,
    `sudo -n perf script --no-inline -i /srv/natsume-bench/${e.version}/runs/${e.name}/perf.data -F comm,pid,tid,time,event,ip,sym,dso`,
  );
  await Deno.writeTextFile(`${base}/perf.script`, script + "\n\n");
  await Deno.writeTextFile(
    `${base}/perf-collapse.script`,
    script.replaceAll(";", ":") + "\n\n",
  );
  const folded = await command("perl", [
    `${flamegraph}/stackcollapse-perf.pl`,
    `${base}/perf-collapse.script`,
  ]);
  await Deno.writeTextFile(`${base}/perf.folded`, folded + "\n");
  const svg = await command("perl", [
    `${flamegraph}/flamegraph.pl`,
    "--title",
    profileTitle(e),
    `${base}/perf.folded`,
  ]);
  await Deno.writeTextFile(`${base}/flamegraph.svg`, svg + "\n");
  const result = attribution(folded);
  await Deno.writeTextFile(
    `${base}/perf-attribution.json`,
    JSON.stringify(result, null, 2) + "\n",
  );
  const revision = await command("git", [
    "-C",
    flamegraph,
    "rev-parse",
    "HEAD",
  ]);
  await Deno.writeTextFile(
    `${base}/summary.md`,
    (await Deno.readTextFile(`${base}/summary.md`)).split("\nCPU profile:")[0] +
      `\nCPU profile: ${result.total_samples} on-CPU samples; logo/decode/SHA union ${
        result.logo_inclusive_percent.toFixed(2)
      }%. Exclusive categories: ${
        JSON.stringify(result.exclusive_percent)
      }. Raw perf.data, perf.script, perf.folded and flamegraph.svg are retained. FlameGraph revision ${revision}; recording: perf record -F 99 --call-graph ${
        perfAction?.call_graph === "fp" ? "fp" : "dwarf,16384"
      } -p PID -- sleep ${perfAction.seconds}. Categories match symbols in sampled stacks; inlined and unresolved frames affect attribution.\n`,
  );
  console.log(
    JSON.stringify({
      total_samples: result.total_samples,
      logo_inclusive_percent: result.logo_inclusive_percent,
      exclusive_percent: result.exclusive_percent,
    }),
  );
}
if (import.meta.main) {
  await analyzePerf(Deno.args[0], Deno.args[1] ?? "target/bench-flamegraph");
}
