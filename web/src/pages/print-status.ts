import {
  CheckCheck,
  CircleHelp,
  Clock,
  Hourglass,
  OctagonX,
  Printer,
  type LucideIcon,
} from "lucide-react";

import type { AttemptOutcome, ExecutionState } from "@/api/generated/types";
import type { Tone } from "@/components/device-status-style";
import { executionLabels } from "@/pages/print-execution";

export type StageKey = ExecutionState | "unknown";

export const stages: Record<
  StageKey,
  { label: string; short: string; icon: LucideIcon; tone: Tone }
> = {
  queued: {
    label: executionLabels.queued,
    short: "Ready",
    icon: Clock,
    tone: "muted",
  },
  active: {
    label: executionLabels.active,
    short: "Executing",
    icon: Printer,
    tone: "info",
  },
  waiting: {
    label: executionLabels.waiting,
    short: "Retry waiting",
    icon: Hourglass,
    tone: "warning",
  },
  completed: {
    label: executionLabels.completed,
    short: "Printed",
    icon: CheckCheck,
    tone: "success",
  },
  exhausted: {
    label: executionLabels.exhausted,
    short: "Exhausted",
    icon: OctagonX,
    tone: "danger",
  },
  unknown: {
    label: "State unavailable",
    short: "Unavailable",
    icon: CircleHelp,
    tone: "muted",
  },
};

/** Pipeline order: in-flight stages first, then terminal ones. */
export const stageOrder: StageKey[] = [
  "queued",
  "active",
  "waiting",
  "completed",
  "exhausted",
  "unknown",
];

export const outcomeLabels: Record<AttemptOutcome, string> = {
  printed: "Printed",
  failed: "Failed",
  unknown: "Outcome unknown",
};

export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** `m:ss` until `target`, or null once it has passed. */
export function countdown(target: number, now: number) {
  const seconds = Math.ceil((target - now) / 1000);
  if (seconds <= 0) return null;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
