import { resolve } from "@std/path";
import { parse as parseYaml } from "@std/yaml";
import {
  exists,
  root,
  sha256,
  targets,
  validateDenoVersion,
  work,
} from "./process.ts";

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

async function fixture() {
  const directory = await Deno.makeTempDir({
    dir: work,
    prefix: "build-test-",
  });
  for (
    const name of [
      "tools",
      "packaging/print-domjudge",
      "packaging/server",
      "packaging/client",
      "web",
      "target/build",
      "bin",
      "crates/example",
    ]
  ) {
    await Deno.mkdir(resolve(directory, name), { recursive: true });
  }
  for (
    const name of [
      "deno.json",
      "deno.lock",
      ".deno-version",
      "packaging/manifest.json",
      "tools/process.ts",
      "tools/assets.ts",
      "tools/build.ts",
      "tools/build_test.ts",
      "tools/package.ts",
      "packaging/server/nfpm.yaml",
      "packaging/client/nfpm.yaml",
      "packaging/print-domjudge/nfpm.yaml",
    ]
  ) {
    await Deno.copyFile(resolve(root, name), resolve(directory, name));
  }
  for (
    const name of [
      "Cargo.toml",
      "Cargo.lock",
      "crates/example/Cargo.toml",
      "rust-toolchain.toml",
      "web/package.json",
      "web/pnpm-lock.yaml",
      "web/pnpm-workspace.yaml",
      "web/.node-version",
      "packaging/print-domjudge/template.typ",
    ]
  ) {
    await Deno.writeTextFile(resolve(directory, name), "fixture\n");
  }
  await Deno.writeTextFile(
    resolve(directory, "probe.ts"),
    `
    import { recordPrepared, requirePrepared, targets } from "./tools/process.ts";
    if (Deno.args[0] === "record") {
      await recordPrepared([...targets]);
    } else {
      const stale = [];
      for (const target of targets) {
        try { await requirePrepared(target); }
        catch (error) {
          if (!(error instanceof Error) || !error.message.startsWith("Run deno task prepare")) throw error;
          stale.push(target);
        }
      }
      console.log(JSON.stringify(stale));
    }
  `,
  );
  const env = { PATH: `${resolve(directory, "bin")}:${Deno.env.get("PATH")}` };
  const command = (file: string, ...args: string[]) =>
    new Deno.Command(Deno.execPath(), {
      args: [
        "run",
        "--cached-only",
        "--frozen",
        "--no-prompt",
        "--allow-read",
        "--allow-write",
        "--allow-env",
        "--allow-run",
        "--deny-net",
        file,
        ...args,
      ],
      cwd: directory,
      env,
      stdout: "piped",
      stderr: "piped",
    }).output();
  const prepared = await command("probe.ts", "record");
  assert(prepared.success, new TextDecoder().decode(prepared.stderr));
  return { directory, command };
}

Deno.test("Deno baseline accepts the tested version and newer stable versions in the same major", () => {
  for (const version of ["2.9.8", "2.9.9", "2.10.0", "2.10.1"]) {
    validateDenoVersion("2.9.8", version);
  }
});

Deno.test("Deno baseline rejects older, prerelease and unvalidated major versions", () => {
  for (const version of ["2.7.6", "2.9.7", "2.9.8-rc.1", "3.0.0"]) {
    let failure;
    try {
      validateDenoVersion("2.9.8", version);
    } catch (error) {
      failure = error;
    }
    assert(failure instanceof Error, `${version} was accepted`);
    const message = (failure as Error).message;
    assert(message.includes(version) && message.includes("2.9.8"), message);
    assert(message.includes(Deno.execPath()), message);
    if (version === "3.0.0") {
      assert(message.includes("has not been validated"), message);
    }
  }
});

Deno.test("an invalid Deno baseline identifies the repository file", () => {
  let failure;
  try {
    validateDenoVersion("latest", "2.9.8");
  } catch (error) {
    failure = error;
  }
  assert(
    failure instanceof Error &&
      failure.message.includes("Invalid .deno-version"),
    "Invalid baseline did not identify the file",
  );
});

Deno.test("version mismatch reports the runtime, executable and stopped preparation", async () => {
  const { directory, command } = await fixture();
  try {
    const [major, minor, patch] = Deno.version.deno.split(".").map(Number);
    const required = `${major}.${minor}.${patch + 1}`;
    await Deno.writeTextFile(
      resolve(directory, ".deno-version"),
      `${required}\n`,
    );
    const result = await command("tools/assets.ts", "rust");
    const stderr = new TextDecoder().decode(result.stderr);
    assert(result.code === 1, "Version mismatch must fail preparation");
    for (
      const expected of [
        "[prepare] FAILED",
        Deno.version.deno,
        required,
        Deno.execPath(),
        "No task commands were executed",
        "Do not continue with offline checks",
      ]
    ) {
      assert(
        stderr.includes(expected),
        `Missing diagnostic ${expected}: ${stderr}`,
      );
    }
    assert(
      !new TextDecoder().decode(result.stdout).includes("cargo fetch"),
      "Cargo ran after the version check failed",
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("preparation errors preserve the failed command and invalidate readiness", async () => {
  const { directory, command } = await fixture();
  try {
    const cargo = resolve(directory, "bin/cargo");
    await Deno.writeTextFile(
      cargo,
      "#!/bin/sh\nprintf '%s\\n' 'fixture: rsproxy-sparse cannot resolve the locked package' >&2\nexit 23\n",
    );
    await Deno.chmod(cargo, 0o755);
    const result = await command("tools/assets.ts", "rust");
    const stderr = new TextDecoder().decode(result.stderr);
    assert(result.code === 1, "Cargo failure must fail preparation");
    for (
      const expected of [
        "[prepare] FAILED",
        "Rust dependency preparation failed",
        "cargo fetch --locked",
        "exit 23",
        directory,
        "rsproxy-sparse",
        "Cargo.lock",
        "Do not continue with offline checks",
      ]
    ) {
      assert(
        stderr.includes(expected),
        `Missing diagnostic ${expected}: ${stderr}`,
      );
    }
    for (const target of ["scripts", "rust"]) {
      assert(
        !await exists(resolve(directory, `target/build/prepared-${target}`)),
        `Failed prepare left ${target} ready`,
      );
    }
    assert(
      await exists(resolve(directory, "target/build/prepared-web")),
      "Unrelated Web preparation was removed",
    );
    assert(
      !new TextDecoder().decode(result.stdout).includes("Prepared:"),
      "Failed preparation reported success",
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("missing commands report the tool, working directory and PATH guidance", async () => {
  const { directory, command } = await fixture();
  try {
    await Deno.writeTextFile(
      resolve(directory, "missing-tool.ts"),
      `
      import { main, run } from "./tools/process.ts";
      await main(async () => { await run("natsume-missing-tool-fixture", []); }, "probe");
    `,
    );
    const result = await command("missing-tool.ts");
    const stderr = new TextDecoder().decode(result.stderr);
    assert(result.code === 1, "Missing command must fail the task");
    for (
      const expected of [
        "[probe] FAILED",
        "Command not found",
        "natsume-missing-tool-fixture",
        directory,
        "PATH",
      ]
    ) {
      assert(
        stderr.includes(expected),
        `Missing diagnostic ${expected}: ${stderr}`,
      );
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("preparation fingerprints invalidate only consumers of changed inputs", async () => {
  const { directory, command } = await fixture();
  try {
    const manifestPath = resolve(directory, "packaging/manifest.json");
    const manifest = await Deno.readTextFile(manifestPath);
    const cases: [string, string[]][] = [
      ["typst", ["print-domjudge"]],
      ["font", ["print-domjudge"]],
      ["font_license", ["print-domjudge"]],
      ["caddy", ["client"]],
      ["nfpm", ["server", "client", "print-domjudge"]],
      ["skia", ["rust", "client"]],
    ];
    for (const [asset, expected] of cases) {
      const changed = JSON.parse(manifest);
      changed[asset].sha256 = "changed";
      await Deno.writeTextFile(manifestPath, JSON.stringify(changed));
      const result = await command("probe.ts");
      assert(result.success, new TextDecoder().decode(result.stderr));
      const actual = new TextDecoder().decode(result.stdout).trim();
      assert(actual === JSON.stringify(expected), `${asset}: ${actual}`);
    }
    await Deno.writeTextFile(manifestPath, manifest);
    for (
      const [file, expected] of [
        ["web/pnpm-lock.yaml", ["web"]],
        ["Cargo.lock", ["rust"]],
        ["crates/example/Cargo.toml", ["rust"]],
        ["packaging/print-domjudge/template.typ", ["print-domjudge"]],
        [".deno-version", [...targets]],
      ] as const
    ) {
      const path = resolve(directory, file);
      const original = await Deno.readTextFile(path);
      await Deno.writeTextFile(path, "changed\n");
      const result = await command("probe.ts");
      assert(result.success, new TextDecoder().decode(result.stderr));
      const actual = new TextDecoder().decode(result.stdout).trim();
      assert(actual === JSON.stringify(expected), `${file}: ${actual}`);
      await Deno.writeTextFile(path, original);
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("API checks compare without Git and never overwrite the working file", async () => {
  const { directory, command } = await fixture();
  try {
    await Deno.mkdir(resolve(directory, "web/src/api/generated"), {
      recursive: true,
    });
    const output = resolve(directory, "web/src/api/generated/types.ts");
    const generated = "export type Example = string;\n";
    const cargo = resolve(directory, "bin/cargo");
    await Deno.writeTextFile(
      cargo,
      `#!/bin/sh
for output do :; done
printf '%s\\n' 'export type Example = string;' > "$output"
`,
    );
    await Deno.chmod(cargo, 0o755);
    const pnpm = resolve(directory, "bin/pnpm");
    await Deno.writeTextFile(pnpm, "#!/bin/sh\nexit 0\n");
    await Deno.chmod(pnpm, 0o755);
    await Deno.writeTextFile(output, generated);
    const matching = await command("tools/build.ts", "api", "--check");
    assert(matching.success, new TextDecoder().decode(matching.stderr));
    const edited = "// local change\n" + generated;
    await Deno.writeTextFile(output, edited);
    const stale = await command("tools/build.ts", "api", "--check");
    assert(!stale.success, "A changed working file must fail the check");
    assert(
      await Deno.readTextFile(output) === edited,
      "Check overwrote the working file",
    );
    const refreshed = await command("tools/build.ts", "api");
    assert(refreshed.success, new TextDecoder().decode(refreshed.stderr));
    assert(
      await Deno.readTextFile(output) === generated,
      "Generation did not update the file",
    );
    const leftovers = [];
    for await (
      const entry of Deno.readDir(resolve(directory, "target/build"))
    ) {
      if (entry.name.startsWith("api-")) leftovers.push(entry.name);
    }
    assert(leftovers.length === 0, "API checks left temporary output behind");
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("Rust checks invoke Clippy once without a separate Cargo check", async () => {
  const { directory, command } = await fixture();
  try {
    for (const name of ["cargo", "pnpm"]) {
      const path = resolve(directory, "bin", name);
      await Deno.writeTextFile(
        path,
        `#!/bin/sh\nprintf '%s\\n' '${name} '"$*" >> calls.log\n`,
      );
      await Deno.chmod(path, 0o755);
    }
    const result = await command("tools/build.ts", "check", "server");
    assert(result.success, new TextDecoder().decode(result.stderr));
    const calls = await Deno.readTextFile(resolve(directory, "calls.log"));
    assert(
      calls ===
        "cargo clippy -p natsume-server --all-targets --all-features --frozen -- -D warnings\n",
      `Unexpected Rust commands: ${calls}`,
    );
    const webCalls = await Deno.readTextFile(
      resolve(directory, "web/calls.log"),
    );
    assert(
      webCalls === "pnpm typecheck\n",
      `Unexpected Web commands: ${webCalls}`,
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("all package manifests preserve explicit Debian versions", async () => {
  const { directory, command } = await fixture();
  try {
    await Deno.writeTextFile(
      resolve(directory, "Cargo.toml"),
      '[workspace.package]\nversion = "2.10.3"\n',
    );
    await Deno.mkdir(resolve(directory, "target/build/tools"), {
      recursive: true,
    });
    const nfpm = resolve(directory, "target/build/tools/nfpm");
    await Deno.writeTextFile(
      nfpm,
      `#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    --config) config=$2; shift 2 ;;
    *) shift ;;
  esac
done
name=$(awk '$1 == "name:" { print $2; exit }' "$config")
cp "$config" "rendered-$name.yaml"
`,
    );
    await Deno.chmod(nfpm, 0o755);
    const shlibdeps = resolve(directory, "bin/dpkg-shlibdeps");
    await Deno.writeTextFile(
      shlibdeps,
      "#!/bin/sh\nprintf 'shlibs:Depends=libc6\\n'\n",
    );
    await Deno.chmod(shlibdeps, 0o755);
    for (
      const name of [
        "target/release/natsume-print-domjudge",
        "target/print-domjudge/runtime/typst",
      ]
    ) {
      const path = resolve(directory, name);
      await Deno.mkdir(resolve(path, ".."), { recursive: true });
      await Deno.writeTextFile(path, "fixture");
    }
    for (const version of ["2.10.3-1", "2.10.3~rc.1", "1:2.10.3-1"]) {
      const result = await command(
        "tools/package.ts",
        "all",
        "--version",
        version,
      );
      assert(result.success, new TextDecoder().decode(result.stderr));
      for (const name of ["server", "client", "print-domjudge"]) {
        const config = parseYaml(
          await Deno.readTextFile(
            resolve(directory, `rendered-natsume-${name}.yaml`),
          ),
        ) as {
          version: string;
          version_schema: string;
        };
        assert(config.version === version, `${name}: version changed`);
        assert(
          config.version_schema === "none",
          `${name}: nFPM must not normalize Debian versions`,
        );
      }
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("Client notices and renamed font caches prepare the installed inputs", async () => {
  const { directory, command } = await fixture();
  try {
    const manifestPath = resolve(directory, "packaging/manifest.json");
    const manifest = JSON.parse(await Deno.readTextFile(manifestPath));
    const downloads = resolve(directory, "target/build/downloads");
    await Deno.mkdir(downloads, { recursive: true });
    const archive = async (
      name: "caddy" | "skia" | "typst",
      files: Record<string, string>,
    ) => {
      const source = resolve(directory, "archives", name);
      for (const [member, content] of Object.entries(files)) {
        const path = resolve(source, member);
        await Deno.mkdir(resolve(path, ".."), { recursive: true });
        await Deno.writeTextFile(path, content);
      }
      const path = resolve(downloads, `${name}.tar`);
      const result = await new Deno.Command("tar", {
        args: ["-cf", path, "-C", source, ...Object.keys(files)],
        stdout: "piped",
        stderr: "piped",
      }).output();
      assert(result.success, new TextDecoder().decode(result.stderr));
      manifest[name].filename = `${name}.tar`;
      manifest[name].sha256 = await sha256(path);
    };
    await archive("caddy", { LICENSE: "Caddy license fixture\n" });
    await archive("skia", {
      "skia-binaries/LICENSE_SKIA": "Skia license fixture\n",
    });
    await archive("typst", {
      "typst-x86_64-unknown-linux-musl/typst": "compiler fixture\n",
      "typst-x86_64-unknown-linux-musl/LICENSE": "Typst license fixture\n",
      "typst-x86_64-unknown-linux-musl/NOTICE": "Typst notice fixture\n",
    });
    manifest.typst.binary_sha256 = await sha256(
      resolve(
        directory,
        "archives/typst/typst-x86_64-unknown-linux-musl/typst",
      ),
    );
    const font = resolve(downloads, "renamed-font-cache.otf");
    await Deno.writeTextFile(font, "new font fixture\n");
    manifest.font.filename = "renamed-font-cache.otf";
    manifest.font.sha256 = await sha256(font);
    const fontLicense = resolve(downloads, "font-license.txt");
    await Deno.writeTextFile(fontLicense, "font license fixture\n");
    manifest.font_license.filename = "font-license.txt";
    manifest.font_license.sha256 = await sha256(fontLicense);
    await Deno.writeTextFile(manifestPath, JSON.stringify(manifest));
    const fonts = resolve(directory, "target/print-domjudge/runtime/fonts");
    await Deno.mkdir(fonts, { recursive: true });
    const installedFont = resolve(fonts, "NotoSansMonoCJKsc-Regular.otf");
    await Deno.writeTextFile(installedFont, "stale font fixture\n");
    const curl = resolve(directory, "bin/curl");
    await Deno.writeTextFile(curl, "#!/bin/sh\nexit 99\n");
    await Deno.chmod(curl, 0o755);
    await Deno.writeTextFile(
      resolve(directory, "asset-probe.ts"),
      `
      import { clientLicenses, printAssets } from "./tools/assets.ts";
      await clientLicenses();
      await printAssets();
    `,
    );
    const result = await command("asset-probe.ts");
    assert(result.success, new TextDecoder().decode(result.stderr));
    for (
      const [file, expected] of [
        [
          "target/build/licenses/client/Caddy-LICENSE",
          "Caddy license fixture\n",
        ],
        ["target/build/licenses/client/Skia-LICENSE", "Skia license fixture\n"],
      ]
    ) {
      const path = resolve(directory, file);
      assert(
        await Deno.readTextFile(path) === expected,
        `${file}: wrong notice`,
      );
      assert(
        (((await Deno.stat(path)).mode ?? 0) & 0o777) === 0o644,
        `${file}: wrong mode`,
      );
    }
    assert(
      await Deno.readTextFile(installedFont) === "new font fixture\n",
      "The prepared font was not replaced",
    );
    assert(
      !await exists(resolve(fonts, "renamed-font-cache.otf")),
      "The cache filename became a runtime path",
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});
