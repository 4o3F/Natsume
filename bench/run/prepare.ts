import { input, remote } from "./run.ts";
import { configuration, type Experiment, quote as q } from "./config.ts";

/** Extend the synthetic roster and enroll with logos disabled, below the TLS cap. */
export async function prepare(devices: number, e: Experiment) {
  if (![500, 600, 800].includes(devices)) {
    throw new Error(
      "Prepare supports the capacity ladder's 500/600/800 baselines",
    );
  }
  const { server: A, loadgen: B, server_url: serverUrl } = e.hosts;
  const version = e.version,
    base = `/srv/natsume-bench/${version}`,
    work = `${input}/prepare-${devices}`;
  const snapshot = `baseline-${devices}.tar.gz`;
  await remote(
    A,
    `test ! -e ${q(`${base}/${snapshot}`)}\nsudo -n bash -se -- ${
      q(version)
    } baseline.tar.gz <<'RESTORE'\n${await Deno.readTextFile(
      "bench/run/restore.sh",
    )}\nRESTORE`,
  );
  try {
    await remote(
      B,
      `test ! -e ${q(work)}\nmkdir -m 700 ${
        q(work)
      }\ncp -a ${input}/fleet-baseline ${work}/fleet\n${input}/natsume-loadgen setup --server ${
        q(serverUrl)
      } --trust-root ${input}/control-ca.crt --operator-login synthetic-admin --operator-password-file ${input}/operator-password --output-dir ${work}/fixture --logo-directory ${input}/logos --reuse-logos --teams ${devices} --schools 140 > ${work}/setup.log 2>&1\nsudo -n systemd-run --collect --unit=natsume-prepare-loadgen -p User=icpc -p LimitNOFILE=65536 -p KillSignal=SIGINT -p TimeoutStopSec=30 -p StandardOutput=append:${work}/enroll.log /usr/bin/numactl --cpunodebind=0 --membind=0 ${input}/natsume-loadgen run --server ${
        q(serverUrl)
      } --trust-root ${input}/control-ca.crt --state-dir ${work}/fleet --seat-plan ${work}/fixture/seats.csv --output-dir ${work}/enrollment --seed 418 --devices ${devices} --ramp 5s --duration 5m --logo-interval 0s`,
    );
    let ready = false;
    for (let i = 0; i < 36; i++) {
      await new Promise((r) => setTimeout(r, 5000));
      const log = await remote(B, `tail -n 1 ${work}/enroll.log`);
      if (log.includes(`connected=${devices} devices=${devices}`)) {
        ready = true;
        break;
      }
    }
    if (!ready) throw new Error("Baseline enrollment did not finish");
    await remote(
      B,
      `sudo -n systemctl stop natsume-prepare-loadgen\nfor ((i=418;i<${devices};i++)); do printf -v target '%05d' "$i"; printf -v source '%05d' "$((i % 140))"; cp ${input}/fleet-baseline/logos-$source.json ${work}/fleet/logos-$target.json; done\nchmod 600 ${work}/fleet/logos-*.json`,
    );
    await remote(
      A,
      `sudo -n systemctl stop natsume-bench\nsudo -n bash -se <<'SNAPSHOT'\nset -euo pipefail\numask 077\ncd ${base}\ntest ! -e ${snapshot}\ntar --xattrs --acls --numeric-owner -cpzf ${snapshot} data\nsha256sum ${snapshot} > ${snapshot}.sha256\nsha256sum -c ${snapshot}.sha256\ntar -tzf ${snapshot} >/dev/null\nSNAPSHOT`,
    );
    console.log(
      `Prepared N=${devices}; enrolled fleet remains on B; snapshot remains on A`,
    );
  } finally {
    await remote(
      B,
      'if test "$(systemctl show natsume-prepare-loadgen -p LoadState --value)" = loaded; then sudo -n systemctl stop natsume-prepare-loadgen; fi',
    );
    await remote(A, "sudo -n systemctl stop natsume-bench");
  }
}
if (import.meta.main) {
  if (Deno.args.length !== 2) {
    throw new Error("Usage: prepare.ts devices experiment.toml");
  }
  await prepare(
    Number(Deno.args[0]),
    configuration(await Deno.readTextFile(Deno.args[1])),
  );
}
