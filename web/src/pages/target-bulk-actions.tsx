import { type ReactNode, useId, useState } from "react";
import {
  FolderSync,
  Monitor,
  MonitorPlay,
  Power,
  RotateCcw,
} from "lucide-react";

import type { DeviceResponse_Serialize as DeviceResponse } from "@/api/generated/types";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

import { targetOperations, type TargetOperation } from "./target-operations";

type Device = DeviceResponse;

export function BulkTargetActions({
  devices,
  disabled,
  onSubmit,
}: {
  devices: Device[];
  disabled: boolean;
  onSubmit: (operation: TargetOperation) => void;
}) {
  const [confirmation, setConfirmation] = useState<TargetOperation | null>(
    null,
  );
  const enabled = devices.filter((device) => device.state === "enabled");
  const offline = enabled.filter(
    (device) => device.convergence.connection_state === "offline",
  ).length;
  return (
    <Card
      role="region"
      aria-label="All device actions"
      className="gap-0 overflow-hidden py-0"
    >
      <div className="grid lg:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="space-y-4 p-5">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="space-y-1">
              <p className="font-semibold">Fleet actions</p>
              <p className="max-w-xl text-sm text-muted-foreground">
                Apply to every enabled device, including offline ones once they
                reconnect. List filters and selection do not change this scope.
              </p>
            </div>
            <div className="flex gap-2">
              <Badge variant="secondary">{enabled.length} enabled</Badge>
              {offline > 0 && (
                <Badge variant="warning">{offline} offline</Badge>
              )}
            </div>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <ActionGroup label="Foreground">
              {(
                [
                  ["waiting", Monitor],
                  ["contest", MonitorPlay],
                ] as const
              ).map(([operation, Icon]) => (
                <Button
                  key={operation}
                  type="button"
                  size="sm"
                  variant="outline"
                  className="flex-1"
                  disabled={disabled || enabled.length === 0}
                  onClick={() => setConfirmation(operation)}
                >
                  <Icon aria-hidden="true" />
                  {operation === "waiting"
                    ? "Waiting screen"
                    : "Contest desktop"}
                </Button>
              ))}
            </ActionGroup>
            <ActionGroup label="Restart">
              {(
                [
                  ["terminate", RotateCcw],
                  ["reset", FolderSync],
                ] as const
              ).map(([operation, Icon]) => (
                <Button
                  key={operation}
                  type="button"
                  size="sm"
                  variant="destructive-outline"
                  className="flex-1"
                  disabled={disabled || enabled.length === 0}
                  onClick={() => setConfirmation(operation)}
                >
                  <Icon aria-hidden="true" />
                  {targetOperations[operation]}
                </Button>
              ))}
            </ActionGroup>
          </div>
        </div>
        <PowerOffAction
          devices={enabled}
          disabled={disabled}
          onSubmit={() => onSubmit("poweroff")}
        />
      </div>
      <AlertDialog
        open={confirmation !== null}
        onOpenChange={(open) => {
          if (!open) setConfirmation(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirmation &&
                `${targetOperations[confirmation]} on all enabled devices?`}
            </AlertDialogTitle>
            <AlertDialogDescription>
              Estimated scope: {enabled.length} devices, including{" "}
              {
                enabled.filter(
                  (device) => device.convergence.connection_state === "offline",
                ).length
              }{" "}
              offline devices. Disabled and revoked devices are excluded. All
              devices enabled when the Server processes this submission will be
              included. The result will show the actual device list.
              {confirmation === "reset" &&
                " This deletes the contest user's Home data and restores the default Home. Contest sessions will restart."}
              {confirmation === "terminate" &&
                " Contest sessions will end and restart. Home files are preserved."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={disabled}
              variant={
                confirmation === "reset" || confirmation === "terminate"
                  ? "destructive"
                  : "default"
              }
              onClick={() => {
                if (confirmation) onSubmit(confirmation);
              }}
            >
              Apply to all enabled devices
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}

function PowerOffAction({
  devices,
  disabled,
  onSubmit,
}: {
  devices: Device[];
  disabled: boolean;
  onSubmit: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [confirmation, setConfirmation] = useState("");
  const confirmationId = useId();
  const online = devices.filter(
    (device) => device.convergence.connection_state === "active",
  );
  const phrase = `POWER OFF ${online.length}`;
  const deviceLabel = online.length === 1 ? "device" : "devices";
  return (
    <div className="flex flex-col items-start gap-3 border-t bg-muted/40 p-5 lg:border-t-0 lg:border-l">
      <div className="space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <p className="flex items-center gap-2 font-semibold">
            <Power aria-hidden="true" className="size-4 text-destructive" />
            Power control
          </p>
          <Badge variant="outline">Online only</Badge>
        </div>
        <p className="text-sm text-muted-foreground">
          Shut down online enabled devices. Requests expire after 60 seconds.
        </p>
      </div>
      <Button
        type="button"
        size="sm"
        variant="destructive"
        className="mt-auto w-full"
        disabled={disabled || online.length === 0}
        onClick={() => setOpen(true)}
      >
        <Power aria-hidden="true" />
        Power off online devices ({online.length})
      </Button>
      <AlertDialog
        open={open}
        onOpenChange={(open) => {
          setOpen(open);
          if (!open) setConfirmation("");
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Power off online devices?</AlertDialogTitle>
            <AlertDialogDescription>
              Sessions will end and unsaved work may be lost. Only online
              enabled devices are included.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="space-y-3 rounded-lg border bg-muted/40 p-4 text-sm">
            <dl className="space-y-2">
              <div className="flex items-center justify-between gap-4">
                <dt className="text-muted-foreground">Estimated scope</dt>
                <dd className="font-medium">
                  {online.length} online {deviceLabel}
                </dd>
              </div>
              <div className="flex items-center justify-between gap-4">
                <dt className="text-muted-foreground">Request expires in</dt>
                <dd className="font-medium">60 seconds</dd>
              </div>
            </dl>
            <p className="border-t pt-3 text-muted-foreground">
              The online device list is checked again at submission. Offline
              devices will not shut down later. Check device status after
              submitting to confirm shutdown.
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor={confirmationId}>Power-off confirmation</Label>
            <p
              id={`${confirmationId}-hint`}
              className="text-sm text-muted-foreground"
            >
              Type{" "}
              <code className="bg-muted px-1.5 py-0.5 font-medium text-foreground">
                {phrase}
              </code>{" "}
              to confirm.
            </p>
            <Input
              id={confirmationId}
              aria-describedby={`${confirmationId}-hint`}
              value={confirmation}
              onChange={(event) => setConfirmation(event.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={
                confirmation !== phrase || disabled || online.length === 0
              }
              variant="destructive"
              onClick={() => {
                onSubmit();
                setConfirmation("");
              }}
            >
              <Power aria-hidden="true" />
              Power off {online.length} {deviceLabel}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function ActionGroup({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div role="group" aria-label={label} className="space-y-1.5">
      <p className="text-xs font-medium text-muted-foreground">{label}</p>
      <div className="flex flex-wrap gap-2">{children}</div>
    </div>
  );
}
