import type {
  DeviceResponse_Serialize as Device,
  DeviceConvergenceResponse_Serialize as Convergence,
} from "@/api/generated/types";
import { DeviceNetworkDetails } from "@/components/device-network-info";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

type ConvergenceStatus = Convergence["gateway"]["status"];

export function DeviceConvergence({ device }: { device: Device }) {
  return (
    <Card className="min-w-0">
      <CardHeader>
        <CardTitle className="break-all font-mono text-base">
          {device.machine_hardware_id}
        </CardTitle>
        <CardDescription className="break-all font-mono">
          {device.device_id}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <DeviceNetworkDetails device={device} />
        <ConvergenceDetails data={device.convergence} />
      </CardContent>
    </Card>
  );
}

function ConvergenceDetails({ data }: { data: Convergence }) {
  const gatewayTarget = data.gateway.target
    ? `${data.gateway.target.credential_id}; leaf ${data.gateway.target.gateway_leaf_sha256 ?? "pending"}`
    : "none";
  const gatewayActual = data.gateway.actual
    ? `${data.gateway.actual.state}; credential ${data.gateway.actual.credential_id ?? "absent"}; leaf ${data.gateway.actual.gateway_leaf_sha256 ?? "absent"}`
    : "none";
  const bindingTarget = data.binding.target
    ? data.binding.target.state === "bound"
      ? `bound to ${data.binding.target.context.seat_code} (${data.binding.target.context.domjudge_username})`
      : `unbound; negotiation ${data.binding.target.negotiation_id}${data.binding.target.evaluation ? `; ${data.binding.target.evaluation.error_code}` : ""}`
    : "none";
  const bindingActual = data.binding.actual
    ? `assignment ${data.binding.actual.assignment_state}; credential ${data.binding.actual.credential_state}; seat ${data.binding.actual.context?.seat_code ?? "absent"}`
    : "none";
  const sessionTarget = data.session_control.target
    ? `${data.session_control.target.foreground_target}; terminate epoch ${data.session_control.target.terminate_epoch ?? "none"}`
    : "none";
  const sessionActual = data.session_control.actual
    ? `${data.session_control.actual.session_state}; foreground ${data.session_control.actual.foreground}; waiting ready ${data.session_control.actual.waiting_ready}; contest ready ${data.session_control.actual.contest_ready}; completed epoch ${data.session_control.actual.completed_terminate_epoch ?? "none"}`
    : "none";

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <Badge
          variant={data.connection_state === "active" ? "default" : "outline"}
        >
          {label(data.connection_state)}
        </Badge>
        <span className="text-muted-foreground">
          Latest state: {formatTimestamp(data.received_at_unix_ms)}
        </span>
      </div>
      <div className="grid gap-3 lg:grid-cols-2">
        <ConvergenceRow
          name="Gateway"
          status={data.gateway.status}
          target={gatewayTarget}
          actual={gatewayActual}
        />
        <ConvergenceRow
          name="Binding"
          status={data.binding.status}
          target={bindingTarget}
          actual={bindingActual}
        />
        <ConvergenceRow
          name="Runtime config"
          status={data.runtime_config.status}
          target={data.runtime_config.target_domjudge_origin ?? "none"}
          actual={
            data.runtime_config.actual
              ? `${data.runtime_config.actual.state}; ${data.runtime_config.actual.applied_domjudge_origin ?? "absent"}`
              : "none"
          }
        />
        <ConvergenceRow
          name="Session control"
          status={data.session_control.status}
          target={sessionTarget}
          actual={sessionActual}
        />
        <ConvergenceRow
          name="Home"
          status={data.home.status}
          target={`reset epoch ${data.home.target_reset_epoch ?? "none"}`}
          actual={
            data.home.actual
              ? `${data.home.actual.state}; completed epoch ${data.home.actual.completed_reset_epoch ?? "none"}`
              : "none"
          }
        />
      </div>
    </div>
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
  target: string;
  actual: string;
}) {
  return (
    <div className="min-w-0 space-y-2 rounded-md border p-3 text-sm break-all">
      <div className="flex items-center justify-between gap-3">
        <span className="font-medium">{name}</span>
        <Badge variant={status === "failed" ? "destructive" : "outline"}>
          {label(status)}
        </Badge>
      </div>
      <p>
        <span className="text-muted-foreground">Target:</span> {target}
      </p>
      <p>
        <span className="text-muted-foreground">Actual:</span> {actual}
      </p>
    </div>
  );
}

function label(value: string) {
  return value.replaceAll("_", " ");
}

function formatTimestamp(value: number | null) {
  return value === null ? "not received" : new Date(value).toLocaleString();
}
