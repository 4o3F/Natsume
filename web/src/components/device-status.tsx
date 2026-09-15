import { useEffect, useState } from "react";

import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { convergenceIcons, statusIcons } from "./device-status-style";

type ConvergenceStatus =
  components["schemas"]["DeviceConvergenceResponse"]["home"]["status"];

export function StatusIcon({
  icon,
  label,
  tone,
}: {
  icon: keyof typeof statusIcons;
  label: string;
  tone: string;
}) {
  const Icon = statusIcons[icon];
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className={`inline-flex size-7 shrink-0 items-center justify-center ${tone}`}
    >
      <Icon aria-hidden="true" className="size-4.5" />
    </span>
  );
}

export function ConvergenceIcon({
  name,
  status,
}: {
  name: string;
  status: ConvergenceStatus;
}) {
  return (
    <StatusIcon
      {...convergenceIcons[status]}
      label={`${name}: ${status.replaceAll("_", " ")}`}
    />
  );
}

export function RefreshCountdown({
  resource = "device",
  updatedAt,
  isFetching,
  isPaused,
  isError,
}: {
  resource?: string;
  updatedAt: number;
  isFetching: boolean;
  isPaused: boolean;
  isError: boolean;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    // Polling restarts its interval after a response, so align the display ticks.
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [updatedAt]);
  const seconds = Math.max(
    0,
    Math.ceil((updatedAt + LIST_POLL_MS - Math.max(now, updatedAt)) / 1_000),
  );
  return (
    <div
      role="timer"
      aria-label={`Next ${resource} refresh`}
      aria-live="off"
      title={
        isError
          ? `Refresh failed; showing the last received ${resource} states.`
          : `Automatically refreshes ${resource} states.`
      }
      className="flex min-w-40 shrink-0 items-center justify-end gap-1 text-sm tabular-nums text-muted-foreground"
    >
      <StatusIcon
        icon={
          isPaused
            ? "pause"
            : isFetching
              ? "sync"
              : isError
                ? "failed"
                : "clock"
        }
        tone={isError ? "text-destructive" : "text-muted-foreground"}
        label="Automatic refresh"
      />
      <span>
        {isPaused
          ? "Refresh paused"
          : isFetching
            ? "Refreshing…"
            : `${isError ? "Retry" : "Refresh"} in ${seconds}s`}
      </span>
    </div>
  );
}
