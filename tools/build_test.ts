import { resolve } from "@std/path";
import { parse as parseYaml } from "@std/yaml";
import {
  exists,
  packageTargets,
  root,
  sha256,
  targets,
  validateDenoVersion,
  work,
} from "./process.ts";

function assert(condition: boolean, message: string): asserts condition {
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
      "tools/windows.ts",
      "tools/windows_test.ts",
      "tools/entry.ts",
      "tools/native-deps.ts",
      "tools/release.ts",
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
  const env: Record<string, string> = {
    PATH: `${resolve(directory, "bin")}:${Deno.env.get("PATH")}`,
    CARGO_TARGET_DIR: "target",
    GITHUB_OUTPUT: resolve(directory, "outputs"),
  };
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
  return { directory, command, env };
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

Deno.test("module checks invoke Clippy once and leave Web validation to its own target", async () => {
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
        "cargo clippy -p natsume-server -p natsume-server-setup --all-targets --all-features --frozen -- -D warnings\n",
      `Unexpected Rust commands: ${calls}`,
    );
    const webLog = resolve(directory, "web/calls.log");
    assert(!await exists(webLog), "Server repeated the Web compile check");
    const tested = await command("tools/build.ts", "test", "server");
    assert(tested.success, new TextDecoder().decode(tested.stderr));
    assert(
      await Deno.readTextFile(resolve(directory, "calls.log")) ===
        `${calls}cargo test -p natsume-server -p natsume-server-setup --all-features --frozen\n`,
      "Server tests omitted a crate or repeated a Rust command",
    );
    assert(!await exists(webLog), "Server repeated Web unit tests");
    const web = await command("tools/build.ts", "check", "web");
    assert(web.success, new TextDecoder().decode(web.stderr));
    assert(
      await Deno.readTextFile(webLog) === "pnpm typecheck\n",
      "Web target lost its compile check",
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("Linux Helper tests use nextest isolation and require its installation", async () => {
  const { directory, command, env } = await fixture();
  try {
    const manifestPath = resolve(directory, "packaging/manifest.json");
    const manifest = JSON.parse(await Deno.readTextFile(manifestPath));
    const archive = resolve(
      directory,
      "target/build/downloads",
      manifest.skia.filename,
    );
    await Deno.mkdir(resolve(archive, ".."), { recursive: true });
    await Deno.writeTextFile(archive, "Skia fixture");
    manifest.skia.sha256 = await sha256(archive);
    await Deno.writeTextFile(manifestPath, JSON.stringify(manifest));
    await Deno.writeTextFile(
      resolve(directory, "Cargo.lock"),
      `name = "skia-bindings"\nversion = "${manifest.skia.version}"\n`,
    );
    const prepared = await command("probe.ts", "record");
    assert(prepared.success, new TextDecoder().decode(prepared.stderr));
    const cargo = resolve(directory, "bin/cargo");
    await Deno.writeTextFile(
      cargo,
      `#!/bin/sh
printf '%s\\n' "$*|$CARGO_NET_OFFLINE|$SKIA_BINARIES_URL" >> calls.log
if [ "$1 $2" = 'nextest --version' ] && [ "$NATSUME_TEST_MISSING_NEXTEST" = '1' ]; then exit 101; fi
`,
    );
    await Deno.chmod(cargo, 0o755);
    const calls = resolve(directory, "calls.log");
    const skia =
      `file://${directory}/target/build/downloads/skia-binaries-{key}.tar.gz`;
    const helper = "natsume-workstation-helper";
    for (
      const target of ["rust", "client", "server", "shared", "print-domjudge"]
    ) {
      await Deno.writeTextFile(calls, "");
      const result = await command("tools/build.ts", "test", target);
      assert(result.success, new TextDecoder().decode(result.stderr));
      const native = target === "rust" || target === "client";
      const packages = target === "rust"
        ? `--workspace --exclude ${helper}`
        : (target === "shared"
          ? [
            "natsume-photo-client",
            "natsume-photo-protocol",
            "natsume-print-client",
          ]
          : packageTargets[target as keyof typeof packageTargets].crates)
          .filter((name) => name !== helper)
          .map((name) => `-p ${name}`).join(" ");
      const expected = [
        ...(native ? ["nextest --version"] : []),
        `test ${packages} --all-features --frozen`,
        ...(native ? [`nextest run -p ${helper} --all-features --frozen`] : []),
      ].map((call) => `${call}|true|${native ? skia : ""}\n`).join("");
      const actual = await Deno.readTextFile(calls);
      assert(
        actual === expected,
        `${target}: unexpected Cargo calls: ${actual}`,
      );
    }
    env.NATSUME_TEST_MISSING_NEXTEST = "1";
    for (const target of ["rust", "client"]) {
      await Deno.writeTextFile(calls, "");
      const result = await command("tools/build.ts", "test", target);
      const stderr = new TextDecoder().decode(result.stderr);
      assert(!result.success, `${target}: missing nextest was accepted`);
      assert(
        stderr.includes(
          "cargo install cargo-nextest --version 0.9.150 --locked",
        ),
        stderr,
      );
      assert(
        await Deno.readTextFile(calls) === `nextest --version|true|${skia}\n`,
        `${target}: missing nextest fell back to cargo test`,
      );
    }
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
      `#!/bin/sh
printf '%s\\n' "$@" >> "${directory}/shlibdeps-args.log"
printf 'shlibs:Depends=libc6\\n'
`,
    );
    await Deno.chmod(shlibdeps, 0o755);
    for (
      const name of [
        ...packageTargets.server.binaries.map((name) =>
          `target/release/${name}`
        ),
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
          depends?: string[];
          contents: {
            src: string;
            dst: string;
            file_info?: { mode: string | number };
          }[];
        };
        assert(config.version === version, `${name}: version changed`);
        if (name === "server") {
          assert(
            config.depends?.includes("libc6") === true,
            "Server dependencies omitted",
          );
          const setup = config.contents.find((entry) =>
            entry.dst === "/usr/bin/natsume-server-setup"
          );
          assert(
            Number(setup?.file_info?.mode) === 0o755,
            "Setup executable missing or not 0755",
          );
          const scanned = await Deno.readTextFile(
            resolve(directory, "shlibdeps-args.log"),
          );
          for (const binary of ["natsume-server", "natsume-server-setup"]) {
            assert(
              scanned.includes(`/usr/bin/${binary}\n`),
              `Dependency scan omitted ${binary}`,
            );
          }
        }
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

Deno.test("package tests retain Cargo documentation tests and require their native host", async () => {
  const { directory, command } = await fixture();
  try {
    const cargo = resolve(directory, "bin/cargo");
    await Deno.writeTextFile(
      cargo,
      '#!/bin/sh\nprintf "%s\\n" "$*" > calls.log\n',
    );
    await Deno.chmod(cargo, 0o755);
    const result = await command("tools/build.ts", "test", "print-domjudge");
    assert(result.success, new TextDecoder().decode(result.stderr));
    assert(
      await Deno.readTextFile(resolve(directory, "calls.log")) ===
        "test -p natsume-print-domjudge --all-features --frozen\n",
      "Tests must use frozen dependencies without restricting Cargo's default test kinds",
    );
    const unsupported = await command("tools/assets.ts", "print-client");
    assert(
      !unsupported.success,
      "Linux accepted a Windows package preparation request",
    );
    assert(
      new TextDecoder().decode(unsupported.stderr).includes("requires windows"),
      "Cross-platform failure did not identify the required host",
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("native prerequisites bootstrap with an empty Deno cache and reject command flags", async () => {
  const { directory, command, env } = await fixture();
  try {
    env.DENO_DIR = resolve(directory, "empty-deno-cache");
    const marker = resolve(directory, "target/build/prepared-scripts");
    await Deno.remove(marker);
    const sudo = resolve(directory, "bin/sudo");
    await Deno.writeTextFile(
      sudo,
      '#!/bin/sh\nprintf "%s\\n" "$*" >> calls.log\n',
    );
    await Deno.chmod(sudo, 0o755);
    const result = await command("tools/native-deps.ts", "curl", "pkg-config");
    assert(result.success, new TextDecoder().decode(result.stderr));
    const calls = await Deno.readTextFile(resolve(directory, "calls.log"));
    assert(
      calls ===
        "apt-get update\nenv DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends curl pkg-config\n",
      calls,
    );
    const invalid = await command("tools/native-deps.ts", "--help");
    assert(!invalid.success, "Accepted an apt-get flag as a package");
    assert(
      await Deno.readTextFile(resolve(directory, "calls.log")) === calls,
      "Invalid prerequisites invoked sudo",
    );
    assert(
      !await exists(marker),
      "Bootstrap treated a cache as prepared inputs",
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("shared check preparation fetches Rust without Linux delivery assets", async () => {
  const { directory, command } = await fixture();
  try {
    const cargo = resolve(directory, "bin/cargo");
    await Deno.writeTextFile(
      cargo,
      '#!/bin/sh\nprintf "%s\\n" "$*" > calls.log\n',
    );
    await Deno.chmod(cargo, 0o755);
    const result = await command("tools/assets.ts", "shared");
    assert(result.success, new TextDecoder().decode(result.stderr));
    assert(
      await Deno.readTextFile(resolve(directory, "calls.log")) ===
        "fetch --locked\n",
      "Unexpected preparation commands",
    );
    for (const name of ["downloads", "tools/nfpm", "author-avatar.png"]) {
      assert(
        !await exists(resolve(directory, "target/build", name)),
        `Shared checks prepared delivery asset: ${name}`,
      );
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("CI checks both Rust platforms before its native delivery matrix and only caches Rust compilation", async () => {
  type Step = {
    name?: string;
    run?: string;
    uses?: string;
    if?: string;
    with?: Record<string, unknown>;
  };
  const ci = parseYaml(
    await Deno.readTextFile(resolve(root, ".github/workflows/ci.yml")),
  ) as {
    jobs: Record<
      string,
      {
        steps?: Step[];
        needs?: string[];
        if?: string;
        "continue-on-error"?: boolean;
        strategy?: {
          "fail-fast": boolean;
          matrix: { include?: { module: string; os: string }[]; os?: string[] };
        };
        "runs-on"?: string;
      }
    >;
  };
  assert(
    ci.jobs.checks["runs-on"] === "${{ matrix.os }}" &&
      JSON.stringify(ci.jobs.checks.strategy?.matrix.os?.toSorted()) ===
        JSON.stringify(["ubuntu-24.04", "windows-2022"]),
    "Shared checks must run natively on both Linux and Windows",
  );
  const checkSteps = ci.jobs.checks.steps ?? [];
  assert(
    checkSteps.some((step) =>
      step.uses === "./.github/actions/prepare-build" &&
      step.with?.module === "rust" &&
      step.with?.runner === "${{ matrix.os }}"
    ),
    "Shared checks must prepare and cache the workspace for their native runner",
  );
  for (const command of ["deno task check rust", "deno task test rust"]) {
    assert(
      checkSteps.filter((step) => step.run === command && !step.if).length ===
        1,
      `Each platform must run the workspace command once: ${command}`,
    );
  }
  for (
    const command of [
      "deno task check scripts",
      "deno task test scripts",
      "cargo install cargo-nextest --version 0.9.150 --locked",
      "cargo install cargo-deny --version 0.20.2 --locked",
      "cargo deny check",
    ]
  ) {
    assert(
      checkSteps.some((step) =>
        step.run === command && step.if === "runner.os == 'Linux'"
      ),
      `Linux-only validation must not be duplicated on Windows: ${command}`,
    );
  }
  const nextestInstall = checkSteps.findIndex((step) =>
    step.run === "cargo install cargo-nextest --version 0.9.150 --locked"
  );
  assert(
    nextestInstall >= 0 &&
      nextestInstall <
        checkSteps.findIndex((step) => step.run === "deno task test rust"),
    "Linux must install pinned nextest before workspace tests",
  );
  assert(
    checkSteps.some((step) =>
      step.run?.includes("tools/windows_test.ts") &&
      step.if === "runner.os == 'Windows'"
    ),
    "Shared checks must validate Windows staging on Windows",
  );
  assert(
    !ci.jobs.modules.steps?.some((step) =>
      /^(deno task (check|test) |deno test |cargo (clippy|test) )/.test(
        step.run ?? "",
      )
    ),
    "Module delivery must not repeat centralized checks and tests",
  );
  assert(
    ci.jobs.modules["runs-on"] === "${{ matrix.os }}",
    "Module runners are not matrix-selected",
  );
  assert(
    JSON.stringify(ci.jobs.modules.needs?.toSorted()) ===
      JSON.stringify(["checks", "web"]),
    "Module delivery must wait for all shared and Web checks",
  );
  assert(
    ci.jobs.modules.if === undefined &&
      ci.jobs.modules["continue-on-error"] !== true,
    "Module delivery bypasses a failed prerequisite",
  );
  for (const name of ["checks", "web"]) {
    assert(
      ci.jobs[name]["continue-on-error"] !== true,
      `Failed gate is ignored: ${name}`,
    );
  }
  assert(
    ci.jobs.modules.strategy?.["fail-fast"] === true,
    "A failed module must cancel remaining matrix work",
  );
  const entries = ci.jobs.modules.strategy?.matrix.include;
  assert(Array.isArray(entries), "Modules must use native YAML matrix.include");
  assert(
    entries.length === Object.keys(packageTargets).length &&
      new Set(entries.map((entry) => entry.module)).size === entries.length,
    "Matrix module ownership differs from package definitions",
  );
  for (const [name, config] of Object.entries(packageTargets)) {
    const entry = entries.find((entry) => entry.module === name);
    assert(
      entry?.os ===
        (config.platform === "linux" ? "ubuntu-24.04" : "windows-2022"),
      `Wrong native runner: ${name}`,
    );
  }
  for (const name of ["plan", "rust", "windows", "windows-print", "packages"]) {
    assert(!(name in ci.jobs), `Legacy aggregate job remains: ${name}`);
  }
  const release = parseYaml(
    await Deno.readTextFile(resolve(root, ".github/workflows/release.yml")),
  ) as { jobs: Record<string, { steps?: Step[] }> };
  const action = parseYaml(
    await Deno.readTextFile(
      resolve(root, ".github/actions/prepare-build/action.yml"),
    ),
  ) as { runs: { steps: Step[] } };
  const steps = [...Object.values(ci.jobs), ...Object.values(release.jobs)]
    .flatMap((job) => job.steps ?? []);
  for (const step of [...steps, ...action.runs.steps]) {
    if (step.run) {
      assert(
        !step.run.includes("\n") &&
          /^(deno task |deno test |cargo |pnpm )/.test(step.run),
        `Inline CI script: ${step.name}`,
      );
    }
  }
  assert(
    !steps.some((step) =>
      step.uses?.startsWith("actions-rust-lang/setup-rust-toolchain")
    ),
    "Workflow bypasses shared Rust setup",
  );
  assert(
    ![...steps, ...action.runs.steps].some((step) =>
      step.uses?.startsWith("actions/cache")
    ),
    "CI caches inputs beyond Rust compilation",
  );
  const rust = action.runs.steps.find((step) =>
    step.uses?.startsWith("actions-rust-lang/setup-rust-toolchain")
  );
  assert(
    rust?.with?.["cache-shared-key"] ===
      "${{ runner.os }}-${{ inputs.module }}",
    "Matrix modules share a conflicting Rust cache key",
  );
  assert(
    rust.with["cache-workspaces"] === ". -> target/cargo",
    "Rust cache includes preparation state",
  );
  assert(
    String(rust.with["cache-key"]).startsWith("${{ inputs.runner }}-"),
    "Rust cache omits the native runner environment",
  );
  assert(
    rust.with["cache-on-failure"] === false &&
      rust.with["cache-save-if"] ===
        "${{ github.event_name == 'push' && github.ref == 'refs/heads/v2' }}",
    "Unverified jobs can save shared Rust caches",
  );
  assert(
    ci.jobs.modules.steps?.some((step) =>
      step.with?.runner === "${{ matrix.os }}"
    ) === true,
    "Module cache does not use the declared native runner",
  );
  const prepare = action.runs.steps.find((step) =>
    step.run === "deno task prepare ${{ inputs.module }}"
  );
  assert(
    prepare !== undefined && prepare.if === undefined,
    "Cache hits skip input preparation",
  );
  for (
    const [prefix, setting] of [
      ["denoland/setup-deno@", "cache"],
      ["actions/setup-node@", "package-manager-cache"],
      ["pnpm/action-setup@", "cache"],
    ]
  ) {
    const setup = action.runs.steps.find((step) =>
      step.uses?.startsWith(prefix)
    );
    assert(
      setup?.with?.[setting] === false,
      `Unexpected dependency cache: ${prefix}`,
    );
  }
  assert(
    (release.jobs.version.steps ?? []).some((step) =>
      step.run === "deno task release:version"
    ),
    "Release versions bypass the tested entry",
  );
  assert(
    (release.jobs.publish.steps ?? []).some((step) =>
      step.run === "deno task release:publish packages"
    ),
    "Release publication bypasses the tested entry",
  );
});

Deno.test("Web package-manager requirements agree with CI installation", async () => {
  const manifest = JSON.parse(
    await Deno.readTextFile(resolve(root, "web/package.json")),
  ) as { engines: { pnpm: string } };
  const range = /^>=([0-9]+) <([0-9]+)$/.exec(manifest.engines.pnpm);
  assert(
    range !== null && Number(range[2]) === Number(range[1]) + 1,
    "Web must declare one supported pnpm major",
  );
  const action = parseYaml(
    await Deno.readTextFile(
      resolve(root, ".github/actions/prepare-build/action.yml"),
    ),
  ) as { runs: { steps: { uses?: string; with?: Record<string, unknown> }[] } };
  const setup = action.runs.steps.find((step) =>
    step.uses?.startsWith("pnpm/action-setup@")
  );
  assert(
    setup?.with?.version === range[1],
    "CI pnpm installation differs from the Web engine requirement",
  );
});

Deno.test("Release tag validation separates Debian and portable prerelease versions", async () => {
  const { directory, command, env } = await fixture();
  try {
    const output = resolve(directory, "outputs");
    for (
      const tag of [
        "v2.10.3",
        "v2.10.3-rc.1",
        "v02.10.3",
        "v2.10.3-01",
        "v2.10.3+build.1",
      ]
    ) {
      await Deno.writeTextFile(output, "");
      env.RELEASE_TAG = tag;
      const result = await command("tools/release.ts", "version");
      if (tag === "v2.10.3" || tag === "v2.10.3-rc.1") {
        assert(result.success, new TextDecoder().decode(result.stderr));
        const values = await Deno.readTextFile(output);
        const prerelease = tag.endsWith("-rc.1");
        assert(
          values.includes(
            `package_version=2.10.3${prerelease ? "~rc.1" : ""}\n`,
          ),
          values,
        );
        assert(values.includes(`release_version=${tag.slice(1)}\n`), values);
        assert(values.includes(`prerelease=${prerelease}\n`), values);
      } else {
        assert(!result.success, `Invalid tag was accepted: ${tag}`);
        assert(
          await Deno.readTextFile(output) === "",
          "Invalid tag emitted release outputs",
        );
      }
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("Release collects both package formats and refuses missing or mismatched Windows output", async () => {
  const { directory, command, env } = await fixture();
  try {
    const gh = resolve(directory, "bin/gh");
    await Deno.writeTextFile(
      gh,
      '#!/bin/sh\nprintf "%s\\n" "$@" > gh-args.txt\n',
    );
    await Deno.chmod(gh, 0o755);
    const version = "2.10.3~rc.1";
    const names = [
      "natsume-client",
      "natsume-server",
      "natsume-print-domjudge",
    ];
    for (const name of names) {
      const source = resolve(directory, name);
      await Deno.mkdir(resolve(source, "DEBIAN"), { recursive: true });
      await Deno.writeTextFile(
        resolve(source, "DEBIAN/control"),
        `Package: ${name}\nVersion: ${version}\nArchitecture: amd64\nMaintainer: Natsume Test <test@example.invalid>\nDescription: Package metadata fixture\n`,
      );
      const result = await new Deno.Command("dpkg-deb", {
        args: [
          "--build",
          source,
          resolve(directory, `${name}_${version}_amd64.deb`),
        ],
        stdout: "piped",
        stderr: "piped",
      }).output();
      assert(result.success, new TextDecoder().decode(result.stderr));
    }
    const archive = resolve(
      directory,
      "natsume-print-client_2.10.3-rc.1_windows_amd64.zip",
    );
    const zip = async (version: string, target = "print-client") => {
      const result = await new Deno.Command("python3", {
        args: [
          "-c",
          'import json, sys, zipfile; z = zipfile.ZipFile(sys.argv[1], "w"); z.writestr("package.json", json.dumps({"name": sys.argv[3], "version": sys.argv[2], "platform": "windows", "architecture": "amd64"})); z.close()',
          resolve(directory, `natsume-${target}_2.10.3-rc.1_windows_amd64.zip`),
          version,
          `natsume-${target}`,
        ],
        stdout: "piped",
        stderr: "piped",
      }).output();
      assert(result.success, new TextDecoder().decode(result.stderr));
    };
    Object.assign(env, {
      PACKAGE_VERSION: version,
      RELEASE_VERSION: "2.10.3-rc.1",
      RELEASE_TAG: "v2.10.3-rc.1",
      PRERELEASE: "true",
    });
    const processPath = resolve(directory, "tools/process.ts");
    await Deno.writeTextFile(
      processPath,
      (await Deno.readTextFile(processPath)).replace(
        '  "print-client": {',
        '  "future-client": { platform: "windows", format: "zip", crates: ["future-client"], binaries: ["future-client"], rustPlatforms: ["windows"], assets: [], web: false, readme: "README.md" },\n  "print-client": {',
      ),
    );
    const publish = () => command("tools/release.ts", "publish", directory);
    await zip("2.10.3-rc.1");
    await zip("2.10.3-rc.1", "photo-client");
    await zip("2.10.3-rc.1", "future-client");
    const good = await publish();
    assert(good.success, new TextDecoder().decode(good.stderr));
    const argumentsPath = resolve(directory, "gh-args.txt");
    const argumentsText = await Deno.readTextFile(argumentsPath);
    assert(
      argumentsText.includes("--prerelease") &&
        argumentsText.includes("--latest=false"),
      argumentsText,
    );
    for (
      const name of [
        ...names.map((name) => `${name}_${version}_amd64.deb`),
        "natsume-photo-client_2.10.3-rc.1_windows_amd64.zip",
        "natsume-print-client_2.10.3-rc.1_windows_amd64.zip",
        "natsume-future-client_2.10.3-rc.1_windows_amd64.zip",
      ]
    ) {
      assert(argumentsText.includes(name), `Release omitted ${name}`);
      const checksum = await Deno.readTextFile(
        resolve(directory, "SHA256SUMS"),
      );
      assert(
        checksum.includes(
          `${await sha256(resolve(directory, name))}  ${name}\n`,
        ),
        `Wrong release checksum: ${name}`,
      );
    }
    for (const failure of ["version", "corrupt", "missing"]) {
      await Deno.remove(argumentsPath);
      if (failure === "version") await zip("2.10.4");
      if (failure === "corrupt") {
        await Deno.writeTextFile(archive, "invalid ZIP");
      }
      if (failure === "missing") await Deno.remove(archive);
      const result = await publish();
      assert(!result.success, `Release accepted ${failure} Windows output`);
      assert(
        !await exists(argumentsPath),
        "Publication ran after package validation failed",
      );
      await Deno.writeTextFile(argumentsPath, "reset fixture");
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});
