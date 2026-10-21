import { resolve } from "@std/path";
import manifest from "../packaging/manifest.json" with { type: "json" };
import {
  exists,
  isPackageTarget,
  main,
  packageTargets,
  readyPath,
  recordPrepared,
  root,
  run,
  runtime,
  selection,
  sha256,
  work,
} from "./process.ts";

type Asset = { filename: string; url: string; sha256: string };

async function download(asset: Asset, directory: string): Promise<string> {
  const destination = resolve(directory, asset.filename);
  await Deno.mkdir(directory, { recursive: true });
  if (!await exists(destination)) {
    const temporary = await Deno.makeTempFile({ dir: directory });
    try {
      await run("curl", [
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--retry",
        "3",
        "--retry-all-errors",
        "--connect-timeout",
        "10",
        "--max-time",
        "90",
        asset.url,
        "--output",
        temporary,
      ]);
      if (await sha256(temporary) !== asset.sha256) {
        throw new Error(`Checksum mismatch: ${asset.filename}`);
      }
      await Deno.rename(temporary, destination);
    } finally {
      if (await exists(temporary)) await Deno.remove(temporary);
    }
  }
  if (await sha256(destination) !== asset.sha256) {
    throw new Error(
      `Checksum mismatch: ${asset.filename}; remove the corrupt cache explicitly.`,
    );
  }
  return destination;
}

async function extract(
  archive: string,
  member: string,
  destination: string,
  mode: number,
): Promise<void> {
  const temporary = await Deno.makeTempDir({ dir: work });
  try {
    // Only fixed, named members from the checksum-pinned archive are extracted.
    await run("tar", [
      "-xf",
      archive,
      "-C",
      temporary,
      "--no-same-owner",
      member,
    ]);
    const source = resolve(temporary, member);
    if (!(await Deno.lstat(source)).isFile) {
      throw new Error(`Expected a regular archive member: ${member}`);
    }
    await Deno.mkdir(resolve(destination, ".."), { recursive: true });
    await Deno.copyFile(source, destination);
    await Deno.chmod(destination, mode);
  } finally {
    await Deno.remove(temporary, { recursive: true });
  }
}

async function tool(name: "nfpm" | "caddy"): Promise<void> {
  const archive = await download(manifest[name], resolve(work, "downloads"));
  const destination = resolve(work, "tools", name);
  await extract(archive, name, destination, 0o755);
  if (name === "caddy") {
    if (await sha256(destination) !== manifest.caddy.binary_sha256) {
      throw new Error("Caddy executable checksum mismatch");
    }
  }
}

async function skiaAsset(): Promise<Asset> {
  if (Deno.build.os !== "linux" || Deno.build.arch !== "x86_64") {
    throw new Error(
      "Client builds require the prepared Linux amd64 Skia library",
    );
  }
  const lock = await Deno.readTextFile(resolve(root, "Cargo.lock"));
  if (
    !lock.includes(
      `name = "skia-bindings"\nversion = "${manifest.skia.version}"`,
    )
  ) {
    throw new Error(
      "Update packaging/manifest.json for the locked Skia version",
    );
  }
  return manifest.skia;
}

export async function skiaEnvironment(): Promise<Record<string, string>> {
  const asset = await skiaAsset();
  const archive = resolve(work, "downloads", asset.filename);
  if (!await exists(archive) || await sha256(archive) !== asset.sha256) {
    throw new Error(
      "Run deno task prepare rust to prepare the pinned Skia library",
    );
  }
  return {
    SKIA_BINARIES_URL: `file://${work}/downloads/skia-binaries-{key}.tar.gz`,
  };
}

export async function clientLicenses(): Promise<void> {
  const directory = resolve(work, "licenses/client");
  for (
    const [name, member, destination] of [
      ["caddy", "LICENSE", "Caddy-LICENSE"],
      ["skia", "skia-binaries/LICENSE_SKIA", "Skia-LICENSE"],
    ] as const
  ) {
    const archive = await download(manifest[name], resolve(work, "downloads"));
    await extract(archive, member, resolve(directory, destination), 0o644);
  }
}

export async function printAssets(): Promise<void> {
  const source = resolve(root, "packaging/print-domjudge");
  const downloads = resolve(work, "downloads");
  const archive = await download(manifest.typst, downloads);
  const members = [
    ["typst", "typst", 0o755],
    ["LICENSE", "licenses/Typst-LICENSE", 0o644],
    ["NOTICE", "licenses/Typst-NOTICE", 0o644],
  ] as const;
  for (const [member, destination, mode] of members) {
    await extract(
      archive,
      `typst-x86_64-unknown-linux-musl/${member}`,
      resolve(runtime, destination),
      mode,
    );
  }
  if (
    await sha256(resolve(runtime, "typst")) !== manifest.typst.binary_sha256
  ) throw new Error("Typst executable checksum mismatch");
  await Deno.mkdir(resolve(runtime, "fonts"), { recursive: true });
  await Deno.copyFile(
    await download(manifest.font, downloads),
    resolve(runtime, "fonts/NotoSansMonoCJKsc-Regular.otf"),
  );
  await Deno.copyFile(
    await download(manifest.font_license, downloads),
    resolve(runtime, "licenses/Noto-OFL.txt"),
  );
  await Deno.copyFile(
    resolve(source, "template.typ"),
    resolve(runtime, "template.typ"),
  );
}

async function avatar(): Promise<void> {
  const destination = resolve(work, "author-avatar.png");
  const directory = await Deno.makeTempDir({ dir: work });
  const fetch = async (url: string, path: string) => {
    await run("curl", [
      "--fail",
      "--silent",
      "--show-error",
      "--location",
      "--proto",
      "=https",
      "--proto-redir",
      "=https",
      "--connect-timeout",
      "10",
      "--max-time",
      "30",
      "--retry",
      "2",
      "--max-filesize",
      "2097152",
      url,
      "--output",
      path,
    ]);
  };
  try {
    const profile = resolve(directory, "profile.json");
    await fetch("https://api.gravatar.com/v3/profiles/4o3f", profile);
    const url = new URL(
      JSON.parse(await Deno.readTextFile(profile)).avatar_url,
    );
    if (
      url.protocol !== "https:" || url.username || url.password ||
      !(url.hostname === "gravatar.com" ||
        url.hostname.endsWith(".gravatar.com")) ||
      !url.pathname.startsWith("/avatar/")
    ) throw new Error("Expected a Gravatar HTTPS avatar URL");
    url.search = "s=512&d=404";
    url.hash = "";
    const image = resolve(directory, "avatar.png");
    await fetch(url.href, image);
    const bytes = await Deno.readFile(image);
    const prefix = [
      137,
      80,
      78,
      71,
      13,
      10,
      26,
      10,
      0,
      0,
      0,
      13,
      73,
      72,
      68,
      82,
      0,
      0,
      2,
      0,
      0,
      0,
      2,
      0,
    ];
    const suffix = [0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
    if (
      !prefix.every((byte, index) => bytes[index] === byte) ||
      !suffix.every((byte, index) =>
        bytes[bytes.length - suffix.length + index] === byte
      )
    ) throw new Error("Expected a complete 512 x 512 PNG avatar");
    await Deno.chmod(image, 0o644);
    await Deno.rename(image, destination);
    console.log(
      `Author avatar: ${destination} (SHA-256 ${await sha256(destination)})`,
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

async function prepare(): Promise<void> {
  const version = (await Deno.readTextFile(resolve(root, ".deno-version")))
    .trim();
  if (Deno.version.deno !== version) {
    throw new Error(`Use Deno ${version} from .deno-version`);
  }
  const selected = new Set(selection(Deno.args));
  selected.add("scripts");
  const needsSkia = selected.has("rust") && Deno.build.os !== "windows" ||
    selected.has("client") && Deno.env.get("RUST_RELEASE_DIR") === undefined;
  const packages = [...selected].filter(isPackageTarget);
  if (
    packages.length &&
    (Deno.build.os !== "linux" || Deno.build.arch !== "x86_64" ||
      (Deno.env.get("ARCH") ?? "amd64") !== "amd64")
  ) throw new Error("Package preparation requires Linux amd64");
  if (packages.length && Deno.env.get("RUST_RELEASE_DIR") === "") {
    throw new Error("RUST_RELEASE_DIR must not be empty");
  }
  if (packages.length && Deno.env.get("RUST_RELEASE_DIR") === undefined) {
    selected.add("rust");
  }
  if (packages.some((target) => packageTargets[target].web)) {
    selected.add("web");
  }
  await Deno.mkdir(work, { recursive: true });
  // A failed prepare must not leave an older success marker for these inputs.
  for (const target of selected) {
    if (await exists(readyPath(target))) await Deno.remove(readyPath(target));
  }
  await run(Deno.execPath(), [
    "cache",
    "--frozen",
    "tools/assets.ts",
    "tools/build.ts",
    "tools/package.ts",
    "tools/build_test.ts",
  ]);
  if (selected.has("rust")) await run("cargo", ["fetch", "--locked"]);
  if (needsSkia) await download(await skiaAsset(), resolve(work, "downloads"));
  if (selected.has("web")) {
    await run("pnpm", ["install", "--frozen-lockfile"], {
      cwd: resolve(root, "web"),
    });
  }
  if (packages.length) await tool("nfpm");
  if (selected.has("client")) {
    await tool("caddy");
    await clientLicenses();
    await avatar();
  }
  if (selected.has("print-domjudge")) await printAssets();
  await recordPrepared([...selected]);
  console.log(`Prepared: ${[...selected].join(", ")}`);
}

if (import.meta.main) await main(prepare);
