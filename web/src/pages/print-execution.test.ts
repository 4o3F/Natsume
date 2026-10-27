import { describe, expect, it } from "vitest";
import type { PrintExecution, PrintQueueEntry } from "@/api/generated/types";
import {
  executionCounts,
  executionLabels,
  executionRows,
} from "./print-execution";
import { validPrintName } from "./print-management";

const entry: PrintQueueEntry = {
  source_id: "source",
  source_name: "Judge",
  request_id: "request",
  account: "Team001",
  team_name: "Team",
  seat_label: null,
  original_filename: "code.cpp",
  ready_sequence: 1,
  pages: 1,
  bytes: 721,
};

describe("print execution projection", () => {
  it("associates immutable admission sequences rather than response positions", () => {
    const execution: PrintExecution = {
      ready_sequence: 1,
      state: "waiting",
      attempts_used: 2,
      requeue_count: 0,
      retry_at_unix_ms: 30000,
      last_outcome: "unknown",
    };
    const rows = executionRows(
      [entry, { ...entry, ready_sequence: 2 }],
      [{ ...execution, ready_sequence: 99 }, execution],
    );
    expect(rows[0].execution).toEqual(execution);
    expect(rows[1].execution).toBeUndefined();
    expect(executionCounts(rows)).toEqual({
      queued: 0,
      active: 0,
      waiting: 1,
      completed: 0,
      exhausted: 0,
      unknown: 1,
    });
  });
  it("never infers queued from missing execution data", () => {
    expect(executionCounts(executionRows([entry], [])).queued).toBe(0);
    expect(executionCounts(executionRows([entry], [])).unknown).toBe(1);
  });
  it("keeps completion labels at the printing-system boundary", () => {
    expect(executionLabels.completed).toBe("Spooler reported printed");
  });
});

describe("print names", () => {
  it("counts Unicode characters and rejects controls", () => {
    expect(validPrintName("😀".repeat(128))).toBe(true);
    expect(validPrintName("😀".repeat(129))).toBe(false);
    expect(validPrintName("")).toBe(false);
    expect(validPrintName("name\nnext")).toBe(false);
    expect(validPrintName("name\u007f")).toBe(false);
  });
});
