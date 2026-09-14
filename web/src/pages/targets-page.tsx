import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useQuery } from "@tanstack/react-query";

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
import {
  targetAction,
  targetActionLabel,
  type TargetOperation,
} from "./target-operations";

type Device = components["schemas"]["DeviceResponse"];
type Convergence = components["schemas"]["DeviceConvergenceResponse"];

const DEVICES_KEY = ["devices"] as const;

export function TargetsPage() {
  const { api, targetSubmission } = useSessionScope();
  const session = useSession().data;
  const [deviceId, setDeviceId] = useState("");
  const [search, setSearch] = useState("");
  const [stateFilter, setStateFilter] =
    useState<DeviceStateFilterValue>("non_revoked");
  const submission = useSyncExternalStore(
    targetSubmission.subscribe,
    targetSubmission.getSnapshot,
  );
  const results = submission.completed?.response.results;
  const currentRequest = submission.pending ?? submission.completed?.request;
  const targetsPending =
    submission.pending !== null || !submission.storageReady;
  const panel = useRef<HTMLDivElement>(null);
  const devices = useQuery({
    queryKey: [...DEVICES_KEY, stateFilter],
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(
        await api.GET("/api/v2/devices", {
          signal,
          params: { query: { state: stateFilter } },
        }),
      ),
    refetchInterval: LIST_POLL_MS,
  });
  const selectedDevice = devices.data?.find(
    (device) => device.device_id === deviceId,
  );
  const isAdmin = session?.role === "admin";
  // All-device actions keep their global Enabled scope even when this view
  // shows only disabled/revoked devices. Other filters already include Enabled.
  const excludesEnabled =
    stateFilter === "disabled" || stateFilter === "revoked";
  const enabledDevices = useQuery({
    queryKey: [...DEVICES_KEY, "enabled"],
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(
        await api.GET("/api/v2/devices", {
          signal,
          params: { query: { state: "enabled" } },
        }),
      ),
    enabled: isAdmin && excludesEnabled,
    refetchInterval: LIST_POLL_MS,
  });
  const batchDevices = excludesEnabled ? enabledDevices : devices;
  useEffect(() => {
    if (deviceId) panel.current?.scrollIntoView({ block: "start" });
  }, [deviceId]);
  const rows = useMemo(() => {
    const query = search.trim().toLowerCase();
    return (devices.data ?? []).filter(
      (device) =>
        !query ||
        [seatCode(device), device.device_id, device.machine_hardware_id].some(
          (value) => value?.toLowerCase().includes(query),
        ),
    );
  }, [devices.data, search]);
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
          <section
            aria-label="Target submission"
            className="space-y-3 rounded-md border p-4"
          >
            {currentRequest && (
              <p className="font-medium">
                {targetActionLabel(currentRequest.action)}
                {" · "}
                {currentRequest.scope.kind === "all_enabled"
                  ? "All enabled devices"
                  : "Selected devices"}
              </p>
            )}
            {currentRequest?.scope.kind === "devices" && (
              <p className="text-sm break-words text-muted-foreground">
                {currentRequest.scope.device_ids
                  .map((id) => {
                    const device = devices.data?.find(
                      (device) => device.device_id === id,
                    );
                    return device ? (seatCode(device) ?? id) : id;
                  })
                  .join(", ")}
              </p>
            )}
            {submission.sending ? (
              <p role="status">
                Submitting target. Showing previous device states.
              </p>
            ) : submission.pending ? (
              <Alert>
                <AlertTitle>Submission result not confirmed</AlertTitle>
                <AlertDescription>
                  <p>
                    Retry the original request to confirm its result. New target
                    actions are paused until the result is known.
                  </p>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={!submission.storageReady}
                    onClick={() => void targetSubmission.retry()}
                  >
                    Retry original request
                  </Button>
                </AlertDescription>
              </Alert>
            ) : submission.error ? (
              <Alert variant="destructive">
                <AlertTitle>{submission.error}</AlertTitle>
              </Alert>
            ) : null}
            {submission.completed && (
              <>
                <p role="status">
                  {
                    submission.completed.response.results.filter(
                      (row) => row.status === "submitted",
                    ).length
                  }{" "}
                  submitted,{" "}
                  {
                    submission.completed.response.results.filter(
                      (row) => row.status === "rejected",
                    ).length
                  }{" "}
                  rejected. Check device convergence for completion.
                </p>
                {submission.completed.response.results.some(
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
                        void targetSubmission.submit(completed.request.action, {
                          kind: "devices",
                          device_ids: completed.response.results
                            .filter((row) => row.status === "rejected")
                            .map((row) => row.device_id),
                        });
                    }}
                  />
                )}
              </>
            )}
          </section>
        )}
      {isAdmin && (
        <>
          <BulkTargetActions
            devices={batchDevices.data ?? []}
            disabled={
              targetsPending || !batchDevices.data || batchDevices.isError
            }
            onSubmit={(operation) =>
              void targetSubmission.submit(targetAction(operation), {
                kind: "all_enabled",
              })
            }
          />
          {excludesEnabled && enabledDevices.isError && (
            <Alert variant="destructive">
              <AlertTitle>Enabled device list unavailable</AlertTitle>
              <AlertDescription>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={enabledDevices.isFetching}
                  onClick={() => void enabledDevices.refetch()}
                >
                  Retry enabled devices
                </Button>
              </AlertDescription>
            </Alert>
          )}
        </>
      )}
      <DeviceStateFilter value={stateFilter} onChange={setStateFilter} />
      <DataState
        isLoading={devices.isLoading}
        error={devices.data ? null : devices.error}
        isEmpty={!devices.data?.length}
        emptyLabel="No devices found."
      >
        <div className="space-y-3">
          <div className="flex items-center justify-between gap-4">
            <Input
              type="search"
              aria-label="Search devices"
              placeholder="Find a seat or device"
              className="max-w-xs"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
            <p className="text-sm text-muted-foreground">
              {rows.length} of {devices.data?.length} devices
            </p>
          </div>
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
      <div className="space-y-2 text-sm text-muted-foreground">
        <p>Connection: {label(device.convergence.connection_state)}</p>
        <p>
          Last device report:{" "}
          {formatTimestamp(device.convergence.received_at_unix_ms)}
        </p>
        {device.convergence.connection_state === "offline" && (
          <p>
            Device offline. Waiting for a connection and a fresh state report.
          </p>
        )}
        {device.convergence.connection_state === "awaiting_fresh_state" && (
          <p>Device connected. Waiting for a fresh state report.</p>
        )}
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <section
          aria-labelledby="session-control-title"
          className="space-y-3 rounded-md border p-4"
        >
          <h3 id="session-control-title" className="font-medium">
            Session Control
          </h3>
          <ConvergenceStatus status={session.status} previous={showPrevious} />
          <div className="space-y-1 text-sm">
            <p>
              Target foreground:{" "}
              {session.target?.foreground_target ?? "not initialized"}
            </p>
            <p>
              Actual foreground: {session.actual?.foreground ?? "not received"}
            </p>
            <p>
              Waiting display ready:{" "}
              {session.actual
                ? String(session.actual.waiting_ready)
                : "not received"}
            </p>
            <p>
              Contest desktop ready:{" "}
              {session.actual
                ? String(session.actual.contest_ready)
                : "not received"}
            </p>
            {session.target?.foreground_target === "contest" &&
              device.convergence.binding.target?.state === "unbound" && (
                <p>Waiting for binding before showing the contest desktop.</p>
              )}
            <p>Terminate epoch: {session.target?.terminate_epoch ?? "none"}</p>
            <p>
              Actual session:{" "}
              {session.actual
                ? label(session.actual.session_state)
                : "not received"}
            </p>
            <p>
              Completed terminate epoch:{" "}
              {session.actual?.completed_terminate_epoch ?? "none"}
            </p>
            {session.actual?.session_state === "ambiguous" && (
              <p className="text-destructive">
                Cannot identify a single session.
              </p>
            )}
            {session.actual?.session_state === "error" && (
              <p className="text-destructive">
                The device reported a session error.
              </p>
            )}
          </div>
          {isAdmin && (
            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={disabled}
                onClick={() => onSubmit("waiting")}
              >
                Show waiting screen
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={disabled}
                onClick={() => onSubmit("contest")}
              >
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
          className="space-y-3 rounded-md border p-4"
        >
          <h3 id="home-title" className="font-medium">
            Home
          </h3>
          <ConvergenceStatus status={home.status} previous={showPrevious} />
          <div className="space-y-1 text-sm">
            <p>Reset epoch: {home.target_reset_epoch ?? "none"}</p>
            <p>
              Actual home:{" "}
              {home.actual ? label(home.actual.state) : "not received"}
            </p>
            <p>
              Completed reset epoch:{" "}
              {home.actual?.completed_reset_epoch ?? "none"}
            </p>
            {home.actual?.state === "recovery_required" && (
              <p className="text-destructive">
                The device requires Home recovery.
              </p>
            )}
          </div>
          {isAdmin && (
            <TargetAction
              label="Reset home"
              title="Reset the contest Home?"
              description="Delete the contest user's Home data and restore the default Home. The contest session will restart. Check its reported state to confirm completion."
              disabled={disabled}
              onConfirm={() => onSubmit("reset")}
            />
          )}
        </section>
      </div>
    </div>
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
