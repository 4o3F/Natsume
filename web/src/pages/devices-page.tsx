import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { Alert, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

type Device = components["schemas"]["DeviceResponse"];
type Convergence = components["schemas"]["DeviceConvergenceResponse"];
type ConvergenceStatus = Convergence["gateway"]["status"];
type DeviceLifecycleState = Device["state"];

const DEVICES_KEY = ["devices"] as const;

export function DevicesPage() {
  const session = useSession().data;
  const queryClient = useQueryClient();
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const devices = useQuery({
    queryKey: DEVICES_KEY,
    queryFn: async () => unwrap<Device[]>(await api.GET("/api/v2/devices")),
    refetchInterval: LIST_POLL_MS,
  });
  const lifecycle = useMutation({
    mutationFn: async ({
      deviceId,
      state,
    }: {
      deviceId: string;
      state: DeviceLifecycleState;
    }) =>
      unwrap<void>(
        await api.PATCH("/api/v2/devices/{device_id}", {
          params: { path: { device_id: deviceId } },
          body: { state },
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
    },
  });
  const selectedDevice = devices.data?.find(
    (device) => device.device_id === selectedDeviceId,
  );

  const isAdmin = session?.role === "admin";
  const columns: ColumnDef<Device>[] = [
    { accessorKey: "machine_hardware_id", header: "Machine hardware ID" },
    {
      accessorKey: "state",
      header: "Lifecycle",
      cell: ({ row }) => (
        <Badge
          variant={row.original.state === "revoked" ? "destructive" : "outline"}
        >
          {row.original.state}
        </Badge>
      ),
    },
    { accessorKey: "evidence_quality", header: "Evidence" },
    {
      id: "convergence",
      header: "Convergence",
      cell: ({ row }) => {
        const convergence = row.original.convergence;
        const statuses = [
          ["Connection", convergence.connection_state],
          ["Gateway", convergence.gateway.status],
          ["Binding", convergence.binding.status],
          ["Runtime", convergence.runtime_config.status],
          ["Session", convergence.session_control.status],
          ["Home", convergence.home.status],
        ];
        return (
          <div className="flex flex-wrap gap-1">
            {statuses.map(([name, status]) => (
              <Badge
                key={name}
                variant={
                  status === "failed"
                    ? "destructive"
                    : status === "active" || status === "converged"
                      ? "default"
                      : "outline"
                }
              >
                {name}: {label(status)}
              </Badge>
            ))}
          </div>
        );
      },
    },
    {
      accessorKey: "created_at_unix_ms",
      header: "Created",
      cell: ({ row }) =>
        new Date(row.original.created_at_unix_ms).toLocaleString(),
    },
    {
      id: "details",
      header: "Details",
      cell: ({ row }) => (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setSelectedDeviceId(row.original.device_id)}
        >
          View
        </Button>
      ),
    },
  ];

  if (isAdmin) {
    columns.push({
      id: "actions",
      header: "Actions",
      cell: ({ row }) => {
        const device = row.original;
        return (
          <div className="flex gap-2">
            {device.state === "enabled" && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={lifecycle.isPending}
                onClick={() =>
                  lifecycle.mutate({
                    deviceId: device.device_id,
                    state: "disabled",
                  })
                }
              >
                Disable
              </Button>
            )}
            {device.state === "disabled" && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={lifecycle.isPending}
                onClick={() =>
                  lifecycle.mutate({
                    deviceId: device.device_id,
                    state: "enabled",
                  })
                }
              >
                Enable
              </Button>
            )}
            {device.state !== "revoked" && (
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button
                    type="button"
                    variant="destructive"
                    size="sm"
                    disabled={lifecycle.isPending}
                  >
                    Revoke
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>
                      Permanently revoke device?
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                      Revocation is terminal and immediately evicts the current
                      connection.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction
                      className="bg-destructive text-white hover:bg-destructive/90"
                      onClick={() =>
                        lifecycle.mutate({
                          deviceId: device.device_id,
                          state: "revoked",
                        })
                      }
                    >
                      Revoke
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            )}
          </div>
        );
      },
    });
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Devices</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Durable lifecycle facts and current connection convergence.
        </p>
      </div>

      {lifecycle.error && (
        <Alert variant="destructive">
          <AlertTitle>
            {lifecycle.error instanceof ApiError
              ? lifecycle.error.title
              : "Device lifecycle update failed"}
          </AlertTitle>
        </Alert>
      )}

      <DataState
        isLoading={devices.isLoading}
        error={devices.data ? null : devices.error}
        isEmpty={!devices.data?.length}
        emptyLabel="No devices found."
      >
        <div className="space-y-6">
          <DataTable columns={columns} data={devices.data ?? []} />
          {selectedDevice && <DeviceConvergence device={selectedDevice} />}
        </div>
      </DataState>
    </div>
  );
}

function DeviceConvergence({ device }: { device: Device }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{device.machine_hardware_id}</CardTitle>
        <CardDescription className="font-mono">
          {device.device_id}
        </CardDescription>
      </CardHeader>
      <CardContent>
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
    ? `${data.session_control.target.lock_state}; terminate epoch ${data.session_control.target.terminate_epoch ?? "none"}`
    : "none";
  const sessionActual = data.session_control.actual
    ? `${data.session_control.actual.session_state}; completed epoch ${data.session_control.actual.completed_terminate_epoch ?? "none"}`
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
    <div className="space-y-2 rounded-md border p-3 text-sm">
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
