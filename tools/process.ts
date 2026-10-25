import { fromFileUrl, resolve } from "@std/path";
import manifest from "../packaging/manifest.json" with { type: "json" };
import { execute, type RunOptions } from "./entry.ts";
export { main, validateDenoVersion } from "./entry.ts";

export const root = fromFileUrl(new URL("../", import.meta.url));
export const work = resolve(root, "target/build");
export const runtime = resolve(root, "target/print-domjudge/runtime");
type PackageDefinition =
  & {
    crates: readonly string[];
    binaries: readonly string[];
    rustPlatforms: readonly string[];
    assets: readonly (keyof typeof manifest)[];
    web: boolean;
  }
  & ({ platform: "linux"; format: "deb" } | {
    platform: "windows";
    format: "zip";
    readme: string;
  });

export const packageTargets = {
  server: {
    platform: "linux",
    rustPlatforms: ["linux"],
    format: "deb",
    crates: ["natsume-server"],
    binaries: ["natsume-server"],
    assets: ["nfpm"],
    web: true,
  },
  client: {
    platform: "linux",
    rustPlatforms: ["linux"],
    format: "deb",
    crates: [
      "natsume-workstation-daemon",
      "natsume-workstation-helper",
      "natsume-workstation-agent",
      "natsume-workstation-protocol",
      "natsume-workstation-ipc",
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
    platform: "linux",
    rustPlatforms: ["linux"],
    format: "deb",
    crates: ["natsume-print-domjudge"],
    binaries: ["natsume-print-domjudge"],
    assets: ["nfpm", "typst", "font", "font_license"],
    web: false,
  },
  "print-client": {
    platform: "windows",
    rustPlatforms: ["linux", "windows"],
    format: "zip",
    crates: ["natsume-print-client"],
    binaries: ["natsume-print-client"],
    assets: [],
    web: false,
    readme: "packaging/print-client/README.md",
  },
} as const satisfies Record<string, PackageDefinition>;
export type PackageTarget = keyof typeof packageTargets;
export const targets = [
  "scripts",
  "rust",
  "shared",
  "web",
  ...Object.keys(packageTargets) as PackageTarget[],
] as const;
export type Target = typeof targets[number];

export function isPackageTarget(target: Target): target is PackageTarget {
  return Object.hasOwn(packageTargets, target);
}

export function packagesFor(platform: string): PackageTarget[] {
  return (Object.keys(packageTargets) as PackageTarget[]).filter((target) =>
    packageTargets[target].platform === platform
  );
}

export function sharedRustCrates(platform: string = Deno.build.os): string[] {
  const definitions: readonly PackageDefinition[] = Object.values(
    packageTargets,
  );
  return [
    ...new Set(
      definitions
        .filter((target) =>
          target.platform !== platform &&
          target.rustPlatforms.includes(platform)
        )
        .flatMap((target) => target.crates),
    ),
  ];
}

export function selection(
  args: string[],
  platform: string = Deno.build.os,
): Target[] {
  if (args.length === 0 || args.length === 1 && args[0] === "all") {
    return platform === "linux"
      ? ["scripts", "rust", "web", ...packagesFor(platform)]
      : packagesFor(platform);
  }
  if (args.some((value) => !targets.includes(value as Target))) {
    throw new Error(`Expected all or one or more of: ${targets.join(", ")}`);
  }
  for (const target of args as Target[]) {
    if (
      isPackageTarget(target) && packageTargets[target].platform !== platform
    ) {
      throw new Error(
        `${target} requires ${
          packageTargets[target].platform
        }; host is ${platform}`,
      );
    }
  }
  return [...new Set(args as Target[])];
}

export function rustArguments(
  platform: string = Deno.build.os,
  definitions: Record<string, {
    crates: readonly string[];
    rustPlatforms: readonly string[];
  }> = packageTargets,
): string[] {
  if (platform !== "linux" && platform !== "windows") {
    throw new Error(`Rust build platform is not supported: ${platform}`);
  }
  const packages = Object.values(definitions);
  if (platform === "linux") {
    return [
      "--workspace",
      ...packages
        .filter((target) => !target.rustPlatforms.includes(platform))
        .flatMap((target) =>
          target.crates.flatMap((name) => ["--exclude", name])
        ),
    ];
  }
  return [
    ...new Set(
      packages
        .filter((target) => target.rustPlatforms.includes(platform))
        .flatMap((target) => target.crates),
    ),
  ].flatMap((name) => ["-p", name]);
}

export function run(
  command: string,
  args: string[],
  options: RunOptions = {},
): Promise<string> {
  return execute(command, args, { ...options, cwd: options.cwd ?? root });
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
    ? Deno.build.os === "linux" ? ["skia"] as const : []
    : isPackageTarget(target)
    ? packageTargets[target].assets
    : [];
  const hashes = [
    `${Deno.build.os}\0${Deno.build.arch}\0`,
    ...(isPackageTarget(target)
      ? [JSON.stringify(packageTargets[target])]
      : []),
    ...assets.map((name) => `${name}\0${JSON.stringify(manifest[name])}\0`),
  ];
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
