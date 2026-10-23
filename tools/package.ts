import { parse as parseToml } from "@std/toml";
import { parse as parseYaml, stringify } from "@std/yaml";
import { basename, resolve } from "@std/path";
import { parseArgs } from "@std/cli/parse-args";
import {
  isPackageTarget,
  main,
  requirePrepared,
  root,
  run,
  runtime,
  selection,
  work,
} from "./process.ts";

async function dependencies(binaries: string[]): Promise<string[]> {
  const directory = await Deno.makeTempDir({ dir: work });
  try {
    await Deno.mkdir(
      resolve(directory, "debian/natsume-print-domjudge/DEBIAN"),
      { recursive: true },
    );
    await Deno.writeTextFile(
      resolve(directory, "debian/control"),
      "Source: natsume-print-domjudge\nSection: utils\nPriority: optional\nMaintainer: Natsume Project\n\nPackage: natsume-print-domjudge\nArchitecture: amd64\nDescription: DOMjudge wrapper\n",
    );
    const args = ["-O"];
    for (const binary of binaries) {
      const relative = `debian/natsume-print-domjudge/usr/bin/${
        basename(binary)
      }`;
      const staged = resolve(directory, relative);
      await Deno.mkdir(resolve(staged, ".."), { recursive: true });
      await Deno.copyFile(binary, staged);
      await Deno.chmod(staged, 0o755);
      args.push("-e", relative);
    }
    const result = await run("dpkg-shlibdeps", args, {
      cwd: directory,
      capture: true,
    });
    if (!result.startsWith("shlibs:Depends=")) {
      throw new Error("Unexpected dpkg-shlibdeps output");
    }
    return result.slice("shlibs:Depends=".length).trim().split(", ").filter(
      Boolean,
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

async function packages(): Promise<void> {
  const values = parseArgs(Deno.args, {
    string: ["output", "version"],
    boolean: ["ci"],
    unknown: (argument) => {
      if (argument.startsWith("-")) {
        throw new Error(`Unknown option: ${argument}`);
      }
      return true;
    },
  });
  const requested = selection(values._.map(String));
  if (
    values._.some((value) => value !== "all") &&
    requested.some((target) => !isPackageTarget(target))
  ) {
    throw new Error("Expected server, client, print-domjudge or all");
  }
  const selected = requested.filter(isPackageTarget);
  if (!selected.length) {
    throw new Error("Expected server, client, print-domjudge or all");
  }
  if (
    Deno.build.os !== "linux" || Deno.build.arch !== "x86_64" ||
    (Deno.env.get("ARCH") ?? "amd64") !== "amd64"
  ) throw new Error("Package on Linux amd64; cross packaging is not supported");
  await requirePrepared("scripts", ...selected);
  const workspace = parseToml(
    await Deno.readTextFile(resolve(root, "Cargo.toml")),
  ) as { workspace: { package: { version: string } } };
  const version = values.version ?? Deno.env.get("VERSION") ??
    `${workspace.workspace.package.version}${values.ci ? "~ci1" : ""}`;
  if (!/^[0-9][A-Za-z0-9.+:~\-]*$/.test(version)) {
    throw new Error("VERSION must be a nonempty Debian package version");
  }
  const prebuilt = Deno.env.get("RUST_RELEASE_DIR");
  if (prebuilt === "") throw new Error("RUST_RELEASE_DIR must not be empty");
  const release = prebuilt === undefined
    ? resolve(root, Deno.env.get("CARGO_TARGET_DIR") ?? "target", "release")
    : resolve(root, prebuilt);
  const output = resolve(
    root,
    values.output ?? Deno.env.get("NATSUME_PACKAGE_OUTPUT") ?? "dist/packages",
  );
  await Deno.mkdir(output, { recursive: true });
  const variables: Record<string, string> = {
    ARCH: "amd64",
    VERSION: version,
    RUST_RELEASE_DIR: release,
    CADDY_BIN: resolve(work, "tools/caddy"),
    CLIENT_LICENSES: resolve(work, "licenses/client"),
    AUTHOR_AVATAR_FILE: resolve(work, "author-avatar.png"),
    WRAPPER_BIN: resolve(release, "natsume-print-domjudge"),
    RUNTIME_ASSETS: runtime,
  };
  for (const target of selected) {
    type Config = {
      arch: string;
      version: string;
      contents: { src: string }[];
      depends?: string[];
    };
    const config = parseYaml(
      await Deno.readTextFile(resolve(root, `packaging/${target}/nfpm.yaml`)),
    ) as Config;
    config.arch = "amd64";
    config.version = version;
    for (const content of config.contents) {
      content.src = content.src.replace(
        /\$\{([A-Z_]+)\}/g,
        (_, name: string) => {
          if (!(name in variables)) {
            throw new Error(`Unknown package input: ${name}`);
          }
          return variables[name];
        },
      );
    }
    if (target === "print-domjudge") {
      config.depends = [
        ...(config.depends ?? []),
        ...await dependencies([
          variables.WRAPPER_BIN,
          resolve(runtime, "typst"),
        ]),
      ];
    }
    const directory = await Deno.makeTempDir({ dir: work });
    try {
      const path = resolve(directory, "nfpm.yaml");
      await Deno.writeTextFile(path, stringify(config));
      await run(resolve(work, "tools/nfpm"), [
        "package",
        "--packager",
        "deb",
        "--config",
        path,
        "--target",
        output + "/",
      ]);
    } finally {
      await Deno.remove(directory, { recursive: true });
    }
  }
  console.log(`Packages: ${output} (version ${version})`);
}

if (import.meta.main) await main(packages, "package");
