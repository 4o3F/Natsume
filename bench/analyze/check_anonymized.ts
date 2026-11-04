/** Independent raw-value inventory and recursive streaming leak check. */
import { join } from "@std/path";
import { lines } from "./io.ts";

const JOURNAL_KEYS = [
  "_HOSTNAME",
  "_MACHINE_ID",
  "_BOOT_ID",
  "_STREAM_ID",
  "_SYSTEMD_INVOCATION_ID",
  "INVOCATION_ID",
  "__CURSOR",
  "__SEQNUM_ID",
];
const ACTOR = /\bactor_id="?([0-9a-fA-F-]{32,36})/g;
const ORGANIZATION = /\borganization_id=("?)([0-9]+)\1/g;
// deno-lint-ignore no-control-regex -- NUL terminates inventoried filesystem paths.
const PATH = /(?:[A-Za-z]:[\\/]|\/)[^\s"\x00,;<>]*/g;
const TOKEN = /[A-Za-z0-9_./:;=\\%+@~-]+/g;
const ADDRESS =
  /(?<![\w.])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![\w.])|\[([0-9a-fA-F:]+)\]/g;
const LISTEN = /\blisten_address="?(\[[^\]]+\]|[^\s":]+):[0-9]+/g;

function isIP(value: string): boolean {
  if (value.includes(":")) {
    try {
      return new URL(`http://[${value}]/`).hostname.startsWith("[");
    } catch {
      return false;
    }
  }
  const parts = value.split(".");
  return parts.length === 4 &&
    parts.every((part) =>
      /^(0|[1-9][0-9]{0,2})$/.test(part) && Number(part) <= 255
    );
}

function* tokens(text: string) {
  for (const match of text.matchAll(TOKEN)) {
    const token = match[0].replace(/^\.+|\.+$/g, "");
    yield token;
    for (const part of token.split(/[;=]/)) {
      if (part) yield part.replace(/^\.+|\.+$/g, "");
    }
    if (token.includes(":")) {
      yield token.slice(0, token.lastIndexOf(":")).replace(
        /^[\[\]]+|[\[\]]+$/g,
        "",
      );
    }
  }
}
async function collect(raw: string) {
  const values = new Map<string, Set<string>>();
  function add(kind: string, value: string) {
    if (!values.has(kind)) values.set(kind, new Set());
    values.get(kind)!.add(value);
  }
  for await (const line of lines(raw)) {
    const record = JSON.parse(line);
    for (const key of JOURNAL_KEYS) {
      if (!record[key]) continue;
      const value = String(record[key]);
      add(key, value);
      if (key === "__CURSOR") {
        for (const part of value.split(";")) add(key, part);
      }
    }
    const text: string = Array.isArray(record.MESSAGE)
      ? new TextDecoder().decode(new Uint8Array(record.MESSAGE))
      : record.MESSAGE ?? "";
    for (const match of text.matchAll(ACTOR)) add("actor_id", match[1]);
    for (const match of text.matchAll(ORGANIZATION)) {
      add("organization_id", match[0]);
    }
    for (
      const value of [
        text,
        ...Object.entries(record).filter(([k]) => k !== "MESSAGE").map((
          [, v],
        ) => String(v)),
      ]
    ) {
      for (const char of value) {
        if (char.codePointAt(0)! > 127) add("non_ascii", char);
      }
      for (const match of value.matchAll(PATH)) {
        const path = match[0].replace(/[.):]+$/g, "");
        if (!path.startsWith("/api/") && path.replaceAll("/", "")) {
          add("filesystem_path", path);
        }
      }
      if (value.split(":").length >= 3) {
        for (const match of value.matchAll(/[0-9a-fA-F:]+:[0-9a-fA-F:]+/g)) {
          if (isIP(match[0])) add("ip_address", match[0]);
        }
      }
      if (value.includes(".") || value.includes("[")) {
        for (const match of value.matchAll(ADDRESS)) {
          const address = match[1] ?? match[0];
          if (isIP(address)) add("ip_address", address);
        }
      }
    }
    for (const match of text.matchAll(LISTEN)) {
      add("listen_host", match[1].replace(/^[\[\]]+|[\[\]]+$/g, ""));
    }
  }
  return values;
}
async function* files(directory: string): AsyncGenerator<string> {
  const entries = await Array.fromAsync(Deno.readDir(directory));
  entries.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory) yield* files(path);
    else if (entry.isFile) yield path;
  }
}
export async function check(raw: string, dataset: string) {
  const sensitive = await collect(raw), patterns = new Map<string, RegExp>();
  for (const [kind, values] of sensitive) {
    if (!values.size || kind === "__CURSOR") continue;
    const escaped = [...values].sort((a, b) => b.length - a.length).map((v) =>
      v.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    );
    patterns.set(kind, new RegExp(escaped.join("|"), "gu"));
  }
  const hits: Record<string, number> = {};
  for await (const path of files(dataset)) {
    for await (let line of lines(path, path.endsWith(".gz"))) {
      if (line.includes("\\")) {
        line = line.replace(
          /\\u([0-9a-fA-F]{4})/g,
          (_, hex) => String.fromCharCode(parseInt(hex, 16)),
        ).replaceAll("\\\\", "\\");
      }
      for (const [kind, pattern] of patterns) {
        const count = [...line.matchAll(pattern)].length;
        if (count) hits[kind] = (hits[kind] ?? 0) + count;
      }
      const candidates = new Set([
        ...tokens(line),
        ...[...line.matchAll(/[a-z]=[A-Za-z0-9]+/g)].map((m) => m[0]),
      ]);
      for (const value of candidates) {
        if (sensitive.get("__CURSOR")?.has(value)) {
          hits.__CURSOR = (hits.__CURSOR ?? 0) + 1;
        }
      }
    }
  }
  return hits;
}
if (import.meta.main) {
  try {
    if (Deno.args.length !== 2) throw new Error("Invalid arguments");
    const hits = await check(Deno.args[0], Deno.args[1]);
    if (Object.keys(hits).length) {
      console.log(JSON.stringify(hits));
      Deno.exit(1);
    }
    console.log("PASS: no inventoried sensitive values found");
  } catch {
    console.error(
      "Anonymization check failed; check arguments, permissions and input format.",
    );
    Deno.exit(1);
  }
}
