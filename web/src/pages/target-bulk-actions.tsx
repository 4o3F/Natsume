import { useId, useState } from "react";
import {
  FolderSync,
  Monitor,
  MonitorPlay,
  Power,
  RotateCcw,
} from "lucide-react";

import type { components } from "@/api/generated/schema";
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

type Device = components["schemas"]["DeviceResponse"];

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
  return (
    <Card
      role="region"
      aria-label="All device actions"
      className="gap-0 overflow-hidden py-0"
    >
      <div className="grid lg:grid-cols-3">
        <div className="space-y-4 p-5 lg:col-span-2">
          <div className="space-y-1">
            <h2 className="font-semibold">
              All enabled devices ({enabled.length})
            </h2>
            <p className="text-sm text-muted-foreground">
              Session and Home targets include offline devices and apply after
              reconnection. List filters do not change this scope.
            </p>
          </div>
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 xl:flex xl:flex-wrap">
            {(
              [
                ["waiting", Monitor],
                ["contest", MonitorPlay],
                ["terminate", RotateCcw],
                ["reset", FolderSync],
              ] as const
            ).map(([operation, Icon]) => (
              <Button
                key={operation}
                type="button"
                size="sm"
                variant="outline"
                className={
                  operation === "reset" || operation === "terminate"
                    ? "text-destructive hover:bg-destructive/5 hover:text-destructive"
                    : undefined
                }
                disabled={disabled || enabled.length === 0}
                onClick={() => setConfirmation(operation)}
              >
                <Icon aria-hidden="true" />
                {targetOperations[operation]} (all)
              </Button>
            ))}
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
              className={
                confirmation === "reset" || confirmation === "terminate"
                  ? "bg-destructive text-white hover:bg-destructive/90"
                  : undefined
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
    <div className="flex flex-col items-start gap-3 border-t bg-muted/30 p-5 lg:border-t-0 lg:border-l">
      <div className="space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="font-semibold">Power control</h3>
          <Badge variant="outline">Online only</Badge>
        </div>
        <p className="text-sm text-muted-foreground">
          Shut down online enabled devices. Requests expire after 60 seconds.
        </p>
      </div>
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="text-destructive hover:bg-destructive/5 hover:text-destructive"
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
          <div className="space-y-3 rounded-md border bg-muted/30 p-4 text-sm">
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
              <code className="rounded bg-muted px-1.5 py-0.5 font-medium text-foreground">
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
              className="bg-destructive text-white hover:bg-destructive/90"
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
