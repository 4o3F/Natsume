import { skiaEnvironment } from "./assets.ts";
import { resolve } from "@std/path";
import {
  isPackageTarget,
  main,
  offline,
  packageTargets,
  requirePrepared,
  root,
  run,
  rustArguments,
  selection,
  sharedRustCrates,
  work,
} from "./process.ts";

async function checkSchema(): Promise<void> {
  // Diesel otherwise silently skips Rust formatting when rustfmt is unavailable.
  await run("rustfmt", ["--version"]);
  const directory = await Deno.makeTempDir({ dir: work, prefix: "schema-" });
  try {
    const database = resolve(directory, "schema.sqlite3");
    await run("diesel", [
      "--database-url",
      database,
      "--config-file",
      "/dev/null",
      "migration",
      "run",
      "--migration-dir",
      "crates/natsume-server/migrations",
    ], { env: offline });
    const schema = await run("diesel", [
      "--database-url",
      database,
      "--config-file",
      "crates/natsume-server/diesel.toml",
      "print-schema",
    ], { env: offline, capture: true });
    const generated = resolve(directory, "schema.rs");
    await Deno.writeTextFile(generated, schema);
    await run("diff", [
      "-u",
      "--",
      generated,
      resolve(root, "crates/natsume-server/diesel/schema.rs"),
    ]);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

async function exportApi(check: boolean): Promise<void> {
  await requirePrepared("rust", "web");
  const directory = check
    ? await Deno.makeTempDir({ dir: work, prefix: "api-" })
    : null;
  const output = directory
    ? resolve(directory, "types.ts")
    : "web/src/api/generated/types.ts";
  try {
    await run("cargo", [
      "run",
      "--frozen",
      "-p",
      "natsume-server",
      "--bin",
      "export-typescript",
      "--",
      output,
    ], { env: offline });
    await run("pnpm", ["exec", "prettier", "--write", resolve(root, output)], {
      cwd: resolve(root, "web"),
      env: offline,
    });
    if (check) {
      await run("diff", [
        "-u",
        "--",
        resolve(root, "web/src/api/generated/types.ts"),
        output,
      ]);
    }
  } finally {
    if (directory) await Deno.remove(directory, { recursive: true });
  }
}

async function build(): Promise<void> {
  const [command, ...args] = Deno.args;
  if (!["check", "test", "build", "api", "schema"].includes(command)) {
    throw new Error("Expected check, test, build, api or schema");
  }
  await requirePrepared("scripts");
  if (command === "schema") {
    if (args.length) throw new Error("schema does not accept arguments");
    await checkSchema();
    return;
  }
  if (command === "api") {
    if (args.length && !(args.length === 1 && args[0] === "--check")) {
      throw new Error("api only accepts --check");
    }
    await exportApi(args[0] === "--check");
    return;
  }
  const selected = selection(args);
  const selectedPackages = selected.filter(isPackageTarget);
  const shared = selected.includes("shared") ? sharedRustCrates() : [];
  if (selected.includes("shared") && (!shared.length || command === "build")) {
    throw new Error(
      "shared requires supported cross-platform crates and is only for check/test",
    );
  }
  const rust = selected.includes("rust") || selectedPackages.length > 0 ||
    shared.length > 0;
  const web = selected.includes("web") || command === "build" &&
      selectedPackages.some((target) => packageTargets[target].web);
  if (rust) await requirePrepared("rust");
  if (web) await requirePrepared("web");
  const native = Deno.build.os === "linux" &&
    (selected.includes("rust") || selected.includes("client"));
  const environment = native
    ? { ...offline, ...await skiaEnvironment() }
    : offline;
  const packages = selected.includes("rust") ? rustArguments() : [
    ...new Set([
      ...selectedPackages.flatMap((target) => packageTargets[target].crates),
      ...shared,
    ]),
  ].flatMap((name) => ["-p", name]);
  if (command === "test") {
    if (selected.includes("scripts")) {
      await run(Deno.execPath(), [
        "test",
        "--cached-only",
        "--frozen",
        "--allow-read",
        "--allow-write=target",
        "--allow-env",
        "--allow-run",
        "--deny-net",
        "tools/build_test.ts",
        "tools/windows_test.ts",
      ]);
    }
    if (rust) {
      await run("cargo", ["test", ...packages, "--all-features", "--frozen"], {
        env: environment,
      });
    }
    if (web) {
      await run("pnpm", ["test"], { cwd: resolve(root, "web"), env: offline });
    }
    return;
  }
  if (command === "check") {
    if (selected.includes("scripts")) {
      await run(Deno.execPath(), [
        "test",
        "--no-run",
        "--cached-only",
        "--frozen",
        "tools/assets.ts",
        "tools/build.ts",
        "tools/package.ts",
        "tools/process.ts",
        "tools/build_test.ts",
        "tools/windows.ts",
        "tools/windows_test.ts",
        "tools/entry.ts",
        "tools/native-deps.ts",
        "tools/release.ts",
      ]);
      await run(Deno.execPath(), ["fmt", "--check"]);
      await run(Deno.execPath(), ["lint"]);
    }
    if (rust) {
      await run("cargo", [
        "clippy",
        ...packages,
        "--all-targets",
        "--all-features",
        "--frozen",
        "--",
        "-D",
        "warnings",
      ], { env: environment });
    }
    if (web) {
      await run("pnpm", ["typecheck"], {
        cwd: resolve(root, "web"),
        env: offline,
      });
    }
    return;
  }
  if (!rust && !web) throw new Error("build needs a Rust or Web target");
  if (rust) {
    const binaries = selected.includes("rust")
      ? []
      : selectedPackages.flatMap((target) =>
        packageTargets[target].binaries.flatMap((name) => ["--bin", name])
      );
    await run("cargo", [
      "build",
      "--release",
      "--frozen",
      ...packages,
      ...binaries,
    ], {
      env: environment,
    });
  }
  if (web) {
    await run("pnpm", ["build"], { cwd: resolve(root, "web"), env: offline });
  }
}

if (import.meta.main) await main(build, Deno.args[0] ?? "build");
