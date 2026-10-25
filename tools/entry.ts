// Native prerequisites must be installable before downloaded modules are prepared.
export type RunOptions = {
  cwd?: string;
  env?: Record<string, string>;
  capture?: boolean;
};

export async function execute(
  command: string,
  args: string[],
  options: RunOptions = {},
): Promise<string> {
  const invocation = [command, ...args].join(" ");
  const cwd = options.cwd ?? Deno.cwd();
  console.log(`$ ${invocation}`);
  let child: Deno.ChildProcess;
  try {
    child = new Deno.Command(command, {
      args,
      cwd,
      env: options.env,
      stdin: "null",
      stdout: options.capture ? "piped" : "inherit",
      stderr: "inherit",
    }).spawn();
  } catch (cause) {
    const detail = cause instanceof Deno.errors.NotFound
      ? `Command not found: ${command}\nCheck that the required tool or prepared executable exists and is available on PATH or at the specified path.`
      : `Could not start command: ${invocation}`;
    throw new Error(`${detail}\nWorking directory: ${cwd}`, { cause });
  }
  // Only small metadata outputs use capture; compiler logs remain streaming.
  const result = options.capture
    ? await child.output()
    : { ...await child.status, stdout: new Uint8Array() };
  if (!result.success) {
    throw new Error(
      `Command failed (exit ${result.code}): ${invocation}\nWorking directory: ${cwd}\nSee the command output above for details.`,
    );
  }
  return new TextDecoder().decode(result.stdout);
}

function stableVersion(value: string): number[] | undefined {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(value);
  return match?.slice(1).map(Number);
}

export function validateDenoVersion(
  required: string,
  current = Deno.version.deno,
): void {
  const minimum = stableVersion(required);
  if (!minimum) {
    throw new Error(
      `Invalid .deno-version: ${
        JSON.stringify(required)
      }; expected a stable major.minor.patch version.`,
    );
  }
  const actual = stableVersion(current);
  if (
    actual && actual[0] === minimum[0] &&
    (actual[1] > minimum[1] ||
      actual[1] === minimum[1] && actual[2] >= minimum[2])
  ) return;
  const guidance = actual && actual[0] > minimum[0]
    ? `Deno ${
      actual[0]
    }.x has not been validated for this project. Use a supported ${
      minimum[0]
    }.x environment or validate the new major version before updating the project baseline.`
    : "Update Deno to a supported stable release using your installation method (standalone Deno: deno upgrade).";
  throw new Error(
    `Unsupported Deno version.\nCurrent: ${current}\nRequired: stable Deno >= ${required} and < ${
      minimum[0] + 1
    }.0.0 (.deno-version)\nExecutable: ${Deno.execPath()}\n${guidance}\nNo task commands were executed.`,
  );
}

export async function main(
  action: () => Promise<void>,
  task: string,
): Promise<void> {
  try {
    const required =
      (await Deno.readTextFile(new URL("../.deno-version", import.meta.url)))
        .trim();
    validateDenoVersion(required);
    await action();
  } catch (error) {
    console.error(`[${task}] FAILED`);
    console.error(error instanceof Error ? error.message : "Build task failed");
    let cause = error instanceof Error ? error.cause : undefined;
    while (cause instanceof Error) {
      console.error(`Caused by: ${cause.message}`);
      cause = cause.cause;
    }
    if (task === "prepare") {
      console.error(
        "Preparation is incomplete. Do not continue with offline checks or builds. Fix the error above, then rerun the same prepare command.",
      );
    }
    Deno.exitCode = 1;
  }
}
