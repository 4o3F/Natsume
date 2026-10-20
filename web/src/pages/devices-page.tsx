import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RotateCcw, Search } from "lucide-react";

import { useSessionScope } from "@/auth/session-context";
import { ApiError, unwrap } from "@/api/errors";
import type {
  DeviceConvergenceResponse_Serialize as DeviceConvergenceResponse,
  DeviceResponse_Serialize as DeviceResponse,
} from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DeviceEmergencyPanel } from "@/components/device-emergency-panel";
import { MultiSelectFilter } from "@/components/multi-select-filter";
import {
  DeviceIpWarning,
  DeviceNetworkDetails,
} from "@/components/device-network-info";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import {
  defaultDeviceStateFilter,
  selectedDeviceStates,
  type DeviceStateFilterValue,
} from "@/components/device-state";
import {
  StatusIcon,
  ConvergenceIcon,
  RefreshCountdown,
} from "@/components/device-status";
import {
  connections,
  convergenceIcons,
} from "@/components/device-status-style";
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
import { Input } from "@/components/ui/input";

type Device = DeviceResponse;
type Convergence = DeviceConvergenceResponse;
type ConvergenceStatus = Convergence["gateway"]["status"];
type DeviceLifecycleState = Device["state"];

const DEVICES_KEY = ["devices"] as const;
const connectionStates = Object.keys(
  connections,
) as Convergence["connection_state"][];
const defaultConnectionFilter = {
  active: true,
  awaiting_fresh_state: true,
  offline: true,
};
const lifecycleOptions = [
  { value: "enabled", label: "Enabled" },
  { value: "disabled", label: "Disabled" },
  { value: "revoked", label: "Revoked" },
] as const;
const connectionOptions = connectionStates.map((value) => ({
  value,
  label: connections[value].label,
}));
const bindingOptions = [
  { value: "bound", label: "Bound" },
  { value: "unbound", label: "Unbound" },
  { value: "unknown", label: "Unknown" },
] as const;
const defaultBindingFilter = { bound: true, unbound: true, unknown: true };

export function DevicesPage() {
  const { api } = useSessionScope();
  const session = useSession().data;
  const queryClient = useQueryClient();
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [connectionFilter, setConnectionFilter] = useState(
    defaultConnectionFilter,
  );
  const [stateFilter, setStateFilter] = useState<DeviceStateFilterValue>(
    defaultDeviceStateFilter,
  );
  const [bindingFilter, setBindingFilter] = useState(defaultBindingFilter);
  const states = selectedDeviceStates(stateFilter);
  const devices = useQuery({
    queryKey: [...DEVICES_KEY, states],
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(await api.listDevices(states, { signal })),
    refetchInterval: LIST_POLL_MS,
    enabled: states.length > 0,
  });
  const lifecycle = useMutation({
    mutationFn: async ({
      deviceId,
      state,
    }: {
      deviceId: string;
      state: DeviceLifecycleState;
    }) => unwrap<void>(await api.updateDevice(deviceId, { state })),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
    },
  });
  const unbind = useMutation({
    mutationFn: async (deviceId: string) =>
      unwrap<void>(await api.deleteDeviceBinding(deviceId)),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["bindings"] }),
        queryClient.invalidateQueries({ queryKey: DEVICES_KEY }),
      ]);
    },
  });
  const listedDevices = states.length === 0 ? [] : (devices.data ?? []);
  const query = search.trim().toLowerCase();
  const visibleDevices = listedDevices.filter((device) => {
    const binding = device.convergence.binding.target;
    const seat = binding?.state === "bound" ? binding.context.seat_code : null;
    return (
      connectionFilter[device.convergence.connection_state] &&
      bindingFilter[binding?.state ?? "unknown"] &&
      (!query ||
        [seat, device.device_id, device.machine_hardware_id].some((value) =>
          value?.toLowerCase().includes(query),
        ))
    );
  });
  const hasActiveFilters =
    search !== "" ||
    connectionStates.some((state) => !connectionFilter[state]) ||
    bindingOptions.some(({ value }) => !bindingFilter[value]) ||
    stateFilter.enabled !== defaultDeviceStateFilter.enabled ||
    stateFilter.disabled !== defaultDeviceStateFilter.disabled ||
    stateFilter.revoked !== defaultDeviceStateFilter.revoked;

  const isAdmin = session?.role === "admin";
  const { isPending: isUpdatingLifecycle, mutate: updateLifecycle } = lifecycle;
  const { isPending: isUnbinding, mutate: unbindDevice } = unbind;
  const columns = useMemo(() => {
    const columns: DataTableColumnDef<Device>[] = [
      {
        id: "seat",
        header: "Seat",
        accessorFn: (device) =>
          device.convergence.binding.target?.state === "bound"
            ? device.convergence.binding.target.context.seat_code
            : undefined,
        enableSorting: true,
        sortFn: "alphanumeric",
        sortUndefined: "last",
        sortDescFirst: false,
        cell: ({ getValue }) => (
          <span className="font-medium tabular-nums">
            {getValue<string | undefined>() ?? "—"}
          </span>
        ),
      },
      {
        accessorKey: "machine_hardware_id",
        header: "Device",
        cell: ({ row }) => {
          const device = row.original;
          const connection = connections[device.convergence.connection_state];
          return (
            <div className="flex items-center gap-1.5">
              <StatusIcon
                {...connection}
                tone={
                  device.state !== "enabled"
                    ? "text-muted-foreground"
                    : connection.tone
                }
                label={`Connection: ${connection.label}`}
              />
              <code
                className="text-xs text-muted-foreground"
                title={device.machine_hardware_id}
              >
                {device.machine_hardware_id.length > 12
                  ? `${device.machine_hardware_id.slice(0, 8)}…`
                  : device.machine_hardware_id}
              </code>
              <DeviceIpWarning
                device={device}
                onView={() => setSelectedDeviceId(device.device_id)}
              />
            </div>
          );
        },
      },
      {
        accessorKey: "state",
        header: () => <span className="block text-center">Lifecycle</span>,
        cell: ({ row }) => (
          <div className="flex justify-center">
            <StatusIcon
              icon={
                row.original.state === "enabled"
                  ? "check"
                  : row.original.state === "disabled"
                    ? "pause"
                    : "ban"
              }
              tone={
                row.original.state === "enabled"
                  ? "text-emerald-700"
                  : row.original.state === "disabled"
                    ? "text-muted-foreground"
                    : "text-destructive"
              }
              label={`Lifecycle: ${row.original.state}`}
            />
          </div>
        ),
      },
      {
        accessorKey: "evidence_quality",
        header: () => <span className="block text-center">Evidence</span>,
        cell: ({ row }) => (
          <div className="flex justify-center">
            <StatusIcon
              icon={
                row.original.evidence_quality === "strong"
                  ? "shieldCheck"
                  : "shield"
              }
              tone={
                row.original.evidence_quality === "strong"
                  ? "text-emerald-700"
                  : "text-amber-700"
              }
              label={`Evidence: ${row.original.evidence_quality}`}
            />
          </div>
        ),
      },
      {
        id: "binding",
        header: () => <span className="block text-center">Binding</span>,
        cell: ({ row }) => {
          const binding = row.original.convergence.binding;
          const state = binding.target?.state;
          return (
            <div className="flex justify-center gap-1">
              <StatusIcon
                icon={
                  state === "bound"
                    ? "linked"
                    : state === "unbound"
                      ? "unlinked"
                      : "unknown"
                }
                tone={
                  state === "bound"
                    ? "text-emerald-700"
                    : "text-muted-foreground"
                }
                label={`Binding: ${state ?? "unknown"}`}
              />
              <ConvergenceIcon name="Binding" status={binding.status} />
            </div>
          );
        },
      },
      ...(
        [
          ["gateway", "Gateway"],
          ["runtime_config", "Runtime"],
          ["session_control", "Session"],
          ["home", "Home"],
        ] as const
      ).map(([key, name]): DataTableColumnDef<Device> => ({
        id: key,
        header: () => <span className="block text-center">{name}</span>,
        cell: ({ row }) => (
          <div className="flex justify-center">
            <ConvergenceIcon
              name={name}
              status={row.original.convergence[key].status}
            />
          </div>
        ),
      })),
      {
        id: "details",
        header: "Details",
        cell: ({ row }) => (
          <Button
            type="button"
            variant="outline"
            size="sm"
            aria-expanded={selectedDeviceId === row.original.device_id}
            onClick={() =>
              setSelectedDeviceId((current) =>
                current === row.original.device_id
                  ? null
                  : row.original.device_id,
              )
            }
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
          const binding = device.convergence.binding.target;
          return (
            <div className="flex gap-2">
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="w-20"
                    disabled={isUnbinding || binding?.state !== "bound"}
                  >
                    Unbind
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>
                      {binding?.state === "bound"
                        ? `Unbind seat ${binding.context.seat_code}?`
                        : "Unbind this device?"}
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                      Device{" "}
                      <code className="break-all">
                        {device.machine_hardware_id}
                      </code>{" "}
                      (<code className="break-all">{device.device_id}</code>)
                      will return to seat selection when it is enabled,
                      connected, and ready for binding.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction
                      className="bg-destructive text-white hover:bg-destructive/90"
                      disabled={isUnbinding || binding?.state !== "bound"}
                      onClick={() => unbindDevice(device.device_id)}
                    >
                      Unbind
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="w-20"
                disabled={isUpdatingLifecycle || device.state === "revoked"}
                onClick={() =>
                  updateLifecycle({
                    deviceId: device.device_id,
                    state: device.state === "enabled" ? "disabled" : "enabled",
                  })
                }
              >
                {device.state === "enabled" ? "Disable" : "Enable"}
              </Button>
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button
                    type="button"
                    variant="destructive"
                    size="sm"
                    className="w-20"
                    disabled={isUpdatingLifecycle || device.state === "revoked"}
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
                      disabled={
                        isUpdatingLifecycle || device.state === "revoked"
                      }
                      onClick={() =>
                        updateLifecycle({
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
            </div>
          );
        },
      });
    }
    return columns;
  }, [
    isAdmin,
    isUpdatingLifecycle,
    updateLifecycle,
    isUnbinding,
    unbindDevice,
    selectedDeviceId,
  ]);

  return (
    <div className="min-w-0 space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0 flex-1 basis-64">
          <h1 className="text-2xl font-semibold tracking-tight">Devices</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Manage device lifecycle, seat bindings, and current convergence.
          </p>
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-3">
          <DeviceEmergencyPanel />
          <RefreshCountdown
            updatedAt={Math.max(devices.dataUpdatedAt, devices.errorUpdatedAt)}
            isFetching={devices.isFetching}
            isPaused={devices.isPaused}
            isError={devices.isError}
          />
        </div>
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

      {unbind.error && (
        <Alert variant="destructive">
          <AlertTitle>
            {unbind.error instanceof ApiError
              ? unbind.error.title
              : "Unbind failed"}
          </AlertTitle>
        </Alert>
      )}

      <section aria-label="Device filters" className="space-y-3">
        <div className="flex flex-wrap items-center gap-3">
          <div className="relative min-w-0 flex-1 basis-64 sm:max-w-sm">
            <Search
              aria-hidden="true"
              className="pointer-events-none absolute top-2.5 left-3 size-4 text-muted-foreground"
            />
            <Input
              type="search"
              aria-label="Search devices"
              placeholder="Search by seat, device ID, or hardware ID…"
              className="pl-9"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          </div>
          <div className="flex flex-wrap items-center gap-3 sm:ml-auto">
            {(states.length === 0 || devices.data) && (
              <p
                role="status"
                className="text-sm text-muted-foreground tabular-nums"
              >
                Showing {visibleDevices.length} of {listedDevices.length}{" "}
                devices
              </p>
            )}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={!hasActiveFilters}
              onClick={() => {
                setSearch("");
                setStateFilter(defaultDeviceStateFilter);
                setConnectionFilter(defaultConnectionFilter);
                setBindingFilter(defaultBindingFilter);
              }}
            >
              <RotateCcw aria-hidden="true" />
              Reset filters
            </Button>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <MultiSelectFilter
            label="Lifecycle"
            options={lifecycleOptions}
            value={stateFilter}
            onChange={setStateFilter}
          />
          <MultiSelectFilter
            label="Connection"
            options={connectionOptions}
            value={connectionFilter}
            onChange={setConnectionFilter}
          />
          <MultiSelectFilter
            label="Binding"
            options={bindingOptions}
            value={bindingFilter}
            onChange={setBindingFilter}
          />
        </div>
      </section>
      <DataState
        isLoading={devices.isLoading}
        error={devices.data ? null : devices.error}
        isEmpty={!visibleDevices.length}
        emptyLabel="No devices found."
      >
        <div className="space-y-6">
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-x-5 gap-y-1 text-xs text-muted-foreground">
              {Object.entries(connections).map(([state, connection]) => (
                <span key={state} className="flex items-center gap-1">
                  <span className={`rounded ${connection.row}`}>
                    <StatusIcon {...connection} />
                  </span>
                  {connection.label}
                </span>
              ))}
              <span className="flex items-center gap-1">
                <span className="rounded bg-muted/60">
                  <StatusIcon
                    icon="pause"
                    tone="text-muted-foreground"
                    label="Disabled"
                  />
                </span>
                Disabled
              </span>
            </div>
            <DataTable
              columns={columns}
              data={visibleDevices}
              getRowId={(device) => device.device_id}
              renderExpandedRow={(device) =>
                device.device_id === selectedDeviceId ? (
                  <DeviceConvergence device={device} />
                ) : null
              }
              rowClassName={(device) =>
                device.state !== "enabled"
                  ? "bg-muted/60 hover:bg-muted"
                  : connections[device.convergence.connection_state].row
              }
            />
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
              {(Object.keys(convergenceIcons) as ConvergenceStatus[]).map(
                (status) => (
                  <span key={status} className="flex items-center gap-1">
                    <ConvergenceIcon name="Convergence" status={status} />
                    {label(status)}
                  </span>
                ),
              )}
              <span className="flex items-center gap-1">
                <StatusIcon
                  icon="linked"
                  tone="text-emerald-700"
                  label="Bound"
                />
                bound
              </span>
              <span className="flex items-center gap-1">
                <StatusIcon
                  icon="unlinked"
                  tone="text-muted-foreground"
                  label="Unbound"
                />
                unbound
              </span>
            </div>
          </div>
        </div>
      </DataState>
    </div>
  );
}

function DeviceConvergence({ device }: { device: Device }) {
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
