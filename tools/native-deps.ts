import { execute, main } from "./entry.ts";

async function install(): Promise<void> {
  const packages = Deno.args;
  if (
    Deno.build.os !== "linux" || !packages.length ||
    packages.some((name) => !/^[a-z0-9][a-z0-9+.-]*$/.test(name))
  ) {
    throw new Error("Expected Ubuntu package names on Linux");
  }
  await execute("sudo", ["apt-get", "update"]);
  await execute("sudo", [
    "env",
    "DEBIAN_FRONTEND=noninteractive",
    "apt-get",
    "install",
    "--yes",
    "--no-install-recommends",
    ...packages,
  ]);
}

if (import.meta.main) await main(install, "native-deps");
