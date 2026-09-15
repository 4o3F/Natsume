import { useState, type ReactNode } from "react";
import {
  ChevronDown,
  CircleCheck,
  CircleX,
  Info,
  LoaderCircle,
  Monitor,
  RotateCcw,
  TriangleAlert,
} from "lucide-react";

import type { components } from "@/api/generated/schema";
import { DataTable } from "@/components/data-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

import { targetActionLabel, type TargetResponse } from "./target-operations";
import type { TargetSubmissionState } from "./target-submission-store";

type Device = components["schemas"]["DeviceResponse"];
type Result = TargetResponse["results"][number];

export function TargetSubmissionResult({
  submission,
  devices,
  onRetry,
  children,
}: {
  submission: TargetSubmissionState;
  devices: Device[];
  onRetry: () => void;
  children: ReactNode;
}) {
  const [onlyRejected, setOnlyRejected] = useState(false);
  const request = submission.pending ?? submission.completed?.request;
  const results = submission.completed?.response.results;
  const rejected = results?.filter((result) => result.status === "rejected");
  const rejectedCount = rejected?.length ?? 0;
  const submittedCount = (results?.length ?? 0) - rejectedCount;
  const byDevice = new Map(devices.map((device) => [device.device_id, device]));
  const Icon = submission.sending
    ? LoaderCircle
    : submission.pending
      ? TriangleAlert
      : submission.error || (rejectedCount > 0 && submittedCount === 0)
        ? CircleX
        : rejectedCount > 0
          ? TriangleAlert
          : submittedCount > 0
            ? CircleCheck
            : Info;
  const tone = submission.sending
    ? "bg-sky-500/10 text-sky-700"
    : submission.pending || (rejectedCount > 0 && submittedCount > 0)
      ? "bg-amber-500/10 text-amber-700"
      : submission.error || rejectedCount > 0
        ? "bg-destructive/10 text-destructive"
        : submittedCount > 0
          ? "bg-emerald-500/10 text-emerald-700"
          : "bg-muted text-muted-foreground";
  const status = submission.sending
    ? "Submitting"
    : submission.pending
      ? "Result unconfirmed"
      : submission.error
        ? "Submission failed"
        : !results?.length
          ? "No devices in scope"
          : rejectedCount === 0
            ? "Targets submitted"
            : submittedCount === 0
              ? "Submission rejected"
              : "Partially submitted";

  return (
    <Card
      role="region"
      aria-label="Target submission"
      className="gap-0 overflow-hidden py-0"
    >
      <div className="flex items-start gap-3 p-5">
        <div className={`rounded-lg p-2.5 ${tone}`}>
          <Icon
            aria-hidden="true"
            className={`size-5 ${submission.sending ? "animate-spin motion-reduce:animate-none" : ""}`}
          />
        </div>
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="font-semibold">
              {request
                ? targetActionLabel(request.action)
                : "Target submission"}
            </h2>
            <Badge variant="outline" className={`border-transparent ${tone}`}>
              {status}
            </Badge>
          </div>
          {request && (
            <p className="break-words text-sm text-muted-foreground">
              {request.scope.kind === "all_enabled"
                ? "All enabled devices"
                : request.scope.kind === "all_online_enabled"
                  ? "All online enabled devices"
                  : `Selected devices · ${
                      request.scope.device_ids.length === 1
                        ? deviceLabel(
                            byDevice.get(request.scope.device_ids[0]),
                            request.scope.device_ids[0],
                          )
                        : `${request.scope.device_ids.length} devices`
                    }`}
            </p>
          )}
        </div>
      </div>

      {results && (
        <>
          <dl className="mx-5 mb-4 grid grid-cols-3 divide-x rounded-md border bg-muted/20">
            {[
              {
                label: "Devices in scope",
                count: results.length,
                Icon: Monitor,
                tone: "text-foreground",
              },
              {
                label: "Submitted",
                count: submittedCount,
                Icon: CircleCheck,
                tone: "text-emerald-700",
              },
              {
                label: "Rejected",
                count: rejectedCount,
                Icon: CircleX,
                tone: rejectedCount
                  ? "text-destructive"
                  : "text-muted-foreground",
              },
            ].map(({ label, count, Icon, tone }) => (
              <div key={label} className="space-y-2 p-3 sm:px-4">
                <dt className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <Icon
                    aria-hidden="true"
                    className={`hidden size-3.5 sm:block ${tone}`}
                  />
                  {label}
                </dt>
                <dd className={`text-2xl font-semibold tabular-nums ${tone}`}>
                  {count}
                </dd>
              </div>
            ))}
          </dl>
          <div className="flex flex-wrap items-center justify-between gap-3 px-5 pb-4">
            <p className="flex items-start gap-2 text-xs text-muted-foreground">
              <Info aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
              {request?.action.kind === "power_off"
                ? "Server acceptance does not confirm shutdown. Check device status."
                : "Server acceptance does not confirm completion. Check device convergence."}
            </p>
            {children}
          </div>
          <p role="status" className="sr-only">
            {submittedCount} submitted, {rejectedCount} rejected.{" "}
            {request?.action.kind === "power_off"
              ? "Check devices to confirm shutdown."
              : "Check device convergence for completion."}
          </p>
          {results.length === 0 ? (
            <p className="border-t px-5 py-4 text-sm text-muted-foreground">
              No eligible devices were included when the Server processed this
              request.
            </p>
          ) : (
            <details className="group border-t">
              <summary className="flex cursor-pointer list-none items-center justify-between gap-3 px-5 py-3 text-sm font-medium hover:bg-muted/40 focus-visible:outline-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
                View device results ({results.length})
                <ChevronDown
                  aria-hidden="true"
                  className="size-4 text-muted-foreground transition-transform group-open:rotate-180"
                />
              </summary>
              <div className="space-y-3 px-5 pb-5">
                <div
                  role="group"
                  aria-label="Submission result filter"
                  className="flex gap-2"
                >
                  <Button
                    type="button"
                    size="sm"
                    variant={onlyRejected ? "ghost" : "secondary"}
                    aria-pressed={!onlyRejected}
                    onClick={() => setOnlyRejected(false)}
                  >
                    All results ({results.length})
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant={onlyRejected ? "secondary" : "ghost"}
                    aria-pressed={onlyRejected}
                    onClick={() => setOnlyRejected(true)}
                  >
                    Rejected ({rejectedCount})
                  </Button>
                </div>
                <DataTable<Result, unknown>
                  scrollable
                  data={onlyRejected ? (rejected ?? []) : results}
                  getRowId={(result) => result.device_id}
                  rowClassName={(result) =>
                    result.status === "rejected"
                      ? "bg-destructive/5 hover:bg-destructive/10"
                      : ""
                  }
                  columns={[
                    {
                      id: "device",
                      header: "Device / Seat",
                      cell: ({ row: { original: result } }) => {
                        const device = byDevice.get(result.device_id);
                        return (
                          <div className="space-y-0.5" title={result.device_id}>
                            <p className="font-medium">
                              {deviceLabel(device, result.device_id)}
                            </p>
                            <p className="font-mono text-xs text-muted-foreground">
                              {device?.machine_hardware_id ?? result.device_id}
                            </p>
                          </div>
                        );
                      },
                    },
                    {
                      id: "result",
                      header: "Submission",
                      cell: ({ row: { original: result } }) => (
                        <span
                          className={`inline-flex items-center gap-1.5 text-sm font-medium [&>svg]:size-3.5 [&>svg]:shrink-0 ${
                            result.status === "rejected"
                              ? "text-destructive"
                              : "text-emerald-700"
                          }`}
                        >
                          {result.status === "rejected" ? (
                            <CircleX aria-hidden="true" />
                          ) : (
                            <CircleCheck aria-hidden="true" />
                          )}
                          {result.status === "rejected"
                            ? "Rejected"
                            : "Submitted"}
                        </span>
                      ),
                    },
                    {
                      id: "detail",
                      header: "Details",
                      cell: ({ row: { original: result } }) => (
                        <p
                          className={`min-w-48 whitespace-normal break-words ${result.status === "rejected" ? "text-destructive" : "text-muted-foreground"}`}
                        >
                          {result.status === "rejected"
                            ? result.message
                            : "Target accepted by Server"}
                        </p>
                      ),
                    },
                  ]}
                />
              </div>
            </details>
          )}
        </>
      )}

      {submission.sending ? (
        <p role="status" className="px-5 pb-5 text-sm text-muted-foreground">
          Submitting target. Showing previous device states.
        </p>
      ) : submission.pending ? (
        <div className="px-5 pb-5">
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
                onClick={onRetry}
              >
                <RotateCcw aria-hidden="true" />
                Retry original request
              </Button>
            </AlertDescription>
          </Alert>
        </div>
      ) : submission.error ? (
        <div className="px-5 pb-5">
          <Alert variant="destructive">
            <AlertTitle>{submission.error}</AlertTitle>
          </Alert>
        </div>
      ) : null}
    </Card>
  );
}

function deviceLabel(device: Device | undefined, id: string) {
  const binding = device?.convergence.binding.target;
  return binding?.state === "bound" ? binding.context.seat_code : id;
}
