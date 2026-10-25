import { resolve } from "@std/path";
import {
  type PackageTarget,
  packageTargets,
  root,
  run,
  sha256,
  work,
} from "./process.ts";

export function validateWindowsVersion(version: string): void {
  const number = "(?:0|[1-9][0-9]*)";
  const match = new RegExp(
    `^${number}\\.${number}\\.${number}(?:-([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?$`,
  ).exec(version);
  if (
    !match || match[0] !== version ||
    match[1]?.split(".").some((part) => /^0[0-9]+$/.test(part))
  ) throw new Error("Windows ZIP versions must be SemVer, not Debian versions");
}

async function copyFile(source: string, destination: string): Promise<void> {
  if (!(await Deno.lstat(source)).isFile) {
    throw new Error(`Expected a regular package input: ${source}`);
  }
  await Deno.copyFile(source, destination);
}

export async function stageWindowsPackage(
  target: PackageTarget,
  version: string,
  release: string,
  directory: string,
): Promise<void> {
  validateWindowsVersion(version);
  const config = packageTargets[target];
  if (config.format !== "zip") {
    throw new Error(`${target} is not a Windows ZIP target`);
  }
  await Deno.mkdir(directory, { recursive: true });
  const binaries = config.binaries.map((name) => `${name}.exe`);
  for (const name of binaries) {
    const source = resolve(release, name);
    const bytes = await Deno.readFile(source);
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const pe = bytes.length >= 64 ? view.getUint32(0x3c, true) : 0;
    if (
      bytes.length < 64 || view.getUint16(0, true) !== 0x5a4d ||
      pe < 64 || pe > bytes.length - 26 ||
      view.getUint32(pe, true) !== 0x4550 ||
      view.getUint16(pe + 4, true) !== 0x8664 ||
      view.getUint16(pe + 24, true) !== 0x20b
    ) throw new Error(`Expected a Windows amd64 PE executable: ${source}`);
    await copyFile(source, resolve(directory, name));
  }
  await copyFile(resolve(root, "LICENSE"), resolve(directory, "LICENSE"));
  await copyFile(resolve(root, config.readme), resolve(directory, "README.md"));
  await Deno.writeTextFile(
    resolve(directory, "package.json"),
    JSON.stringify(
      {
        name: `natsume-${target}`,
        version,
        platform: "windows",
        architecture: "amd64",
        binaries,
      },
      null,
      2,
    ) + "\n",
  );
  const files = [...binaries, "LICENSE", "README.md", "package.json"].sort();
  const checksums = await Promise.all(
    files.map(async (name) =>
      `${await sha256(resolve(directory, name))}  ${name}\n`
    ),
  );
  await Deno.writeTextFile(
    resolve(directory, "SHA256SUMS"),
    checksums.join(""),
  );
}

export async function packageWindows(
  target: PackageTarget,
  version: string,
  release: string,
  output: string,
): Promise<void> {
  const temporary = await Deno.makeTempDir({
    dir: output,
    prefix: ".windows-",
  });
  try {
    const source = resolve(temporary, "contents");
    await stageWindowsPackage(target, version, release, source);
    const archive = resolve(temporary, "package.zip");
    const restored = resolve(temporary, "verified");
    // Paths are passed as environment values, never interpolated into PowerShell.
    await run("pwsh", [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "$ErrorActionPreference = 'Stop'; " +
      "Add-Type -AssemblyName System.IO.Compression.FileSystem; " +
      "[IO.Compression.ZipFile]::CreateFromDirectory($env:NATSUME_ZIP_SOURCE, $env:NATSUME_ZIP_OUTPUT); " +
      "[IO.Compression.ZipFile]::ExtractToDirectory($env:NATSUME_ZIP_OUTPUT, $env:NATSUME_ZIP_VERIFY)",
    ], {
      cwd: work,
      env: {
        NATSUME_ZIP_SOURCE: source,
        NATSUME_ZIP_OUTPUT: archive,
        NATSUME_ZIP_VERIFY: restored,
      },
    });
    const expected = [];
    for await (const entry of Deno.readDir(source)) expected.push(entry.name);
    const actual = [];
    for await (const entry of Deno.readDir(restored)) actual.push(entry.name);
    if (JSON.stringify(expected.sort()) !== JSON.stringify(actual.sort())) {
      throw new Error("Windows ZIP round-trip changed the file list");
    }
    for (const name of expected) {
      if (
        await sha256(resolve(source, name)) !==
          await sha256(resolve(restored, name))
      ) {
        throw new Error(`Windows ZIP round-trip changed ${name}`);
      }
    }
    await Deno.rename(
      archive,
      resolve(output, `natsume-${target}_${version}_windows_amd64.zip`),
    );
  } finally {
    await Deno.remove(temporary, { recursive: true });
  }
}
