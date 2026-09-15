import { useState } from "react";

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
import { Input } from "@/components/ui/input";

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
    <section
      aria-label="All device actions"
      className="space-y-3 rounded-md border p-4"
    >
      <div>
        <h2 className="font-medium">All enabled devices ({enabled.length})</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Includes offline devices; targets apply after reconnection. Disabled
          and revoked devices are skipped. List filters do not change this
          scope.
        </p>
      </div>
      <div className="flex flex-wrap gap-2">
        {(
          ["waiting", "contest", "terminate", "reset"] as TargetOperation[]
        ).map((operation) => (
          <Button
            key={operation}
            type="button"
            size="sm"
            variant={
              operation === "reset" || operation === "terminate"
                ? "destructive"
                : "outline"
            }
            disabled={disabled || enabled.length === 0}
            onClick={() => setConfirmation(operation)}
          >
            {targetOperations[operation]} (all)
          </Button>
        ))}
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
      <PowerOffAction
        devices={enabled}
        disabled={disabled}
        onSubmit={() => onSubmit("poweroff")}
      />
    </section>
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
  const online = devices.filter(
    (device) => device.convergence.connection_state === "active",
  );
  const phrase = `POWER OFF ${online.length}`;
  return (
    <div className="border-t pt-3">
      <Button
        type="button"
        size="sm"
        variant="destructive"
        disabled={disabled || online.length === 0}
        onClick={() => setOpen(true)}
      >
        Power off online devices ({online.length})
      </Button>
      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Power off online devices?</AlertDialogTitle>
            <AlertDialogDescription>
              The Server will select online enabled devices when it processes
              this request. The request expires after 60 seconds; offline
              devices will not power off later. Enter <code>{phrase}</code> to
              confirm. A submitted request cannot prove physical power-off.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <Input
            value={confirmation}
            onChange={(event) => setConfirmation(event.target.value)}
            aria-label="Power-off confirmation"
          />
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={confirmation !== phrase || disabled}
              className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => {
                onSubmit();
                setConfirmation("");
              }}
            >
              Power off {online.length} devices
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
