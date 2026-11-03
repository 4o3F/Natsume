import { command, input, remote, sshOptions } from "./run.ts";
import { configuration, type Experiment, quote as q } from "./config.ts";

// Existing operator-password on B is never read by the workstation.
// The PTY helper on A decrypts an envelope with an ephemeral private key.
export async function bootstrap(e: Experiment) {
  const { server: A, loadgen: B } = e.hosts;
  const version = e.version;
  if (!/^v\d+\.\d+\.\d+$/.test(version)) throw new Error("Invalid version");
  const directory = await remote(
    A,
    "sudo -n mktemp -d /run/natsume-bench-bootstrap.XXXXXXXX",
  );
  let relay: Deno.ChildProcess | undefined,
    source: Deno.ChildProcess | undefined;
  try {
    await remote(
      A,
      `command -v expect >/dev/null\ntest "$(systemctl show natsume-bench -p MainPID --value)" = 0\nsudo -n openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out ${
        q(`${directory}/key.pem`)
      } 2>/dev/null\nsudo -n tee ${
        q(`${directory}/bootstrap.exp`)
      } >/dev/null <<'EXPECT'\n${await Deno.readTextFile(
        "bench/run/bootstrap.exp",
      )}\nEXPECT`,
    );
    const publicKey = await remote(
      A,
      `sudo -n openssl pkey -in ${q(`${directory}/key.pem`)} -pubout`,
    );
    await remote(
      B,
      `umask 077\ncat > ${input}/bootstrap-public.pem <<'PUBLIC'\n${publicKey}\nPUBLIC`,
    );
    source = new Deno.Command("ssh", {
      args: [
        ...sshOptions,
        B,
        `openssl pkeyutl -encrypt -pubin -inkey ${input}/bootstrap-public.pem -pkeyopt rsa_padding_mode:oaep -in ${input}/operator-password`,
      ],
      stdout: "piped",
      stderr: "null",
    }).spawn();
    relay = new Deno.Command("ssh", {
      args: [
        ...sshOptions,
        A,
        `sudo -n expect ${q(`${directory}/bootstrap.exp`)} ${q(directory)} ${
          q(version)
        }`,
      ],
      stdin: "piped",
      stdout: "null",
      stderr: "null",
    }).spawn();
    await source.stdout.pipeTo(relay.stdin);
    const results = await Promise.all([source.status, relay.status]);
    if (results.some((r) => !r.success)) {
      throw new Error("Bootstrap failed; terminal transcript withheld");
    }
    console.log("Synthetic operator bootstrap completed through a remote PTY");
  } finally {
    await command("ssh", [
      ...sshOptions,
      A,
      `sudo -n rm -rf -- ${q(directory)}`,
    ]);
    await remote(B, `rm -f ${input}/bootstrap-public.pem`);
  }
}
if (import.meta.main) {
  if (Deno.args.length !== 1) {
    throw new Error("Usage: bootstrap.ts experiment.toml");
  }
  await bootstrap(configuration(await Deno.readTextFile(Deno.args[0])));
}
