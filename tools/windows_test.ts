import { resolve } from "@std/path";
import { parse as parseToml } from "@std/toml";
import {
  packagesFor,
  packageTargets,
  root,
  rustArguments,
  selection,
  sha256,
  sharedRustCrates,
  work,
} from "./process.ts";
import { stageWindowsPackage, validateWindowsVersion } from "./windows.ts";

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function rejects(action: () => unknown): void {
  let failure;
  try {
    action();
  } catch (error) {
    failure = error;
  }
  assert(failure instanceof Error, "Expected an error");
}

Deno.test("package defaults and explicit selections respect the host platform", () => {
  assert(
    JSON.stringify(packagesFor("linux")) ===
      JSON.stringify(["server", "client", "print-domjudge"]),
    "Linux package selection changed",
  );
  assert(
    JSON.stringify(selection([], "windows")) ===
      JSON.stringify(packagesFor("windows")),
    "Windows defaults must not select Linux, Web or workspace builds",
  );
  assert(
    JSON.stringify(selection(["all"], "windows")) ===
      JSON.stringify(packagesFor("windows")),
    "Windows all must use the package definitions",
  );
  rejects(() => selection(["print-client"], "linux"));
  rejects(() => selection(["photo-client"], "linux"));
  assert(
    rustArguments("windows").includes("--workspace") &&
      packageTargets["photo-client"].crates.includes("natsume-photo-client"),
    "Windows workspace checks must include the photo backend",
  );
  rejects(() => selection(["client"], "windows"));
});

Deno.test("Rust checks preserve Linux shared coverage and check the Windows workspace", () => {
  assert(
    !rustArguments("linux").some((argument, index, args) =>
      args[index - 1] === "--exclude" && argument === "natsume-print-client"
    ),
    "Linux must retain the print client's shared tests",
  );
  assert(
    JSON.stringify(rustArguments("windows")) ===
      JSON.stringify(["--workspace"]),
    "Windows must check every workspace member, including Linux-only entry points",
  );
  const extended = {
    ...packageTargets,
    future: { crates: ["future-windows-client"], rustPlatforms: ["windows"] },
  };
  assert(
    JSON.stringify(rustArguments("linux", extended)) ===
      JSON.stringify([
        ...rustArguments("linux"),
        "--exclude",
        "future-windows-client",
      ]),
    "A Windows-only addition must not break Linux workspace checks",
  );
  assert(
    JSON.stringify(rustArguments("windows", extended)) ===
      JSON.stringify(["--workspace"]),
    "Windows workspace checks must include future members without a package filter",
  );
});

Deno.test("shared Linux regressions and Client library tests have explicit owners", () => {
  assert(
    sharedRustCrates("linux").includes("natsume-print-client"),
    "Linux print regressions disappeared",
  );
  for (
    const name of ["natsume-workstation-protocol", "natsume-workstation-ipc"]
  ) {
    assert(
      (packageTargets.client.crates as readonly string[]).includes(name),
      `Client checks omitted ${name}`,
    );
  }
});

Deno.test("every workspace crate has a module owner so splitting CI cannot drop library tests", async () => {
  const workspace = parseToml(
    await Deno.readTextFile(resolve(root, "Cargo.toml")),
  ) as { workspace: { members: string[] } };
  for (const member of workspace.workspace.members) {
    const manifest = parseToml(
      await Deno.readTextFile(resolve(root, member, "Cargo.toml")),
    ) as { package: { name: string } };
    const owners = Object.values(packageTargets).filter((target) =>
      (target.crates as readonly string[]).includes(manifest.package.name)
    );
    assert(
      owners.length === 1,
      `Expected one CI module owner: ${manifest.package.name}`,
    );
  }
});

Deno.test("portable versions preserve SemVer and reject Debian and unsafe names", () => {
  for (
    const version of ["2.10.3", "2.10.3-rc.1", "2.10.3-ci.1", "2.10.3+build.1"]
  ) {
    validateWindowsVersion(version);
  }
  for (
    const version of [
      "",
      "2.10.3~rc.1",
      "1:2.10.3",
      "../2.10.3",
      "02.10.3",
      "2.10.3-01",
      "2.10.3-rc..1",
      "2.10.3/other",
      "2.10.3\n",
    ]
  ) rejects(() => validateWindowsVersion(version));
});

function executable(machine = 0x8664): Uint8Array {
  const bytes = new Uint8Array(128);
  const view = new DataView(bytes.buffer);
  view.setUint16(0, 0x5a4d, true);
  view.setUint32(0x3c, 64, true);
  view.setUint32(64, 0x4550, true);
  view.setUint16(68, machine, true);
  view.setUint16(88, 0x20b, true);
  return bytes;
}

Deno.test("Windows staging includes only declared executables, public docs and checksums", async () => {
  const directory = await Deno.makeTempDir({
    dir: work,
    prefix: "windows-test-",
  });
  try {
    const release = resolve(directory, "release");
    const staged = resolve(directory, "staged");
    await Deno.mkdir(release);
    await Deno.writeFile(
      resolve(release, "natsume-print-client.exe"),
      executable(),
    );
    for (
      const name of [
        "client.toml",
        "state.jsonl",
        "credentials.txt",
        "other.exe",
      ]
    ) {
      await Deno.writeTextFile(resolve(release, name), "must not be packaged");
    }
    await stageWindowsPackage("print-client", "2.10.3-rc.1", release, staged);
    const names = [];
    for await (const entry of Deno.readDir(staged)) names.push(entry.name);
    const expected = [
      "LICENSE",
      "README.md",
      "SHA256SUMS",
      "natsume-print-client.exe",
      "package.json",
    ];
    assert(
      JSON.stringify(names.sort()) === JSON.stringify(expected.sort()),
      "Unexpected package files",
    );
    assert(
      await Deno.readTextFile(resolve(staged, "LICENSE")) ===
        await Deno.readTextFile(resolve(root, "LICENSE")),
      "Wrong project license",
    );
    const metadata = JSON.parse(
      await Deno.readTextFile(resolve(staged, "package.json")),
    );
    assert(metadata.name === "natsume-print-client", "Wrong package name");
    assert(
      metadata.version === "2.10.3-rc.1",
      "The prerelease version changed",
    );
    assert(
      metadata.platform === "windows" && metadata.architecture === "amd64",
      "Wrong platform",
    );
    const checksums = await Deno.readTextFile(resolve(staged, "SHA256SUMS"));
    for (const name of expected.filter((name) => name !== "SHA256SUMS")) {
      assert(
        checksums.includes(`${await sha256(resolve(staged, name))}  ${name}\n`),
        `Wrong checksum: ${name}`,
      );
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("Windows staging rejects missing, non-Windows and wrong-architecture binaries", async () => {
  const directory = await Deno.makeTempDir({
    dir: work,
    prefix: "windows-test-",
  });
  try {
    const release = resolve(directory, "release");
    await Deno.mkdir(release);
    const binary = resolve(release, "natsume-print-client.exe");
    const cases = [
      undefined,
      new TextEncoder().encode("ELF fixture"),
      executable(0xaa64),
    ];
    for (const [index, bytes] of cases.entries()) {
      if (bytes) await Deno.writeFile(binary, bytes);
      let failure;
      try {
        await stageWindowsPackage(
          "print-client",
          "2.10.3",
          release,
          resolve(directory, `staged-${index}`),
        );
      } catch (error) {
        failure = error;
      }
      assert(failure instanceof Error, "Invalid executable was packaged");
    }
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});
