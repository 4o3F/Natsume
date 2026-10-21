import { fromFileUrl, resolve } from "@std/path";
import manifest from "../packaging/manifest.json" with { type: "json" };

export const root = fromFileUrl(new URL("../", import.meta.url));
export const work = resolve(root, "target/build");
export const runtime = resolve(root, "target/print-domjudge/runtime");
export const packageTargets = {
  server: {
    crates: ["natsume-server"],
    binaries: ["natsume-server"],
    assets: ["nfpm"],
    web: true,
  },
  client: {
    crates: [
      "natsume-workstation-daemon",
      "natsume-workstation-helper",
      "natsume-workstation-agent",
    ],
    binaries: [
      "natsume-device-daemon",
      "natsume-privileged-helper",
      "natsume-session-agent",
    ],
    assets: ["nfpm", "caddy", "skia"],
    web: false,
  },
  "print-domjudge": {
    crates: ["natsume-print-domjudge"],
    binaries: ["natsume-print-domjudge"],
    assets: ["nfpm", "typst", "font", "font_license"],
    web: false,
  },
} as const;
export type PackageTarget = keyof typeof packageTargets;
export const targets = [
  "scripts",
  "rust",
  "web",
  ...Object.keys(packageTargets) as PackageTarget[],
] as const;
export type Target = typeof targets[number];

export function isPackageTarget(target: Target): target is PackageTarget {
  return Object.hasOwn(packageTargets, target);
}

export function selection(args: string[]): Target[] {
  if (args.length === 0 || args.length === 1 && args[0] === "all") {
    return [...targets];
  }
  if (args.some((value) => !targets.includes(value as Target))) {
    throw new Error(`Expected all or one or more of: ${targets.join(", ")}`);
  }
  return [...new Set(args as Target[])];
}

export async function run(
  command: string,
  args: string[],
  options: { cwd?: string; env?: Record<string, string>; capture?: boolean } =
    {},
): Promise<string> {
  console.log(`$ ${command} ${args.join(" ")}`);
  const child = new Deno.Command(command, {
    args,
    cwd: options.cwd ?? root,
    env: options.env,
    stdin: "null",
    stdout: options.capture ? "piped" : "inherit",
    stderr: "inherit",
  }).spawn();
  // Only small metadata outputs use capture; compiler logs remain streaming.
  const result = options.capture
    ? await child.output()
    : { ...await child.status, stdout: new Uint8Array() };
  if (!result.success) {
    throw new Error(`${command} failed (exit ${result.code})`);
  }
  return new TextDecoder().decode(result.stdout);
}

export const offline = {
  CARGO_NET_OFFLINE: "true",
  COREPACK_ENABLE_NETWORK: "0",
  npm_config_offline: "true",
};

export async function exists(path: string): Promise<boolean> {
  try {
    await Deno.stat(path);
    return true;
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) return false;
    throw error;
  }
}

function digest(bytes: Uint8Array<ArrayBuffer>): Promise<ArrayBuffer> {
  return crypto.subtle.digest("SHA-256", bytes);
}

function hex(bytes: ArrayBuffer): string {
  return Array.from(
    new Uint8Array(bytes),
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
}

export async function sha256(path: string): Promise<string> {
  return hex(await digest(await Deno.readFile(path)));
}

async function fingerprint(target: Target): Promise<string> {
  const paths = [
    "deno.json",
    "deno.lock",
    ".deno-version",
  ];
  if (target === "rust") {
    paths.push(
      "Cargo.toml",
      "Cargo.lock",
      "rust-toolchain.toml",
    );
    for await (const entry of Deno.readDir(resolve(root, "crates"))) {
      const manifest = `crates/${entry.name}/Cargo.toml`;
      if (entry.isDirectory && await exists(resolve(root, manifest))) {
        paths.push(manifest);
      }
    }
  } else if (target === "web") {
    paths.push(
      "web/package.json",
      "web/pnpm-lock.yaml",
      "web/pnpm-workspace.yaml",
      "web/.node-version",
    );
  } else if (target === "print-domjudge") {
    paths.push("packaging/print-domjudge/template.typ");
  }
  const assets = target === "rust"
    ? ["skia"] as const
    : isPackageTarget(target)
    ? packageTargets[target].assets
    : [];
  const hashes = assets.map((name) =>
    `${name}\0${JSON.stringify(manifest[name])}\0`
  );
  for (const path of paths.sort()) {
    hashes.push(`${path}\0${await sha256(resolve(root, path))}\0`);
  }
  return hex(await digest(new TextEncoder().encode(hashes.join(""))));
}

export function readyPath(target: Target): string {
  return resolve(work, `prepared-${target}`);
}

export async function recordPrepared(selected: Target[]): Promise<void> {
  for (const target of selected) {
    await Deno.writeTextFile(readyPath(target), await fingerprint(target));
  }
}

export async function requirePrepared(...selected: Target[]): Promise<void> {
  for (const target of selected) {
    if (
      !await exists(readyPath(target)) ||
      await Deno.readTextFile(readyPath(target)) !== await fingerprint(target)
    ) {
      throw new Error(
        `Run deno task prepare ${target} before this task (inputs are missing or changed).`,
      );
    }
  }
}

export async function main(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    console.error(error instanceof Error ? error.message : "Build task failed");
    Deno.exitCode = 1;
  }
}
