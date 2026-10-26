import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router";
import {
  Ban,
  CirclePause,
  CirclePlay,
  Clock3,
  Monitor,
  RotateCcw,
  Search,
  TriangleAlert,
  Unlink,
  Wifi,
  WifiOff,
} from "lucide-react";

import { usePageToast } from "@/components/use-page-toast";

import { useSessionScope } from "@/auth/session-context";
import { ApiError, unwrap } from "@/api/errors";
import type {
  DeviceConvergenceResponse_Serialize as DeviceConvergenceResponse,
  DeviceResponse_Serialize as DeviceResponse,
} from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { BulkActionBar } from "@/components/bulk-action-bar";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DeviceEmergencyPanel } from "@/components/device-emergency-panel";
import { MultiSelectFilter } from "@/components/multi-select-filter";
import { DeviceIpWarning } from "@/components/device-network-info";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import {
  defaultDeviceStateFilter,
  selectedDeviceStates,
  type DeviceStateFilterValue,
} from "@/components/device-state";
import {
  ConvergenceStrip,
  RefreshCountdown,
  StatusDot,
  StatusIcon,
} from "@/components/device-status";
import {
  connections,
  convergenceIcons,
  convergenceParts,
  tones,
} from "@/components/device-status-style";
import { PageHeader } from "@/components/page-header";
import { StatGrid, StatTile } from "@/components/stat-tile";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { InputGroup } from "@/components/ui/input";
import {
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { DeviceActions, type DeviceActionHandlers } from "./device-actions";
import { DeviceConvergence } from "./device-convergence";

type Device = DeviceResponse;
type Convergence = DeviceConvergenceResponse;
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
const lifecycleBadges = {
  enabled: { variant: "success", label: "Enabled" },
  disabled: { variant: "muted", label: "Disabled" },
  revoked: { variant: "destructive", label: "Revoked" },
} as const;

function seatOf(device: Device) {
  const binding = device.convergence.binding.target;
  return binding?.state === "bound" ? binding.context.seat_code : undefined;
}

function needsAttention(device: Device) {
  return convergenceParts.some(([key]) => {
    const status = device.convergence[key].status;
    return status === "drifted" || status === "failed";
  });
}

function only<T extends string>(keys: readonly T[], keep: T) {
  return Object.fromEntries(keys.map((key) => [key, key === keep])) as Record<
    T,
    boolean
  >;
}

function errorTitle(error: unknown, fallback: string) {
  return error instanceof ApiError ? error.title : fallback;
}

export function DevicesPage() {
  const toast = usePageToast();
  const { api } = useSessionScope();
  const session = useSession().data;
  const queryClient = useQueryClient();
  const [searchParams] = useSearchParams();
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(() => new Set());
  const [bulkConfirm, setBulkConfirm] = useState<"revoke" | "unbind" | null>(
    null,
  );
  const [search, setSearch] = useState(() => searchParams.get("q") ?? "");
  const [attentionOnly, setAttentionOnly] = useState(false);
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
  const invalidate = () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: ["bindings"] }),
      queryClient.invalidateQueries({ queryKey: DEVICES_KEY }),
    ]);
  const lifecycle = useMutation({
    mutationFn: async ({
      deviceId,
      state,
    }: {
      deviceId: string;
      state: DeviceLifecycleState;
    }) => unwrap<void>(await api.updateDevice(deviceId, { state })),
    onSuccess: async (_, { state }) => {
      toast.success(`Device ${state}`);
      await invalidate();
    },
    onError: (error) =>
      toast.error(errorTitle(error, "Device lifecycle update failed")),
  });
  const unbind = useMutation({
    mutationFn: async (deviceId: string) =>
      unwrap<void>(await api.deleteDeviceBinding(deviceId)),
    onSuccess: async () => {
      toast.success("Seat unbound");
      await invalidate();
    },
    onError: (error) => toast.error(errorTitle(error, "Unbind failed")),
  });
  const bulk = useMutation({
    mutationFn: async ({
      action,
      ids,
    }: {
      action: "enabled" | "disabled" | "revoked" | "unbind";
      ids: string[];
    }) => {
      const results = await Promise.allSettled(
        ids.map(async (id) =>
          unwrap<void>(
            action === "unbind"
              ? await api.deleteDeviceBinding(id)
              : await api.updateDevice(id, { state: action }),
          ),
        ),
      );
      return {
        action,
        failed: results.filter((result) => result.status === "rejected").length,
        total: ids.length,
      };
    },
    onSuccess: async ({ action, failed, total }) => {
      const verb = action === "unbind" ? "unbound" : action;
      if (failed === 0) toast.success(`${total} devices ${verb}`);
      else
        toast.warning(`${total - failed} of ${total} devices ${verb}`, {
          description: `${failed} request${failed === 1 ? "" : "s"} failed; check the list and retry.`,
        });
      setChecked(new Set());
      await invalidate();
    },
  });

  const listedDevices = useMemo(
    () => (states.length === 0 ? [] : (devices.data ?? [])),
    [states.length, devices.data],
  );
  const query = search.trim().toLowerCase();
  const visibleDevices = listedDevices.filter((device) => {
    const binding = device.convergence.binding.target;
    return (
      connectionFilter[device.convergence.connection_state] &&
      bindingFilter[binding?.state ?? "unknown"] &&
      (!attentionOnly || needsAttention(device)) &&
      (!query ||
        [seatOf(device), device.device_id, device.machine_hardware_id].some(
          (value) => value?.toLowerCase().includes(query),
        ))
    );
  });
  const stats = useMemo(() => {
    const count = (predicate: (device: Device) => boolean) =>
      listedDevices.filter(predicate).length;
    return {
      total: listedDevices.length,
      active: count((d) => d.convergence.connection_state === "active"),
      awaiting: count(
        (d) => d.convergence.connection_state === "awaiting_fresh_state",
      ),
      offline: count((d) => d.convergence.connection_state === "offline"),
      attention: count(needsAttention),
      unbound: count((d) => d.convergence.binding.target?.state !== "bound"),
    };
  }, [listedDevices]);
  const hasActiveFilters =
    search !== "" ||
    attentionOnly ||
    connectionStates.some((state) => !connectionFilter[state]) ||
    bindingOptions.some(({ value }) => !bindingFilter[value]) ||
    stateFilter.enabled !== defaultDeviceStateFilter.enabled ||
    stateFilter.disabled !== defaultDeviceStateFilter.disabled ||
    stateFilter.revoked !== defaultDeviceStateFilter.revoked;
  function resetFilters() {
    setSearch("");
    setAttentionOnly(false);
    setStateFilter(defaultDeviceStateFilter);
    setConnectionFilter(defaultConnectionFilter);
    setBindingFilter(defaultBindingFilter);
  }
  function toggleConnection(state: Convergence["connection_state"]) {
    const isOnly = connectionStates.every(
      (key) => connectionFilter[key] === (key === state),
    );
    setConnectionFilter(
      isOnly ? defaultConnectionFilter : only(connectionStates, state),
    );
  }
  const unboundOnly =
    !bindingFilter.bound && bindingFilter.unbound && bindingFilter.unknown;

  const isAdmin = session?.role === "admin";
  const handlers: DeviceActionHandlers = {
    isUnbinding: unbind.isPending,
    unbindDevice: unbind.mutate,
    isUpdatingLifecycle: lifecycle.isPending,
    updateLifecycle: lifecycle.mutate,
  };
  const { isPending: isUpdatingLifecycle, mutate: updateLifecycle } = lifecycle;
  const { isPending: isUnbinding, mutate: unbindDevice } = unbind;
  const selectedDevice =
    listedDevices.find((device) => device.device_id === selectedDeviceId) ??
    null;
  if (
    selectedDeviceId !== null &&
    (states.length === 0 || (devices.data && selectedDevice === null))
  ) {
    setSelectedDeviceId(null);
  }
  const checkedDevices = listedDevices.filter((device) =>
    checked.has(device.device_id),
  );
  const checkedBound = checkedDevices.filter((device) => seatOf(device));

  const columns = useMemo(() => {
    const columns: DataTableColumnDef<Device>[] = [
      {
        id: "seat",
        header: "Seat",
        accessorFn: seatOf,
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
            <div className="flex items-center gap-2.5">
              <StatusDot
                tone={muted ? "muted" : connection.key}
                pulse={!muted && connection.key === "success"}
                label={`Connection: ${connection.label}`}
              />
              <div className="min-w-0 leading-tight">
                <code
                  className="block font-mono text-xs"
                  title={device.machine_hardware_id}
                >
                  {device.machine_hardware_id.length > 14
                    ? `${device.machine_hardware_id.slice(0, 12)}…`
                    : device.machine_hardware_id}
                </code>
                <span className="text-xs text-muted-foreground">
                  {connection.label}
                </span>
              </div>
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
        header: "Lifecycle",
        cell: ({ row }) => {
          const badge = lifecycleBadges[row.original.state];
          return (
            <Badge variant={badge.variant} dot>
              {badge.label}
            </Badge>
          );
        },
      },
      {
        id: "binding",
        header: "Binding",
        cell: ({ row }) => {
          const binding = row.original.convergence.binding;
          const state = binding.target?.state;
          const tone = convergenceIcons[binding.status].tone;
          return (
            <span className="flex items-center gap-1.5 text-sm">
              <StatusIcon
                icon={
                  state === "bound"
                    ? "linked"
                    : state === "unbound"
                      ? "unlinked"
                      : "unknown"
                }
                tone={state === "bound" ? tone : "text-muted-foreground"}
                label={`Binding: ${state ?? "unknown"}, ${binding.status.replaceAll("_", " ")}`}
                className="size-4"
              />
              <span
                className={
                  state === "bound" ? undefined : "text-muted-foreground"
                }
              >
                {state === "bound"
                  ? binding.target?.state === "bound" &&
                    binding.target.context.domjudge_username
                  : (state ?? "unknown")}
              </span>
            </span>
          );
        },
      },
      {
        accessorKey: "evidence_quality",
        header: "Evidence",
        cell: ({ row }) => (
          <StatusIcon
            icon={
              row.original.evidence_quality === "strong"
                ? "shieldCheck"
                : "shield"
            }
            tone={
              row.original.evidence_quality === "strong"
                ? tones.success.text
                : tones.warning.text
            }
            label={`Evidence: ${row.original.evidence_quality}`}
          />
        ),
      },
      {
        id: "convergence",
        header: "Convergence",
        cell: ({ row }) => (
          <ConvergenceStrip convergence={row.original.convergence} />
        ),
      },
      {
        id: "seen",
        header: "Last state",
        cell: ({ row }) => (
          <span className="text-xs text-muted-foreground tabular-nums">
            {relativeTime(row.original.convergence.received_at_unix_ms)}
          </span>
        ),
      },
    ];

    if (isAdmin) {
      columns.push({
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        cell: ({ row }) => (
          <div className="flex justify-end">
            <DeviceActions
              device={row.original}
              isUnbinding={isUnbinding}
              unbindDevice={unbindDevice}
              isUpdatingLifecycle={isUpdatingLifecycle}
              updateLifecycle={updateLifecycle}
            />
          </div>
        ),
      });
    }
    return columns;
  }, [
    isAdmin,
    isUpdatingLifecycle,
    updateLifecycle,
    isUnbinding,
    unbindDevice,
  ]);

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Devices"
        description="Live connection, seat binding, and convergence for every contest machine."
        actions={
          <>
            <RefreshCountdown
              updatedAt={Math.max(
                devices.dataUpdatedAt,
                devices.errorUpdatedAt,
              )}
              isFetching={devices.isFetching}
              isPaused={devices.isPaused}
              isError={devices.isError}
              onRefresh={() => void devices.refetch()}
            />
            <DeviceEmergencyPanel />
          </>
        }
      />

      <StatGrid>
        <StatTile
          label="Listed"
          value={stats.total}
          icon={<Monitor aria-hidden="true" />}
          hint="ALL"
        />
        <StatTile
          label="Online"
          value={stats.active}
          tone="success"
          icon={<Wifi aria-hidden="true" />}
          active={
            connectionFilter.active &&
            !connectionFilter.offline &&
            !connectionFilter.awaiting_fresh_state
          }
          onClick={() => toggleConnection("active")}
        />
        <StatTile
          label="Awaiting state"
          value={stats.awaiting}
          tone="warning"
          icon={<Clock3 aria-hidden="true" />}
          active={
            connectionFilter.awaiting_fresh_state &&
            !connectionFilter.active &&
            !connectionFilter.offline
          }
          onClick={() => toggleConnection("awaiting_fresh_state")}
        />
        <StatTile
          label="Offline"
          value={stats.offline}
          tone="danger"
          icon={<WifiOff aria-hidden="true" />}
          active={
            connectionFilter.offline &&
            !connectionFilter.active &&
            !connectionFilter.awaiting_fresh_state
          }
          onClick={() => toggleConnection("offline")}
        />
        <StatTile
          label="Drifted or failed"
          value={stats.attention}
          tone={stats.attention ? "danger" : "muted"}
          icon={<TriangleAlert aria-hidden="true" />}
          active={attentionOnly}
          onClick={() => setAttentionOnly((value) => !value)}
        />
        <StatTile
          label="Without seat"
          value={stats.unbound}
          tone={stats.unbound ? "info" : "muted"}
          icon={<Unlink aria-hidden="true" />}
          active={unboundOnly}
          onClick={() =>
            setBindingFilter(
              unboundOnly
                ? defaultBindingFilter
                : { bound: false, unbound: true, unknown: true },
            )
          }
        />
      </StatGrid>

      <section aria-label="Device list" className="space-y-3">
        <div
          role="search"
          aria-label="Device filters"
          className="flex flex-wrap items-center gap-2"
        >
          <InputGroup
            type="search"
            aria-label="Search devices"
            placeholder="Seat, device ID, or hardware ID"
            icon={<Search aria-hidden="true" />}
            className="w-full sm:w-72"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
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
          {hasActiveFilters && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={resetFilters}
            >
              <RotateCcw aria-hidden="true" />
              Reset
            </Button>
          )}
          {(states.length === 0 || devices.data) && (
            <p
              role="status"
              className="ml-auto text-xs text-muted-foreground tabular-nums"
            >
              {visibleDevices.length} of {listedDevices.length} devices
            </p>
          )}
        </div>

        <DataState
          isLoading={devices.isLoading}
          error={devices.data ? null : devices.error}
          isEmpty={!visibleDevices.length}
          emptyLabel="No devices match"
          emptyDescription={
            hasActiveFilters
              ? "Try widening the filters or clearing the search."
              : "Devices appear here after they enroll."
          }
          emptyAction={
            hasActiveFilters ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={resetFilters}
              >
                <RotateCcw aria-hidden="true" />
                Reset filters
              </Button>
            ) : undefined
          }
        >
          <DataTable
            label="Devices"
            virtualized
            columns={columns}
            data={visibleDevices}
            getRowId={(device) => device.device_id}
            onRowClick={(device) => setSelectedDeviceId(device.device_id)}
            activeRowId={selectedDeviceId}
            selection={
              isAdmin
                ? {
                    selected: checked,
                    onChange: setChecked,
                    isSelectable: (id) =>
                      listedDevices.find((device) => device.device_id === id)
                        ?.state !== "revoked",
                  }
                : undefined
            }
            rowClassName={(device) =>
              device.state !== "enabled"
                ? "text-muted-foreground"
                : connections[device.convergence.connection_state].row
            }
          />
          <Legend />
        </DataState>
      </section>

      <Sheet
        open={selectedDevice !== null}
        onOpenChange={(open) => !open && setSelectedDeviceId(null)}
      >
        <SheetContent>
          {selectedDevice && (
            <>
              <SheetHeader>
                <div className="flex items-center gap-2">
                  <SheetTitle className="font-mono">
                    {seatOf(selectedDevice) ?? "Unbound device"}
                  </SheetTitle>
                  <Badge
                    variant={lifecycleBadges[selectedDevice.state].variant}
                    dot
                  >
                    {lifecycleBadges[selectedDevice.state].label}
                  </Badge>
                  <Badge
                    variant={
                      tones[
                        connections[selectedDevice.convergence.connection_state]
                          .key
                      ].badge
                    }
                  >
                    {
                      connections[selectedDevice.convergence.connection_state]
                        .label
                    }
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
                <DeviceConvergence device={selectedDevice} />
              </SheetBody>
              {isAdmin && (
                <SheetFooter>
                  <DeviceActions
                    device={selectedDevice}
                    layout="buttons"
                    {...handlers}
                  />
                </SheetFooter>
              )}
            </>
          )}
        </SheetContent>
      </Sheet>

      {isAdmin && (
        <>
          <BulkActionBar
            count={checkedDevices.length}
            onClear={() => setChecked(new Set())}
          >
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={bulk.isPending}
              onClick={() =>
                bulk.mutate({
                  action: "enabled",
                  ids: checkedDevices.map((device) => device.device_id),
                })
              }
            >
              <CirclePlay aria-hidden="true" />
              Enable
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={bulk.isPending}
              onClick={() =>
                bulk.mutate({
                  action: "disabled",
                  ids: checkedDevices.map((device) => device.device_id),
                })
              }
            >
              <CirclePause aria-hidden="true" />
              Disable
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={bulk.isPending || checkedBound.length === 0}
              onClick={() => setBulkConfirm("unbind")}
            >
              <Unlink aria-hidden="true" />
              Unbind
            </Button>
            <Button
              type="button"
              variant="destructive-soft"
              size="sm"
              disabled={bulk.isPending}
              onClick={() => setBulkConfirm("revoke")}
            >
              <Ban aria-hidden="true" />
              Revoke
            </Button>
          </BulkActionBar>
          <ConfirmDialog
            open={bulkConfirm === "unbind"}
            onOpenChange={(open) => !open && setBulkConfirm(null)}
            tone="warning"
            icon={<Unlink aria-hidden="true" />}
            title={`Unbind ${checkedBound.length} seats?`}
            description="These devices return to seat selection when they are enabled, connected, and ready for binding. Selected devices without a seat are skipped."
            confirmLabel="Unbind seats"
            onConfirm={() =>
              bulk.mutate({
                action: "unbind",
                ids: checkedBound.map((device) => device.device_id),
              })
            }
          />
          <ConfirmDialog
            open={bulkConfirm === "revoke"}
            onOpenChange={(open) => !open && setBulkConfirm(null)}
            icon={<Ban aria-hidden="true" />}
            title={`Permanently revoke ${checkedDevices.length} devices?`}
            description="Revocation is terminal and immediately evicts each device's connection. They must re-enroll to be used again."
            confirmLabel="Revoke devices"
            onConfirm={() =>
              bulk.mutate({
                action: "revoked",
                ids: checkedDevices.map((device) => device.device_id),
              })
            }
          />
        </>
      )}
    </div>
  );
}

function Legend() {
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 px-1 text-xs text-muted-foreground">
      <span className="font-medium">Convergence segments</span>
      <span className="font-mono">
        {convergenceParts.map(([, name]) => name).join(" · ")}
      </span>
      <span aria-hidden="true" className="h-3 w-px bg-border" />
      {(
        Object.entries(convergenceIcons) as [
          keyof typeof convergenceIcons,
          (typeof convergenceIcons)[keyof typeof convergenceIcons],
        ][]
      ).map(([status, style]) => (
        <span key={status} className="flex items-center gap-1.5">
          <span className={`h-3 w-1.5 rounded-sm ${tones[style.key].dot}`} />
          {status.replaceAll("_", " ")}
        </span>
      ))}
    </div>
  );
}

function relativeTime(value: number | null) {
  if (value === null) return "never";
  const seconds = Math.max(0, Math.round((Date.now() - value) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours}h ago`;
  return new Date(value).toLocaleDateString();
}
