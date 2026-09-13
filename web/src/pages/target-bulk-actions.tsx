import { useState, type Dispatch, type SetStateAction } from "react";
import {
  useIsMutating,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";

import { ApiError } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { useSessionScope } from "@/auth/session-context";
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

import {
  submitTarget,
  targetOperations,
  type TargetOperation,
  type TargetSubmission,
} from "./target-operations";

type Device = components["schemas"]["DeviceResponse"];
type Batch = { operation: TargetOperation; devices: Device[]; skipped: number };

export function BulkTargetActions({
  devices,
  results,
  setResults,
}: {
  devices: Device[];
  results: TargetSubmission[];
  setResults: Dispatch<SetStateAction<TargetSubmission[]>>;
}) {
  const { api } = useSessionScope();
  const queryClient = useQueryClient();
  const [confirmation, setConfirmation] = useState<Batch | null>(null);
  const singlePending = useIsMutating({ mutationKey: ["device-target"] }) > 0;
  const bulkPending = useIsMutating({ mutationKey: ["bulk-target"] }) > 0;
  const batch = useMutation({
    mutationKey: ["bulk-target"],
    mutationFn: async ({ operation, devices }: Batch) => {
      setResults([]);
      let next = 0;
      // Bound request concurrency for fleets of hundreds of devices. Each
      // device is submitted once; reset/terminate must not be retried blindly.
      await Promise.all(
        Array.from({ length: Math.min(6, devices.length) }, async () => {
          while (next < devices.length) {
            const device = devices[next++];
            let error: string | null = null;
            try {
              await submitTarget(api, device.device_id, operation);
            } catch (cause) {
              error =
                cause instanceof ApiError
                  ? cause.title
                  : "Submission not confirmed";
            }
            setResults((current) => [
              ...current,
              { deviceId: device.device_id, error },
            ]);
          }
        }),
      );
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["devices"] });
    },
  });
  const enabled = devices.filter((device) => device.state === "enabled");
  const disabled = singlePending || bulkPending || enabled.length === 0;
  const accepted = results.filter((result) => result.error === null).length;

  return (
    <section
      aria-label="All device actions"
      className="space-y-3 rounded-md border p-4"
    >
      <div>
        <h2 className="font-medium">All enabled devices ({enabled.length})</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Includes offline devices; targets apply after reconnection. Disabled
          and revoked devices are skipped. Search does not change this scope.
        </p>
      </div>
      <div className="flex flex-wrap gap-2">
        {(Object.keys(targetOperations) as TargetOperation[]).map(
          (operation) => (
            <Button
              key={operation}
              type="button"
              size="sm"
              variant={
                operation === "reset" || operation === "terminate"
                  ? "destructive"
                  : "outline"
              }
              disabled={disabled}
              onClick={() =>
                setConfirmation({
                  operation,
                  devices: enabled,
                  skipped: devices.length - enabled.length,
                })
              }
            >
              {targetOperations[operation]} (all)
            </Button>
          ),
        )}
      </div>
      {batch.variables && (
        <div role="status" className="space-y-1 text-sm">
          <p>
            {targetOperations[batch.variables.operation]}: {accepted} submitted,{" "}
            {results.length - accepted} not confirmed,{" "}
            {batch.variables.devices.length - results.length} remaining.
          </p>
          <p className="text-muted-foreground">
            {batch.isPending
              ? "Submitting targets and refreshing status. "
              : "Submission finished. "}
            Check each device's Session and Home convergence for completion.
            {results.length > accepted &&
              " Review devices marked Not confirmed before submitting another action."}
          </p>
        </div>
      )}
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
                `${targetOperations[confirmation.operation]} on ${confirmation.devices.length} devices?`}
            </AlertDialogTitle>
            <AlertDialogDescription>
              This applies to the {confirmation?.devices.length} enabled devices
              selected when this dialog opened, including{" "}
              {
                confirmation?.devices.filter(
                  (device) => device.convergence.connection_state === "offline",
                ).length
              }{" "}
              offline devices. {confirmation?.skipped} disabled or revoked
              devices are skipped.
              {confirmation?.operation === "reset" &&
                " This deletes the contest user's Home data and restores the default Home. Contest sessions will restart."}
              {confirmation?.operation === "terminate" &&
                " Contest sessions will end and restart. Home files are preserved."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={singlePending || bulkPending}
              className={
                confirmation?.operation === "reset" ||
                confirmation?.operation === "terminate"
                  ? "bg-destructive text-white hover:bg-destructive/90"
                  : undefined
              }
              onClick={() => {
                if (confirmation) batch.mutate(confirmation);
              }}
            >
              Apply to {confirmation?.devices.length} devices
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
