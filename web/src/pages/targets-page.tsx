import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "@/api/client";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
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
import { Label } from "@/components/ui/label";

type Device = components["schemas"]["DeviceResponse"];
type SessionControl = components["schemas"]["SessionControlResponse"];
type SessionLock = components["schemas"]["SessionLockRequest"]["lock_state"];
type Home = components["schemas"]["HomeResponse"];

const DEVICES_KEY = ["devices"] as const;

export function TargetsPage() {
  const session = useSession().data;
  const [deviceId, setDeviceId] = useState("");
  const devices = useQuery({
    queryKey: DEVICES_KEY,
    queryFn: async () => unwrap<Device[]>(await api.GET("/api/v2/devices")),
    refetchInterval: LIST_POLL_MS,
  });
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

          {deviceId && (
            <DeviceTargets
              deviceId={deviceId}
              isAdmin={session?.role === "admin"}
            />
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function DeviceTargets({
  deviceId,
  isAdmin,
}: {
  deviceId: string;
  isAdmin: boolean;
}) {
  const queryClient = useQueryClient();
  const sessionControl = useQuery({
    queryKey: ["device-session-control", deviceId],
    queryFn: async () =>
      unwrap<SessionControl>(
        await api.GET("/api/v2/devices/{device_id}/session-control", {
          params: { path: { device_id: deviceId } },
        }),
      ),
  });
  const home = useQuery({
    queryKey: ["device-home", deviceId],
    queryFn: async () =>
      unwrap<Home>(
        await api.GET("/api/v2/devices/{device_id}/home", {
          params: { path: { device_id: deviceId } },
        }),
      ),
  });
  const setLock = useMutation({
    mutationFn: async (lockState: SessionLock) =>
      unwrap<SessionControl>(
        await api.PUT("/api/v2/devices/{device_id}/session-control", {
          params: { path: { device_id: deviceId } },
          body: { lock_state: lockState },
        }),
      ),
    onSuccess: async (target) => {
      queryClient.setQueryData(["device-session-control", deviceId], target);
      await queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
    },
  });
  const terminate = useMutation({
    mutationFn: async () =>
      unwrap<SessionControl>(
        await api.POST(
          "/api/v2/devices/{device_id}/session-control/actions/terminate",
          { params: { path: { device_id: deviceId } } },
        ),
      ),
    onSuccess: async (target) => {
      queryClient.setQueryData(["device-session-control", deviceId], target);
      await queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
    },
  });
  const reset = useMutation({
    mutationFn: async () =>
      unwrap<Home>(
        await api.POST("/api/v2/devices/{device_id}/home/actions/reset", {
          params: { path: { device_id: deviceId } },
        }),
      ),
    onSuccess: async (target) => {
      queryClient.setQueryData(["device-home", deviceId], target);
      await queryClient.invalidateQueries({ queryKey: DEVICES_KEY });
    },
  });

  const mutationError = setLock.error ?? terminate.error ?? reset.error;
  const isMutating =
    setLock.isPending || terminate.isPending || reset.isPending;
  return (
    <div className="space-y-4">
      {mutationError && (
        <MutationError error={mutationError} fallback="Target update failed" />
      )}
      <div className="grid gap-4 md:grid-cols-2">
        <div className="space-y-3 rounded-md border p-4">
          <h3 className="font-medium">Session Control</h3>
          <DataState
            isLoading={sessionControl.isLoading}
            error={sessionControl.data ? null : sessionControl.error}
            isEmpty={false}
            emptyLabel=""
          >
            {sessionControl.data && (
              <div className="space-y-3">
                <Badge variant="outline">
                  {sessionControl.data.target?.lock_state ?? "unlocked"}
                </Badge>
                <p className="text-sm text-muted-foreground">
                  Terminate epoch:{" "}
                  {sessionControl.data.target?.terminate_epoch ?? "none"}
                </p>
                {isAdmin && (
                  <div className="flex gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={isMutating}
                      onClick={() =>
                        setLock.mutate(
                          sessionControl.data.target?.lock_state === "locked"
                            ? "unlocked"
                            : "locked",
                        )
                      }
                    >
                      {sessionControl.data.target?.lock_state === "locked"
                        ? "Unlock"
                        : "Lock"}
                    </Button>
                    <TargetAction
                      label="Terminate"
                      title="Terminate the current session?"
                      description="This advances the durable terminate epoch for this device."
                      disabled={isMutating}
                      onConfirm={() => terminate.mutate()}
                    />
                  </div>
                )}
              </div>
            )}
          </DataState>
        </div>

        <div className="space-y-3 rounded-md border p-4">
          <h3 className="font-medium">Home</h3>
          <DataState
            isLoading={home.isLoading}
            error={home.data ? null : home.error}
            isEmpty={false}
            emptyLabel=""
          >
            {home.data && (
              <div className="space-y-3">
                <p className="text-sm text-muted-foreground">
                  Reset epoch: {home.data.reset_epoch ?? "none"}
                </p>
                {isAdmin && (
                  <TargetAction
                    label="Reset home"
                    title="Reset this device home?"
                    description="This advances the durable Home reset epoch."
                    disabled={isMutating}
                    onConfirm={() => reset.mutate()}
                  />
                )}
              </div>
            )}
          </DataState>
        </div>
      </div>
    </div>
  );
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
