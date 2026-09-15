import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderSync, Monitor, MonitorPlay } from "lucide-react";

import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DataState } from "@/components/data-state";
import { DataTable } from "@/components/data-table";
import {
  DeviceStateFilter,
  type DeviceStateFilterValue,
} from "@/components/device-state-filter";
import {
  defaultDeviceStateFilter,
  selectedDeviceStates,
} from "@/components/device-state";
import {
  ConvergenceIcon,
  RefreshCountdown,
  StatusIcon,
} from "@/components/device-status";
import { connections } from "@/components/device-status-style";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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

import { BulkTargetActions } from "./target-bulk-actions";
import { TargetSubmissionResult } from "./target-submission-result";
import { targetAction, type TargetOperation } from "./target-operations";

type Device = components["schemas"]["DeviceResponse"];
type Convergence = components["schemas"]["DeviceConvergenceResponse"];

const DEVICES_KEY = ["devices"] as const;

export function TargetsPage() {
  const { api, targetSubmission } = useSessionScope();
  const session = useSession().data;
  const [deviceId, setDeviceId] = useState("");
  const [search, setSearch] = useState("");
  const [stateFilter, setStateFilter] = useState<DeviceStateFilterValue>(
    defaultDeviceStateFilter,
  );
  const states = selectedDeviceStates(stateFilter);
  const submission = useSyncExternalStore(
    targetSubmission.subscribe,
    targetSubmission.getSnapshot,
  );
  const results = submission.completed?.response.results;
  const targetsPending =
    submission.pending !== null || !submission.storageReady;
  const panel = useRef<HTMLDivElement>(null);
  const devices = useQuery({
    queryKey: [...DEVICES_KEY, states],
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(
        await api.GET("/api/v2/devices", {
          signal,
          params: { query: { state: states } },
        }),
      ),
    refetchInterval: LIST_POLL_MS,
    enabled: states.length > 0,
  });
  const selectedDevice = devices.data?.find(
    (device) => device.device_id === deviceId,
  );
  const isAdmin = session?.role === "admin";
  const enabledDevices = useQuery({
    queryKey: [...DEVICES_KEY, ["enabled"]],
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(
        await api.GET("/api/v2/devices", {
          signal,
          params: { query: { state: ["enabled"] } },
        }),
      ),
    enabled: isAdmin,
    refetchInterval: LIST_POLL_MS,
  });
  useEffect(() => {
    if (deviceId) panel.current?.scrollIntoView({ block: "start" });
  }, [deviceId]);
  const rows = useMemo(() => {
    const query = search.trim().toLowerCase();
    return (states.length === 0 ? [] : (devices.data ?? [])).filter(
      (device) =>
        !query ||
        [seatCode(device), device.device_id, device.machine_hardware_id].some(
          (value) => value?.toLowerCase().includes(query),
        ),
    );
  }, [devices.data, search, states.length]);
  const columns = useMemo(() => {
    const columns: ColumnDef<Device>[] = [
      {
        id: "seat",
        header: "Seat",
        accessorFn: seatCode,
        enableSorting: true,
        sortingFn: "alphanumeric",
        sortUndefined: "last",
        sortDescFirst: false,
        cell: ({ getValue }) => (
          <span className="font-semibold">
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
            <div className="flex items-center gap-1.5" title={device.device_id}>
              <StatusIcon
                {...connection}
                label={`Connection: ${connection.label}`}
                tone={
                  device.state === "enabled"
                    ? connection.tone
                    : "text-muted-foreground"
                }
              />
              <code
                className="text-xs text-muted-foreground"
                title={device.machine_hardware_id}
              >
                {device.machine_hardware_id.length > 12
                  ? `${device.machine_hardware_id.slice(0, 8)}…`
                  : device.machine_hardware_id}
              </code>
            </div>
          );
        },
      },
      {
        accessorKey: "state",
        header: "Lifecycle",
        cell: ({ row }) => (
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
                : "text-muted-foreground"
            }
            label={`Lifecycle: ${row.original.state}`}
          />
        ),
      },
      {
        id: "foreground-target",
        header: "Target",
        cell: ({ row }) =>
          row.original.convergence.session_control.target?.foreground_target ??
          "—",
      },
      {
        id: "foreground-actual",
        header: "Actual",
        cell: ({ row }) =>
          row.original.convergence.session_control.actual?.foreground ?? "—",
      },
      ...(
        [
          ["session_control", "Session"],
          ["home", "Home"],
        ] as const
      ).map(([key, name]): ColumnDef<Device> => ({
        id: key,
        header: name,
        cell: ({ row }) => (
          <ConvergenceIcon
            name={
              devices.isFetching || devices.isError || targetsPending
                ? `${name} (last known)`
                : name
            }
            status={row.original.convergence[key].status}
          />
        ),
      })),
    ];
    if (results?.length) {
      const byDevice = new Map(
        results.map((result) => [result.device_id, result]),
      );
      columns.push({
        id: "submission",
        header: "Last submission",
        cell: ({ row }) => {
          const result = byDevice.get(row.original.device_id);
          if (!result) return "—";
          return (
            <StatusIcon
              icon={result.status === "rejected" ? "failed" : "check"}
              tone={
                result.status === "rejected"
                  ? "text-destructive"
                  : "text-emerald-700"
              }
              label={
                result.status === "rejected"
                  ? `Rejected: ${result.message}`
                  : "Target submitted; check convergence for completion"
              }
            />
          );
        },
      });
    }
    columns.push({
      id: "actions",
      header: "Actions",
      cell: ({ row }) => (
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="w-20"
          aria-pressed={deviceId === row.original.device_id}
          onClick={() => setDeviceId(row.original.device_id)}
        >
          {isAdmin ? "Manage" : "View"}
        </Button>
      ),
    });
    return columns;
  }, [
    deviceId,
    isAdmin,
    results,
    devices.isFetching,
    devices.isError,
    targetsPending,
  ]);

  return (
    <div className="min-w-0 space-y-6">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Targets</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Manage Session and Home targets for all devices or one seat.
          </p>
        </div>
        <RefreshCountdown
          updatedAt={Math.max(devices.dataUpdatedAt, devices.errorUpdatedAt)}
          isFetching={devices.isFetching}
          isPaused={devices.isPaused}
          isError={devices.isError}
        />
      </div>
      {devices.isError && (
        <Alert variant="destructive">
          <AlertTitle>Refresh failed</AlertTitle>
          <AlertDescription>
            <p>
              {devices.data
                ? "Showing the last successful result; it may be outdated."
                : "Device targets are unavailable."}
            </p>
            <Button
              variant="outline"
              size="sm"
              disabled={devices.isFetching}
              onClick={() => void devices.refetch()}
            >
              Retry
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {isAdmin &&
        (submission.pending || submission.completed || submission.error) && (
          <TargetSubmissionResult
            key={
              submission.pending?.operation_id ??
              submission.completed?.request.operation_id ??
              "error"
            }
            submission={submission}
            devices={[...(enabledDevices.data ?? []), ...(devices.data ?? [])]}
            onRetry={() => void targetSubmission.retry()}
          >
            {submission.completed && (
              <>
                {submission.completed.request.action.kind !== "power_off" &&
                  submission.completed.response.results.some(
                    (row) => row.status === "rejected",
                  ) && (
                    <TargetAction
                      label="Retry failed devices"
                      title="Retry the rejected devices?"
                      description="This submits a new operation for the rejected devices only. The Server checks whether they are still enabled. Successful devices will not be submitted again."
                      disabled={targetsPending}
                      onConfirm={() => {
                        const completed = submission.completed;
                        if (completed)
                          void targetSubmission.submit(
                            completed.request.action,
                            {
                              kind: "devices",
                              device_ids: completed.response.results
                                .filter((row) => row.status === "rejected")
                                .map((row) => row.device_id),
                            },
                          );
                      }}
                    />
                  )}
              </>
            )}
          </TargetSubmissionResult>
        )}
      {isAdmin && (
        <>
          <BulkTargetActions
            devices={enabledDevices.data ?? []}
            disabled={
              targetsPending || !enabledDevices.data || enabledDevices.isError
            }
            onSubmit={(operation) =>
              void targetSubmission.submit(targetAction(operation), {
                kind:
                  operation === "poweroff"
                    ? "all_online_enabled"
                    : "all_enabled",
              })
            }
          />
        </>
      )}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <DeviceStateFilter value={stateFilter} onChange={setStateFilter} />
        <div className="flex w-full items-center gap-3 sm:w-auto">
          <Input
            type="search"
            aria-label="Search devices"
            placeholder="Find a seat or device"
            className="min-w-0 sm:w-56"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <p className="shrink-0 text-sm text-muted-foreground">
            {rows.length} of {devices.data?.length ?? 0} devices
          </p>
        </div>
      </div>
      <DataState
        isLoading={devices.isLoading}
        error={devices.data ? null : devices.error}
        isEmpty={!devices.data?.length}
        emptyLabel="No devices found."
      >
        <div className="space-y-3">
          <DataTable
            columns={columns}
            data={rows}
            getRowId={(device) => device.device_id}
            rowClassName={(device) =>
              device.state === "enabled"
                ? connections[device.convergence.connection_state].row
                : "bg-muted/60 hover:bg-muted"
            }
          />
          {targetsPending && (
            <p role="status" className="text-sm text-muted-foreground">
              Targets are being submitted. Displaying last known device states.
            </p>
          )}
          <p className="text-xs text-muted-foreground">
            Offline devices use warning rows; disabled and revoked devices are
            gray. Submitted targets are complete only when device convergence
            confirms them.
          </p>
        </div>
      </DataState>
      {devices.data && (
        <div className="text-sm text-muted-foreground">
          <p>
            Last successful refresh: {formatTimestamp(devices.dataUpdatedAt)}
          </p>
          {devices.isFetching && (
            <p role="status">Refreshing. Showing previous result.</p>
          )}
        </div>
      )}
      <div ref={panel}>
        {selectedDevice && (
          <Card>
            <CardHeader>
              <div className="flex items-center justify-between gap-4">
                <CardTitle>
                  {seatCode(selectedDevice) ?? "Unbound device"} ·{" "}
                  {selectedDevice.machine_hardware_id}
                </CardTitle>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setDeviceId("")}
                >
                  Close details
                </Button>
              </div>
              <CardDescription className="break-all font-mono">
                {selectedDevice.device_id}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <DeviceTargets
                key={deviceId}
                device={selectedDevice}
                isAdmin={isAdmin}
                previous={
                  devices.isFetching || devices.isError || targetsPending
                }
                disabled={targetsPending || selectedDevice.state !== "enabled"}
                onSubmit={(operation) =>
                  void targetSubmission.submit(targetAction(operation), {
                    kind: "devices",
                    device_ids: [selectedDevice.device_id],
                  })
                }
              />
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}

function seatCode(device: Device) {
  const target = device.convergence.binding.target;
  return target?.state === "bound" ? target.context.seat_code : undefined;
}

function DeviceTargets({
  device,
  isAdmin,
  previous,
  disabled,
  onSubmit,
}: {
  device: Device;
  isAdmin: boolean;
  previous: boolean;
  disabled: boolean;
  onSubmit: (operation: TargetOperation) => void;
}) {
  const { session_control: session, home } = device.convergence;
  const showPrevious = previous;
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-md bg-muted/30 px-4 py-3 text-sm text-muted-foreground">
        <div className="flex items-center gap-2">
          <StatusIcon
            {...connections[device.convergence.connection_state]}
            label={connections[device.convergence.connection_state].label}
          />
          <span>Connection: {label(device.convergence.connection_state)}</span>
        </div>
        <p className="text-xs">
          Last device report:{" "}
          {formatTimestamp(device.convergence.received_at_unix_ms)}
        </p>
        {device.convergence.connection_state === "offline" && (
          <p className="w-full text-xs">
            Device offline. Waiting for a connection and a fresh state report.
          </p>
        )}
        {device.convergence.connection_state === "awaiting_fresh_state" && (
          <p className="w-full text-xs">
            Device connected. Waiting for a fresh state report.
          </p>
        )}
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <section
          aria-labelledby="session-control-title"
          className="flex flex-col gap-4 rounded-md border p-4"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3
              id="session-control-title"
              className="flex items-center gap-2 font-medium"
            >
              <Monitor
                aria-hidden="true"
                className="size-4 text-muted-foreground"
              />
              Session Control
            </h3>
            <ConvergenceStatus
              status={session.status}
              previous={showPrevious}
            />
          </div>
          <div className="grid grid-cols-2 gap-3 rounded-md bg-muted/30 p-3">
            <TargetFact name="Target foreground">
              {session.target?.foreground_target ?? "not initialized"}
            </TargetFact>
            <TargetFact name="Actual foreground">
              {session.actual?.foreground ?? "not received"}
            </TargetFact>
          </div>
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            <TargetFact name="Waiting display ready">
              {session.actual
                ? String(session.actual.waiting_ready)
                : "not received"}
            </TargetFact>
            <TargetFact name="Contest desktop ready">
              {session.actual
                ? String(session.actual.contest_ready)
                : "not received"}
            </TargetFact>
            <TargetFact name="Actual session">
              {session.actual
                ? label(session.actual.session_state)
                : "not received"}
            </TargetFact>
          </div>
          <div className="grid grid-cols-2 gap-4 border-t pt-3">
            <TargetFact name="Terminate epoch">
              {session.target?.terminate_epoch ?? "none"}
            </TargetFact>
            <TargetFact name="Completed terminate epoch">
              {session.actual?.completed_terminate_epoch ?? "none"}
            </TargetFact>
          </div>
          {session.target?.foreground_target === "contest" &&
            device.convergence.binding.target?.state === "unbound" && (
              <p className="rounded-md bg-amber-500/10 p-3 text-sm text-amber-700">
                Waiting for binding before showing the contest desktop.
              </p>
            )}
          {session.actual?.session_state === "ambiguous" && (
            <p className="rounded-md bg-destructive/5 p-3 text-sm text-destructive">
              Cannot identify a single session.
            </p>
          )}
          {session.actual?.session_state === "error" && (
            <p className="rounded-md bg-destructive/5 p-3 text-sm text-destructive">
              The device reported a session error.
            </p>
          )}
          {isAdmin && (
            <div className="mt-auto flex flex-wrap gap-2 border-t pt-4">
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={disabled}
                onClick={() => onSubmit("waiting")}
              >
                <Monitor aria-hidden="true" />
                Show waiting screen
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={disabled}
                onClick={() => onSubmit("contest")}
              >
                <MonitorPlay aria-hidden="true" />
                Show contest desktop
              </Button>
              <TargetAction
                label="Terminate"
                title="Terminate the contest session?"
                description="End and restart this device's contest session. Home files are preserved. Check its reported state to confirm completion."
                disabled={disabled}
                onConfirm={() => onSubmit("terminate")}
              />
            </div>
          )}
        </section>

        <section
          aria-labelledby="home-title"
          className="flex flex-col gap-4 rounded-md border p-4"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 id="home-title" className="flex items-center gap-2 font-medium">
              <FolderSync
                aria-hidden="true"
                className="size-4 text-muted-foreground"
              />
              Home
            </h3>
            <ConvergenceStatus status={home.status} previous={showPrevious} />
          </div>
          <div className="rounded-md bg-muted/30 p-3">
            <TargetFact name="Actual home">
              {home.actual ? label(home.actual.state) : "not received"}
            </TargetFact>
          </div>
          <div className="grid grid-cols-2 gap-4">
            <TargetFact name="Reset epoch">
              {home.target_reset_epoch ?? "none"}
            </TargetFact>
            <TargetFact name="Completed reset epoch">
              {home.actual?.completed_reset_epoch ?? "none"}
            </TargetFact>
          </div>
          {home.actual?.state === "recovery_required" && (
            <p className="rounded-md bg-destructive/5 p-3 text-sm text-destructive">
              The device requires Home recovery.
            </p>
          )}
          {isAdmin && (
            <div className="mt-auto border-t pt-4">
              <TargetAction
                label="Reset home"
                title="Reset the contest Home?"
                description="Delete the contest user's Home data and restore the default Home. The contest session will restart. Check its reported state to confirm completion."
                disabled={disabled}
                onConfirm={() => onSubmit("reset")}
              />
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function TargetFact({
  name,
  children,
}: {
  name: string;
  children: string | number;
}) {
  return (
    <p className="space-y-1">
      <span className="block text-xs text-muted-foreground">{name}: </span>
      <span className="block text-sm font-medium tabular-nums">{children}</span>
    </p>
  );
}

function ConvergenceStatus({
  status,
  previous,
}: {
  status: Convergence["home"]["status"];
  previous: boolean;
}) {
  return (
    <Badge variant={status === "failed" ? "destructive" : "outline"}>
      {previous ? "Last known convergence" : "Convergence"}: {label(status)}
    </Badge>
  );
}

function label(value: string) {
  return value.replaceAll("_", " ");
}

function formatTimestamp(value: number | null) {
  return value === null ? "not received" : new Date(value).toLocaleString();
}

function TargetAction({
  label,
  title,
  description,
  disabled,
  onConfirm,
}: {
  label: string;
  title: string;
  description: string;
  disabled: boolean;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button
          type="button"
          variant="destructive"
          size="sm"
          disabled={disabled}
        >
          {label}
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            className="bg-destructive text-white hover:bg-destructive/90"
            disabled={disabled}
            onClick={onConfirm}
          >
            {label}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
