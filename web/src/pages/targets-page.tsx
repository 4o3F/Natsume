import { type ReactNode, useMemo, useState, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowRight,
  FolderSync,
  Monitor,
  MonitorPlay,
  RotateCcw,
  Search,
} from "lucide-react";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { DeviceResponse_Serialize as DeviceResponse } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { BulkActionBar } from "@/components/bulk-action-bar";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DataState } from "@/components/data-state";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import {
  DeviceStateFilter,
  type DeviceStateFilterValue,
} from "@/components/device-state-filter";
import {
  defaultDeviceStateFilter,
  selectedDeviceStates,
} from "@/components/device-state";
import {
  ConvergenceBadge,
  RefreshCountdown,
  StatusDot,
  StatusIcon,
} from "@/components/device-status";
import { connections, tones } from "@/components/device-status-style";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { InputGroup } from "@/components/ui/input";
import {
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { cn } from "@/lib/utils";

import { BulkTargetActions } from "./target-bulk-actions";
import { TargetSubmissionResult } from "./target-submission-result";
import {
  targetAction,
  targetOperations,
  type TargetOperation,
} from "./target-operations";

type Device = DeviceResponse;

const DEVICES_KEY = ["devices"] as const;
const lifecycleBadges = {
  enabled: { variant: "success", label: "Enabled" },
  disabled: { variant: "muted", label: "Disabled" },
  revoked: { variant: "destructive", label: "Revoked" },
} as const;

export function TargetsPage() {
  const { api, targetSubmission } = useSessionScope();
  const session = useSession().data;
  const [deviceId, setDeviceId] = useState("");
  const [search, setSearch] = useState("");
  const [checked, setChecked] = useState<Set<string>>(() => new Set());
  const [confirm, setConfirm] = useState<"terminate" | "reset" | null>(null);
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
  const devices = useQuery({
    queryKey: [...DEVICES_KEY, states],
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(await api.listDevices(states, { signal })),
    refetchInterval: LIST_POLL_MS,
    enabled: states.length > 0,
  });
  const isAdmin = session?.role === "admin";
  const enabledDevices = useQuery({
    queryKey: [...DEVICES_KEY, ["enabled"]],
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(await api.listDevices(["enabled"], { signal })),
    enabled: isAdmin,
    refetchInterval: LIST_POLL_MS,
  });
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
  const selectedDevice =
    devices.data?.find((device) => device.device_id === deviceId) ?? null;
  if (
    deviceId &&
    (states.length === 0 || (devices.data && selectedDevice === null))
  ) {
    setDeviceId("");
  }
  const checkedIds = (devices.data ?? [])
    .filter(
      (device) => checked.has(device.device_id) && device.state === "enabled",
    )
    .map((device) => device.device_id);

  function submitSelected(operation: TargetOperation) {
    void targetSubmission.submit(targetAction(operation), {
      kind: "devices",
      device_ids: checkedIds,
    });
    setChecked(new Set());
  }

  const columns = useMemo(() => {
    const columns: DataTableColumnDef<Device>[] = [
      {
        id: "seat",
        header: "Seat",
        accessorFn: seatCode,
        enableSorting: true,
        sortFn: "alphanumeric",
        sortUndefined: "last",
        sortDescFirst: false,
        cell: ({ getValue }) => {
          const seat = getValue<string | undefined>();
          return seat ? (
            <span className="font-mono text-sm font-semibold">{seat}</span>
          ) : (
            <span className="text-muted-foreground">—</span>
          );
        },
      },
      {
        accessorKey: "machine_hardware_id",
        header: "Device",
        cell: ({ row }) => {
          const device = row.original;
          const connection = connections[device.convergence.connection_state];
          const muted = device.state !== "enabled";
          return (
            <div className="flex items-center gap-2.5" title={device.device_id}>
              <StatusDot
                tone={muted ? "muted" : connection.key}
                label={`Connection: ${connection.label}`}
              />
              <code
                className="font-mono text-xs"
                title={device.machine_hardware_id}
              >
                {device.machine_hardware_id.length > 14
                  ? `${device.machine_hardware_id.slice(0, 12)}…`
                  : device.machine_hardware_id}
              </code>
              {muted && (
                <Badge variant={lifecycleBadges[device.state].variant}>
                  {lifecycleBadges[device.state].label}
                </Badge>
              )}
            </div>
          );
        },
      },
      {
        id: "foreground",
        header: "Foreground",
        cell: ({ row }) => {
          const control = row.original.convergence.session_control;
          const target = control.target?.foreground_target;
          const actual = control.actual?.foreground;
          const matches = target !== undefined && target === actual;
          return (
            <span className="flex items-center gap-1.5 text-sm">
              <ForegroundLabel value={target} />
              <ArrowRight
                aria-label="actual"
                className="size-3.5 text-muted-foreground/60"
              />
              <ForegroundLabel
                value={actual}
                className={
                  target !== undefined && actual !== undefined && !matches
                    ? tones.warning.text
                    : undefined
                }
              />
            </span>
          );
        },
      },
      {
        id: "session",
        header: "Session",
        cell: ({ row }) => (
          <ConvergenceBadge
            status={row.original.convergence.session_control.status}
          />
        ),
      },
      {
        id: "home",
        header: "Home",
        cell: ({ row }) => (
          <ConvergenceBadge status={row.original.convergence.home.status} />
        ),
      },
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
          if (!result) return <span className="text-muted-foreground">—</span>;
          return (
            <StatusIcon
              icon={result.status === "rejected" ? "failed" : "check"}
              tone={
                result.status === "rejected"
                  ? tones.danger.text
                  : tones.success.text
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
    return columns;
  }, [results]);

  const feedback = (
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">
        <span>
          Last successful refresh:{" "}
          {formatTimestamp(devices.dataUpdatedAt || null)}
        </span>
        <span role="status" className="sr-only">
          {devices.isFetching && devices.data
            ? "Refreshing. Showing previous result."
            : ""}
        </span>
      </p>
      <p className="text-xs text-muted-foreground">
        States reflect the last received device report. A submitted target is
        complete only when the device reports convergence.
      </p>
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
            {submission.completed &&
              submission.completed.request.action.kind !== "power_off" &&
              submission.completed.response.results.some(
                (row) => row.status === "rejected",
              ) && (
                <RetryRejected
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
          </TargetSubmissionResult>
        )}
    </div>
  );

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Targets"
        description="Set the desired foreground, session, and Home state for the fleet or individual seats. Devices converge on their own."
        actions={
          <RefreshCountdown
            updatedAt={Math.max(devices.dataUpdatedAt, devices.errorUpdatedAt)}
            isFetching={devices.isFetching}
            isPaused={devices.isPaused}
            isError={devices.isError}
            onRefresh={() => void devices.refetch()}
          />
        }
      />
      {feedback}
      {isAdmin && (
        <BulkTargetActions
          devices={enabledDevices.data ?? []}
          disabled={
            targetsPending || !enabledDevices.data || enabledDevices.isError
          }
          onSubmit={(operation) =>
            void targetSubmission.submit(targetAction(operation), {
              kind:
                operation === "poweroff" ? "all_online_enabled" : "all_enabled",
            })
          }
        />
      )}

      <section aria-label="Device targets" className="space-y-3">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <InputGroup
            type="search"
            aria-label="Search devices"
            placeholder="Find a seat or device"
            icon={<Search aria-hidden="true" />}
            className="w-full sm:w-64"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <DeviceStateFilter value={stateFilter} onChange={setStateFilter} />
          <p
            role="status"
            className="ml-auto text-xs text-muted-foreground tabular-nums"
          >
            {targetsPending
              ? "Submitting targets · showing last known states"
              : `${rows.length} of ${devices.data?.length ?? 0} devices`}
          </p>
        </div>
        <DataState
          isLoading={devices.isLoading}
          error={devices.data ? null : devices.error}
          isEmpty={!devices.data?.length}
          emptyLabel="No devices found"
          emptyDescription="Adjust the device state filter to include more devices."
        >
          <DataTable
            label="Device targets"
            virtualized
            columns={columns}
            data={rows}
            getRowId={(device) => device.device_id}
            onRowClick={(device) => setDeviceId(device.device_id)}
            activeRowId={deviceId || null}
            selection={
              isAdmin
                ? {
                    selected: checked,
                    onChange: setChecked,
                    isSelectable: (id) =>
                      devices.data?.find((device) => device.device_id === id)
                        ?.state === "enabled",
                  }
                : undefined
            }
            rowClassName={(device) =>
              device.state === "enabled"
                ? connections[device.convergence.connection_state].row
                : "text-muted-foreground"
            }
          />
          <p className="px-1 text-xs text-muted-foreground">
            Select rows to target specific seats.
          </p>
        </DataState>
      </section>

      <Sheet
        open={selectedDevice !== null}
        onOpenChange={(open) => !open && setDeviceId("")}
      >
        <SheetContent>
          {selectedDevice && (
            <>
              <SheetHeader>
                <div className="flex items-center gap-2">
                  <SheetTitle className="font-mono">
                    {seatCode(selectedDevice) ?? "Unbound device"}
                  </SheetTitle>
                  <Badge
                    variant={lifecycleBadges[selectedDevice.state].variant}
                    dot
                  >
                    {lifecycleBadges[selectedDevice.state].label}
                  </Badge>
                </div>
                <SheetDescription
                  asChild
                  className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs"
                >
                  <dl>
                    <dt>Hardware ID</dt>
                    <dd className="font-mono break-all">
                      {selectedDevice.machine_hardware_id}
                    </dd>
                    <dt>Device ID</dt>
                    <dd className="font-mono break-all">
                      {selectedDevice.device_id}
                    </dd>
                  </dl>
                </SheetDescription>
              </SheetHeader>
              <SheetBody>
                {feedback}
                <DeviceTargets
                  key={selectedDevice.device_id}
                  device={selectedDevice}
                  isAdmin={isAdmin}
                  disabled={
                    targetsPending || selectedDevice.state !== "enabled"
                  }
                  onSubmit={(operation) =>
                    void targetSubmission.submit(targetAction(operation), {
                      kind: "devices",
                      device_ids: [selectedDevice.device_id],
                    })
                  }
                />
              </SheetBody>
            </>
          )}
        </SheetContent>
      </Sheet>

      {isAdmin && (
        <>
          <BulkActionBar
            count={checkedIds.length}
            onClear={() => setChecked(new Set())}
          >
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={targetsPending}
              onClick={() => submitSelected("waiting")}
            >
              <Monitor aria-hidden="true" />
              Waiting
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={targetsPending}
              onClick={() => submitSelected("contest")}
            >
              <MonitorPlay aria-hidden="true" />
              Contest
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={targetsPending}
              onClick={() => setConfirm("terminate")}
            >
              <RotateCcw aria-hidden="true" />
              Terminate
            </Button>
            <Button
              type="button"
              variant="destructive-soft"
              size="sm"
              disabled={targetsPending}
              onClick={() => setConfirm("reset")}
            >
              <FolderSync aria-hidden="true" />
              Reset home
            </Button>
          </BulkActionBar>
          <ConfirmDialog
            open={confirm !== null}
            onOpenChange={(open) => !open && setConfirm(null)}
            title={
              confirm &&
              `${targetOperations[confirm]} on ${checkedIds.length} selected devices?`
            }
            description={
              confirm === "reset"
                ? "This deletes the contest user's Home data on the selected devices and restores the default Home. Contest sessions will restart."
                : "Contest sessions on the selected devices will end and restart. Home files are preserved."
            }
            confirmLabel={confirm ? targetOperations[confirm] : "Confirm"}
            disabled={targetsPending}
            onConfirm={() => confirm && submitSelected(confirm)}
          />
        </>
      )}
    </div>
  );
}

function ForegroundLabel({
  value,
  className,
}: {
  value: string | undefined;
  className?: string;
}) {
  if (value === undefined)
    return <span className="text-muted-foreground">—</span>;
  return <span className={cn("capitalize", className)}>{value}</span>;
}

function RetryRejected({
  disabled,
  onConfirm,
}: {
  disabled: boolean;
  onConfirm: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={disabled}
        onClick={() => setOpen(true)}
      >
        <RotateCcw aria-hidden="true" />
        Retry failed devices
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        tone="info"
        icon={<RotateCcw aria-hidden="true" />}
        title="Retry the rejected devices?"
        description="This submits a new operation for the rejected devices only. The Server checks whether they are still enabled. Successful devices will not be submitted again."
        confirmLabel="Retry failed devices"
        disabled={disabled}
        onConfirm={onConfirm}
      />
    </>
  );
}

function seatCode(device: Device) {
  const target = device.convergence.binding.target;
  return target?.state === "bound" ? target.context.seat_code : undefined;
}

function DeviceTargets({
  device,
  isAdmin,
  disabled,
  onSubmit,
}: {
  device: Device;
  isAdmin: boolean;
  disabled: boolean;
  onSubmit: (operation: TargetOperation) => void;
}) {
  const [confirm, setConfirm] = useState<"terminate" | "reset" | null>(null);
  const { session_control: session, home } = device.convergence;
  const connection = connections[device.convergence.connection_state];
  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 rounded-lg border bg-muted/40 px-4 py-3 text-sm">
        <span className="flex items-center gap-2 font-medium">
          <StatusDot tone={connection.key} />
          {connection.label}
        </span>
        <span className="text-xs text-muted-foreground">
          Last report {formatTimestamp(device.convergence.received_at_unix_ms)}
        </span>
        {device.convergence.connection_state === "offline" && (
          <p className="w-full text-xs text-muted-foreground">
            Waiting for a connection and a fresh state report.
          </p>
        )}
        {device.convergence.connection_state === "awaiting_fresh_state" && (
          <p className="w-full text-xs text-muted-foreground">
            Connected. Waiting for a fresh state report.
          </p>
        )}
      </div>

      <TargetPanel
        icon={<Monitor aria-hidden="true" />}
        title="Session control"
        badge={<ConvergenceBadge status={session.status} />}
      >
        <div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border">
          <TargetFact name="Target foreground" emphasis>
            {session.target?.foreground_target ?? "not initialized"}
          </TargetFact>
          <TargetFact name="Actual foreground" emphasis>
            {session.actual?.foreground ?? "not received"}
          </TargetFact>
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
          <TargetFact name="Terminate epoch (target / completed)">
            {`${session.target?.terminate_epoch ?? "none"} / ${session.actual?.completed_terminate_epoch ?? "none"}`}
          </TargetFact>
        </div>
        {session.target?.foreground_target === "contest" &&
          device.convergence.binding.target?.state === "unbound" && (
            <Alert variant="warning">
              <AlertTitle>
                Waiting for binding before showing the contest desktop.
              </AlertTitle>
            </Alert>
          )}
        {session.actual?.session_state === "ambiguous" && (
          <Alert variant="destructive">
            <AlertTitle>Cannot identify a single session.</AlertTitle>
          </Alert>
        )}
        {session.actual?.session_state === "error" && (
          <Alert variant="destructive">
            <AlertTitle>The device reported a session error.</AlertTitle>
          </Alert>
        )}
        {isAdmin && (
          <div className="flex flex-wrap gap-2">
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
            <Button
              type="button"
              variant="destructive-outline"
              size="sm"
              disabled={disabled}
              onClick={() => setConfirm("terminate")}
            >
              <RotateCcw aria-hidden="true" />
              Terminate
            </Button>
          </div>
        )}
      </TargetPanel>

      <TargetPanel
        icon={<FolderSync aria-hidden="true" />}
        title="Home"
        badge={<ConvergenceBadge status={home.status} />}
      >
        <div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border">
          <TargetFact name="Actual home" emphasis>
            {home.actual ? label(home.actual.state) : "not received"}
          </TargetFact>
          <TargetFact name="Reset epoch (target / completed)">
            {`${home.target_reset_epoch ?? "none"} / ${home.actual?.completed_reset_epoch ?? "none"}`}
          </TargetFact>
        </div>
        {home.actual?.state === "recovery_required" && (
          <Alert variant="destructive">
            <AlertTitle>The device requires Home recovery.</AlertTitle>
          </Alert>
        )}
        {isAdmin && (
          <div>
            <Button
              type="button"
              variant="destructive-outline"
              size="sm"
              disabled={disabled}
              onClick={() => setConfirm("reset")}
            >
              <FolderSync aria-hidden="true" />
              Reset home
            </Button>
          </div>
        )}
      </TargetPanel>

      <ConfirmDialog
        open={confirm === "terminate"}
        onOpenChange={(open) => !open && setConfirm(null)}
        icon={<RotateCcw aria-hidden="true" />}
        title="Terminate the contest session?"
        description="End and restart this device's contest session. Home files are preserved. Check its reported state to confirm completion."
        confirmLabel="Terminate"
        disabled={disabled}
        onConfirm={() => onSubmit("terminate")}
      />
      <ConfirmDialog
        open={confirm === "reset"}
        onOpenChange={(open) => !open && setConfirm(null)}
        icon={<FolderSync aria-hidden="true" />}
        title="Reset the contest Home?"
        description="Delete the contest user's Home data and restore the default Home. The contest session will restart. Check its reported state to confirm completion."
        confirmLabel="Reset home"
        disabled={disabled}
        onConfirm={() => onSubmit("reset")}
      />
    </div>
  );
}

function TargetPanel({
  icon,
  title,
  badge,
  children,
}: {
  icon: ReactNode;
  title: string;
  badge: ReactNode;
  children: ReactNode;
}) {
  return (
    <section aria-label={title} className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="flex items-center gap-2 text-sm font-semibold [&_svg]:size-4 [&_svg]:text-muted-foreground">
          {icon}
          {title}
        </p>
        {badge}
      </div>
      {children}
    </section>
  );
}

function TargetFact({
  name,
  children,
  emphasis = false,
}: {
  name: string;
  children: string | number;
  emphasis?: boolean;
}) {
  return (
    <div className="bg-card px-3 py-2.5">
      <p className="text-xs text-muted-foreground">{name}</p>
      <p
        className={cn(
          "mt-0.5 font-mono tabular-nums",
          emphasis ? "text-sm font-semibold" : "text-xs",
        )}
      >
        {children}
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
