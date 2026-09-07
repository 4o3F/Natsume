import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useSessionScope } from "@/auth/session-context";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DataState } from "@/components/data-state";
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
import { Label } from "@/components/ui/label";

type Device = components["schemas"]["DeviceResponse"];
type SessionControl = components["schemas"]["SessionControlResponse"];
type SessionForeground =
  components["schemas"]["SessionForegroundRequest"]["foreground_target"];
type Home = components["schemas"]["HomeResponse"];
type Convergence = components["schemas"]["DeviceConvergenceResponse"];
type TargetOperation = SessionForeground | "terminate" | "reset";

const DEVICES_KEY = ["devices"] as const;

export function TargetsPage() {
  const { api } = useSessionScope();
  const session = useSession().data;
  const [deviceId, setDeviceId] = useState("");
  const devices = useQuery({
    queryKey: DEVICES_KEY,
    queryFn: async ({ signal }) =>
      unwrap<Device[]>(await api.GET("/api/v2/devices", { signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const selectedDevice = devices.data?.find(
    (device) => device.device_id === deviceId,
  );
  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Targets</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Update the current Session Control and Home targets.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Device targets</CardTitle>
          <CardDescription>
            Select one device before changing its Session Control or Home
            target.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
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
          <DataState
            isLoading={devices.isLoading}
            error={devices.data ? null : devices.error}
            isEmpty={!devices.data?.length}
            emptyLabel="No devices found."
          >
            <div className="space-y-2">
              <Label htmlFor="target-device">Device</Label>
              <select
                id="target-device"
                className="h-9 w-full rounded-md border bg-transparent px-3 text-sm shadow-xs"
                value={deviceId}
                onChange={(event) => setDeviceId(event.target.value)}
              >
                <option value="">Select a device</option>
                {devices.data?.map((device) => (
                  <option key={device.device_id} value={device.device_id}>
                    {device.machine_hardware_id} ({device.state})
                  </option>
                ))}
              </select>
            </div>
          </DataState>

          {devices.data && (
            <div className="text-sm text-muted-foreground">
              <p>
                Last successful refresh:{" "}
                {formatTimestamp(devices.dataUpdatedAt)}
              </p>
              {devices.isFetching && (
                <p role="status">Refreshing. Showing previous result.</p>
              )}
            </div>
          )}
          {selectedDevice && (
            <DeviceTargets
              key={deviceId}
              device={selectedDevice}
              isAdmin={session?.role === "admin"}
              previous={devices.isFetching || devices.isError}
            />
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function DeviceTargets({
  device,
  isAdmin,
  previous,
}: {
  device: Device;
  isAdmin: boolean;
  previous: boolean;
}) {
  const { api } = useSessionScope();
  const queryClient = useQueryClient();
  const { session_control: session, home } = device.convergence;
  const updateTarget = useMutation({
    mutationFn: async (operation: TargetOperation) => {
      const params = { path: { device_id: device.device_id } };
      switch (operation) {
        case "waiting":
        case "contest":
          return unwrap<SessionControl>(
            await api.PUT("/api/v2/devices/{device_id}/session-control", {
              params,
              body: { foreground_target: operation },
            }),
          );
        case "terminate":
          return unwrap<SessionControl>(
            await api.POST(
              "/api/v2/devices/{device_id}/session-control/actions/terminate",
              { params },
            ),
          );
        case "reset":
          return unwrap<Home>(
            await api.POST("/api/v2/devices/{device_id}/home/actions/reset", {
              params,
            }),
          );
      }
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
    },
  });

  const showPrevious = previous || updateTarget.isPending;
  return (
    <div className="space-y-4">
      {updateTarget.error && (
        <MutationError
          error={updateTarget.error}
          fallback="Target update failed"
        />
      )}
      {updateTarget.isPending && (
        <p role="status" className="text-sm text-muted-foreground">
          Submitting target and refreshing status. Showing previous result.
        </p>
      )}
      {updateTarget.isSuccess && (
        <p role="status" className="text-sm">
          Target submitted. Check the convergence state below for device
          completion.
        </p>
      )}
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
                disabled={updateTarget.isPending}
                onClick={() => updateTarget.mutate("waiting")}
              >
                Show waiting screen
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={updateTarget.isPending}
                onClick={() => updateTarget.mutate("contest")}
              >
                Show contest desktop
              </Button>
              <TargetAction
                label="Terminate"
                title="Terminate the current session?"
                description="Request termination of this device's current session. Check its reported state to confirm completion."
                disabled={updateTarget.isPending}
                onConfirm={() => updateTarget.mutate("terminate")}
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
              title="Reset this device home?"
              description="Request a reset of this device's Home. Check its reported state to confirm completion."
              disabled={updateTarget.isPending}
              onConfirm={() => updateTarget.mutate("reset")}
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

function MutationError({
  error,
  fallback,
}: {
  error: unknown;
  fallback: string;
}) {
  return (
    <Alert variant="destructive">
      <AlertTitle>
        {error instanceof ApiError ? error.title : fallback}
      </AlertTitle>
    </Alert>
  );
}
