import { main, packageTargets, run, sha256 } from "./process.ts";

function required(name: string): string {
  const value = Deno.env.get(name);
  if (!value) throw new Error(`Missing release input: ${name}`);
  return value;
}

export function releaseVersions(tag: string) {
  const number = "(?:0|[1-9][0-9]*)";
  const match = new RegExp(
    `^v(${number}\\.${number}\\.${number})(?:-([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$`,
  ).exec(tag);
  if (
    !match || match[0] !== tag ||
    match[2]?.split(".").some((part) => /^0[0-9]+$/.test(part))
  ) {
    throw new Error(
      "Expected vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-prerelease without leading zeroes",
    );
  }
  return {
    package_version: match[1] + (match[2] ? "~" + match[2] : ""),
    release_version: tag.slice(1),
    prerelease: String(match[2] !== undefined),
  };
}

async function publish(directory: string): Promise<void> {
  const versions = releaseVersions(required("RELEASE_TAG"));
  for (
    const name of ["package_version", "release_version", "prerelease"] as const
  ) {
    if (required(name.toUpperCase()) !== versions[name]) {
      throw new Error(`Release ${name} does not match the tag`);
    }
  }
  const assets: string[] = [];
  for (const [target, config] of Object.entries(packageTargets)) {
    const name = `natsume-${target}`;
    let asset: string;
    if (config.format === "deb") {
      asset = `${name}_${versions.package_version}_amd64.deb`;
      for (
        const [field, expected] of [
          ["Package", name],
          ["Version", versions.package_version],
          ["Architecture", "amd64"],
        ]
      ) {
        const actual = await run("dpkg-deb", ["--field", asset, field], {
          cwd: directory,
          capture: true,
        });
        if (actual.trim() !== expected) {
          throw new Error(`Debian ${field} mismatch: ${asset}`);
        }
      }
    } else {
      asset = `${name}_${versions.release_version}_windows_amd64.zip`;
      await run("unzip", ["-t", asset], { cwd: directory });
      const metadata = JSON.parse(
        await run("unzip", ["-p", asset, "package.json"], {
          cwd: directory,
          capture: true,
        }),
      );
      if (
        metadata.name !== name ||
        metadata.version !== versions.release_version ||
        metadata.platform !== "windows" || metadata.architecture !== "amd64"
      ) {
        throw new Error(`Windows package metadata mismatch: ${asset}`);
      }
    }
    assets.push(asset);
  }
  const checksums = [];
  for (const asset of assets) {
    checksums.push(`${await sha256(`${directory}/${asset}`)}  ${asset}\n`);
  }
  await Deno.writeTextFile(`${directory}/SHA256SUMS`, checksums.join(""));
  await run("gh", [
    "release",
    "create",
    required("RELEASE_TAG"),
    ...assets,
    "SHA256SUMS",
    "--verify-tag",
    "--generate-notes",
    "--title",
    required("RELEASE_TAG"),
    ...versions.prerelease === "true" ? ["--prerelease", "--latest=false"] : [],
  ], { cwd: directory });
}

async function release(): Promise<void> {
  const [command, ...args] = Deno.args;
  if (command === "version" && !args.length) {
    const values = releaseVersions(required("RELEASE_TAG"));
    await Deno.writeTextFile(
      required("GITHUB_OUTPUT"),
      Object.entries(values).map(([name, value]) => `${name}=${value}\n`).join(
        "",
      ),
      { append: true },
    );
  } else if (command === "publish" && args.length === 1) {
    await publish(args[0]);
  } else {
    throw new Error("Expected version or publish <package-directory>");
  }
}

if (import.meta.main) await main(release, "release");
