import type { ReactNode } from "react";

import type {
  DeviceResponse_Serialize as Device,
  DeviceConvergenceResponse_Serialize as Convergence,
} from "@/api/generated/types";
import { DeviceNetworkDetails } from "@/components/device-network-info";
import { StatusDot } from "@/components/device-status";
import { convergenceIcons, tones } from "@/components/device-status-style";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

type ConvergenceStatus = Convergence["gateway"]["status"];
type Fact = [label: string, value: ReactNode];

/** Detailed target/actual view for one device, used in the detail sheet. */
export function DeviceConvergence({ device }: { device: Device }) {
  return (
    <div className="space-y-6">
      <DeviceNetworkDetails device={device} />
      <ConvergenceDetails data={device.convergence} />
    </div>
  );
}

function ConvergenceDetails({ data }: { data: Convergence }) {
  const bindingTarget = data.binding.target;
  const bindingActual = data.binding.actual;
  const sessionTarget = data.session_control.target;
  const sessionActual = data.session_control.actual;

  return (
    <section aria-label="Convergence" className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm font-semibold">Convergence</p>
        <span className="text-xs text-muted-foreground">
          Latest state {formatTimestamp(data.received_at_unix_ms)}
        </span>
      </div>
      <div className="divide-y overflow-hidden rounded-lg border">
        <ConvergenceRow
          name="Gateway"
          status={data.gateway.status}
          target={
            data.gateway.target
              ? [
                  ["Credential", data.gateway.target.credential_id],
                  [
                    "Leaf",
                    data.gateway.target.gateway_leaf_sha256 ?? "pending",
                  ],
                ]
              : null
          }
          actual={
            data.gateway.actual
              ? [
                  ["State", data.gateway.actual.state],
                  ["Credential", data.gateway.actual.credential_id ?? "absent"],
                  ["Leaf", data.gateway.actual.gateway_leaf_sha256 ?? "absent"],
                ]
              : null
          }
        />
        <ConvergenceRow
          name="Binding"
          status={data.binding.status}
          target={
            bindingTarget
              ? bindingTarget.state === "bound"
                ? [
                    ["Seat", bindingTarget.context.seat_code],
                    ["Account", bindingTarget.context.domjudge_username],
                  ]
                : [
                    ["State", "unbound"],
                    ["Negotiation", bindingTarget.negotiation_id],
                    ...(bindingTarget.evaluation
                      ? ([
                          ["Error", bindingTarget.evaluation.error_code],
                        ] as Fact[])
                      : []),
                  ]
              : null
          }
          actual={
            bindingActual
              ? [
                  ["Assignment", bindingActual.assignment_state],
                  ["Credential", bindingActual.credential_state],
                  ["Seat", bindingActual.context?.seat_code ?? "absent"],
                ]
              : null
          }
        />
        <ConvergenceRow
          name="Runtime config"
          status={data.runtime_config.status}
          target={[
            ["Origin", data.runtime_config.target_domjudge_origin ?? "none"],
          ]}
          actual={
            data.runtime_config.actual
              ? [
                  ["State", data.runtime_config.actual.state],
                  [
                    "Origin",
                    data.runtime_config.actual.applied_domjudge_origin ??
                      "absent",
                  ],
                ]
              : null
          }
        />
        <ConvergenceRow
          name="Session control"
          status={data.session_control.status}
          target={
            sessionTarget
              ? [
                  ["Foreground", sessionTarget.foreground_target],
                  ["Terminate epoch", sessionTarget.terminate_epoch ?? "none"],
                ]
              : null
          }
          actual={
            sessionActual
              ? [
                  ["Session", sessionActual.session_state],
                  ["Foreground", sessionActual.foreground],
                  ["Waiting ready", String(sessionActual.waiting_ready)],
                  ["Contest ready", String(sessionActual.contest_ready)],
                  [
                    "Completed epoch",
                    sessionActual.completed_terminate_epoch ?? "none",
                  ],
                ]
              : null
          }
        />
        <ConvergenceRow
          name="Home"
          status={data.home.status}
          target={[["Reset epoch", data.home.target_reset_epoch ?? "none"]]}
          actual={
            data.home.actual
              ? [
                  ["State", data.home.actual.state],
                  [
                    "Completed epoch",
                    data.home.actual.completed_reset_epoch ?? "none",
                  ],
                ]
              : null
          }
        />
      </div>
    </section>
  );
}

function ConvergenceRow({
  name,
  status,
  target,
  actual,
}: {
  name: string;
  status: ConvergenceStatus;
  target: Fact[] | null;
  actual: Fact[] | null;
}) {
  const tone = convergenceIcons[status].key;
  return (
    <div className="space-y-2.5 bg-card p-3.5">
      <div className="flex items-center justify-between gap-3">
        <span className="flex items-center gap-2 text-sm font-medium">
          <StatusDot tone={tone} pulse={status === "reconciling"} />
          {name}
        </span>
        <Badge variant={tones[tone].badge}>{label(status)}</Badge>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <FactList title="Target" facts={target} />
        <FactList title="Actual" facts={actual} />
      </div>
    </div>
  );
}

function FactList({ title, facts }: { title: string; facts: Fact[] | null }) {
  return (
    <div className="min-w-0 rounded-md bg-muted/50 px-3 py-2">
      <p className="mb-1 text-xs font-medium text-muted-foreground">{title}</p>
      {facts ? (
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-xs">
          {facts.map(([key, value]) => (
            <div key={key} className="contents">
              <dt className="text-muted-foreground">{key}</dt>
              <dd
                className={cn(
                  "truncate font-mono",
                  (value === "absent" || value === "none") &&
                    "text-muted-foreground",
                )}
                title={typeof value === "string" ? value : undefined}
              >
                {value}
              </dd>
            </div>
          ))}
        </dl>
      ) : (
        <p className="text-xs text-muted-foreground">none</p>
      )}
    </div>
  );
}

function label(value: string) {
  return value.replaceAll("_", " ");
}

function formatTimestamp(value: number | null) {
  return value === null ? "not received" : new Date(value).toLocaleString();
}
