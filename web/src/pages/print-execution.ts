import type {
  ExecutionState,
  PrintExecution,
  PrintQueueEntry,
} from "@/api/generated/types";

export const executionLabels: Record<ExecutionState, string> = {
  queued: "Ready to claim",
  active: "Executing",
  waiting: "Retry waiting",
  completed: "Spooler reported printed",
  exhausted: "Attempts exhausted",
};

export type PrintRow = PrintQueueEntry & { execution?: PrintExecution };

export function executionRows(
  queue: PrintQueueEntry[],
  executions: PrintExecution[],
): PrintRow[] {
  const bySequence = new Map(
    executions.map((entry) => [entry.ready_sequence, entry]),
  );
  return queue.map((entry) => ({
    ...entry,
    execution: bySequence.get(entry.ready_sequence),
  }));
}

export function executionCounts(rows: PrintRow[]) {
  const counts: Record<ExecutionState | "unknown", number> = {
    queued: 0,
    active: 0,
    waiting: 0,
    completed: 0,
    exhausted: 0,
    unknown: 0,
  };
  for (const row of rows) counts[row.execution?.state ?? "unknown"] += 1;
  return counts;
}
