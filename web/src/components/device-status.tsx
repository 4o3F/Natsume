import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";

import type { DeviceConvergenceResponse_Serialize as DeviceConvergenceResponse } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  convergenceIcons,
  convergenceParts,
  statusIcons,
  tones,
} from "./device-status-style";

type ConvergenceStatus = DeviceConvergenceResponse["home"]["status"];

export function StatusIcon({
  icon,
  label,
  tone,
  className,
}: {
  icon: keyof typeof statusIcons;
  label: string;
  tone: string;
  className?: string;
}) {
  const Icon = statusIcons[icon];
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex size-6 shrink-0 items-center justify-center",
        tone,
        className,
      )}
    >
      <Icon aria-hidden="true" className="size-4" />
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

/** Colored dot with an accessible label; optional pulse for live states. */
export function StatusDot({
  tone,
  label,
  pulse = false,
  className,
}: {
  tone: keyof typeof tones;
  label?: string;
  pulse?: boolean;
  className?: string;
}) {
  return (
    <span
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={cn(
        "inline-block size-2 shrink-0 rounded-full",
        tones[tone].dot,
        pulse && cn("animate-pulse-ring", tones[tone].text),
        className,
      )}
    />
  );
}

/**
 * Five-segment convergence summary. Each segment is colored by status and
 * the hover card lists every part, replacing five separate icon columns.
 */
export function ConvergenceStrip({
  convergence,
}: {
  convergence: DeviceConvergenceResponse;
}) {
  const statuses = convergenceParts.map(
    ([key, name]) => [name, convergence[key].status] as const,
  );
  const summary = statuses
    .map(([name, status]) => `${name}: ${status.replaceAll("_", " ")}`)
    .join(", ");
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          role="img"
          aria-label={summary}
          tabIndex={0}
          className="inline-flex items-center gap-1 rounded-md p-1 outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
        >
          {statuses.map(([name, status]) => (
            <span
              key={name}
              className={cn(
                "h-4 w-2 rounded-sm",
                tones[convergenceIcons[status].key].dot,
              )}
            />
          ))}
        </span>
      </TooltipTrigger>
      <TooltipContent side="bottom" className="p-2.5">
        <ul className="grid gap-1">
          {statuses.map(([name, status]) => (
            <li key={name} className="flex items-center justify-between gap-6">
              <span className="flex items-center gap-2">
                <span
                  className={cn(
                    "size-2 rounded-full",
                    tones[convergenceIcons[status].key].dot,
                  )}
                />
                {name}
              </span>
              <span className="opacity-70">{status.replaceAll("_", " ")}</span>
            </li>
          ))}
        </ul>
      </TooltipContent>
    </Tooltip>
  );
}

export function RefreshCountdown({
  resource = "device",
  updatedAt,
  isFetching,
  isPaused,
  isError,
  onRefresh,
}: {
  resource?: string;
  updatedAt: number;
  isFetching: boolean;
  isPaused: boolean;
  isError: boolean;
  onRefresh?: () => void;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    // Polling restarts its interval after a response, so align the display ticks.
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [updatedAt]);
  const remaining = Math.max(
    0,
    updatedAt + LIST_POLL_MS - Math.max(now, updatedAt),
  );
  const seconds = Math.ceil(remaining / 1_000);
  const progress = isFetching || isPaused ? 1 : remaining / LIST_POLL_MS;
  const label = isPaused
    ? "Paused"
    : isFetching
      ? "Refreshing…"
      : `${isError ? "Retry" : "Refresh"} in ${seconds}s`;
  const content = (
    <>
      <svg
        viewBox="0 0 16 16"
        aria-hidden="true"
        className={cn("size-4 -rotate-90", isFetching && "animate-spin")}
      >
        <circle
          cx="8"
          cy="8"
          r="6"
          fill="none"
          strokeWidth="2"
          className="stroke-border"
        />
        <circle
          cx="8"
          cy="8"
          r="6"
          fill="none"
          strokeWidth="2"
          strokeLinecap="round"
          pathLength={100}
          strokeDasharray={`${Math.round(progress * 100)} 100`}
          className={cn(
            "transition-[stroke-dasharray] duration-1000 ease-linear",
            isError ? "stroke-destructive" : "stroke-primary",
          )}
        />
      </svg>
      <span>{label}</span>
      {onRefresh && (
        <RefreshCw
          aria-hidden="true"
          className="size-3.5 opacity-0 transition-opacity group-hover:opacity-100"
        />
      )}
    </>
  );
  const className = cn(
    "group inline-flex h-8 shrink-0 items-center gap-2 border bg-card px-3 text-caption tabular-nums text-muted-foreground",
    isError && "border-destructive/30 text-destructive",
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
    >
      {onRefresh ? (
        <button
          type="button"
          onClick={onRefresh}
          disabled={isFetching}
          className={cn(
            className,
            "cursor-pointer transition-colors outline-none hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
          )}
        >
          {content}
          <span className="sr-only">Refresh now</span>
        </button>
      ) : (
        <span className={className}>{content}</span>
      )}
    </div>
  );
}

export function ConvergenceBadge({ status }: { status: ConvergenceStatus }) {
  const key = convergenceIcons[status].key;
  return (
    <Badge variant={tones[key].badge} dot>
      {status.replaceAll("_", " ")}
    </Badge>
  );
}
