import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";

import { unwrap } from "@/api/errors";
import {
  executionCounts,
  executionLabels,
  executionRows,
  type PrintRow,
} from "@/pages/print-execution";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { PrintNavigation } from "@/components/print-navigation";
import { RefreshCountdown } from "@/components/device-status";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const columns: DataTableColumnDef<PrintRow>[] = [
  { accessorKey: "ready_sequence", header: "Admission sequence" },
  {
    id: "execution",
    header: "Execution",
    cell: ({ row }) =>
      row.original.execution
        ? executionLabels[row.original.execution.state]
        : "State unavailable",
  },
  {
    id: "attempts",
    header: "Attempts used",
    cell: ({ row }) => row.original.execution?.attempts_used ?? "Unknown",
  },
  {
    id: "outcome",
    header: "Last outcome",
    cell: ({ row }) =>
      row.original.execution
        ? (row.original.execution.last_outcome ?? "None reported")
        : "Unknown",
  },
  {
    id: "retry",
    header: "Retry eligible at",
    cell: ({ row }) =>
      row.original.execution?.retry_at_unix_ms == null
        ? "—"
        : new Date(row.original.execution.retry_at_unix_ms).toLocaleString(),
  },
  {
    accessorKey: "team_name",
    header: "Team",
    cell: ({ row }) => (
      <span className="block min-w-40 max-w-xs whitespace-normal break-all">
        {row.original.team_name}
      </span>
    ),
  },
  { accessorKey: "account", header: "Account" },
  {
    accessorKey: "seat_label",
    header: "Seat",
    cell: ({ row }) => row.original.seat_label ?? "Unassigned",
  },
  {
    id: "source",
    header: "Source",
    cell: ({ row }) => (
      <div className="min-w-40 max-w-xs whitespace-normal break-all">
        <span>{row.original.source_name}</span>
        <code className="mt-1 block text-xs text-muted-foreground">
          {row.original.source_id}
        </code>
      </div>
    ),
  },
  {
    accessorKey: "original_filename",
    header: "File",
    cell: ({ row }) => (
      <span className="block min-w-48 max-w-xs whitespace-normal break-all">
        {row.original.original_filename}
      </span>
    ),
  },
  { accessorKey: "pages", header: "Pages" },
  {
    accessorKey: "bytes",
    header: "Size",
    cell: ({ row }) => `${row.original.bytes.toLocaleString()} bytes`,
  },
  {
    accessorKey: "request_id",
    header: "Request ID",
    cell: ({ row }) => (
      <code className="block min-w-40 max-w-xs whitespace-normal break-all text-xs">
        {row.original.request_id}
      </code>
    ),
  },
];

export function PrintQueuePage() {
  const { api } = useSessionScope();
  const [filter, setFilter] = useState("");
  const [stateFilter, setStateFilter] = useState("all");
  const executions = useQuery({
    queryKey: ["print-executions"],
    queryFn: async ({ signal }) =>
      unwrap(await api.listPrintExecutions({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const queue = useQuery({
    queryKey: ["print-queue"],
    queryFn: async ({ signal }) => unwrap(await api.listPrintQueue({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const allRows = useMemo(
    () => executionRows(queue.data ?? [], executions.data ?? []),
    [queue.data, executions.data],
  );
  const counts = executionCounts(allRows);
  const rows = useMemo(() => {
    const search = filter.trim().toLocaleLowerCase();
    return allRows.filter(
      (entry) =>
        (stateFilter === "all" ||
          (entry.execution?.state ?? "unknown") === stateFilter) &&
        (!search ||
          [
            entry.team_name,
            entry.account,
            entry.seat_label,
            entry.source_name,
            entry.source_id,
            entry.original_filename,
            entry.request_id,
          ].some((value) => value?.toLocaleLowerCase().includes(search))),
    );
  }, [allRows, filter, stateFilter]);
  const pages =
    queue.data?.reduce((total, entry) => total + entry.pages, 0) ?? 0;

  return (
    <div className="min-w-0 space-y-4">
      <PrintNavigation />
      <div className="space-y-1">
        <h1 className="text-2xl font-semibold tracking-tight">Print queue</h1>
        <p className="text-sm text-muted-foreground">
          Admitted documents with current execution states. Queued does not mean
          printed. Admission sequence is not the live retry order. Spooler
          completion does not prove physical delivery. Preparing requests and
          generation failures are not in this queue.
        </p>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3">
        {queue.data && (
          <p className="text-sm tabular-nums" aria-live="polite">
            {queue.data.length.toLocaleString()} admitted{" "}
            {queue.data.length === 1 ? "task" : "tasks"} ·{" "}
            {pages.toLocaleString()} {pages === 1 ? "page" : "pages"}
          </p>
        )}
        <div className="ml-auto flex flex-wrap items-center gap-3">
          <RefreshCountdown
            resource="print queue"
            updatedAt={Math.max(queue.dataUpdatedAt, queue.errorUpdatedAt)}
            isFetching={queue.isFetching}
            isPaused={queue.isPaused}
            isError={queue.isError}
          />
          <Button
            type="button"
            variant="outline"
            disabled={queue.isFetching || executions.isFetching}
            onClick={() => {
              void queue.refetch();
              void executions.refetch();
            }}
          >
            Refresh queue
          </Button>
        </div>
      </div>
      <p className="text-sm text-muted-foreground" aria-live="polite">
        {counts.queued} ready to claim · {counts.active} executing ·{" "}
        {counts.waiting} retry waiting · {counts.completed} reported printed ·{" "}
        {counts.exhausted} exhausted · {counts.unknown} state unavailable
      </p>
      <label className="flex items-center gap-3 text-sm">
        Execution state
        <select
          aria-label="Execution state"
          value={stateFilter}
          onChange={(event) => setStateFilter(event.target.value)}
          className="h-9 rounded-md border bg-background px-3"
        >
          <option value="all">All states</option>
          {Object.entries(executionLabels).map(([state, label]) => (
            <option key={state} value={state}>
              {label}
            </option>
          ))}
          <option value="unknown">State unavailable</option>
        </select>
      </label>
      {executions.isError && (
        <Alert variant="destructive">
          <AlertTitle>Execution refresh failed</AlertTitle>
          <AlertDescription>
            {executions.data
              ? "Showing last received execution states; they may be out of date."
              : "Execution states are unavailable. Missing states are not treated as queued."}
          </AlertDescription>
        </Alert>
      )}
      <Input
        aria-label="Filter print queue"
        placeholder="Search team, account, seat, source, file or request ID"
        value={filter}
        onChange={(event) => setFilter(event.target.value)}
      />
      {queue.data && queue.isError && (
        <Alert variant="destructive">
          <AlertTitle>Queue refresh failed</AlertTitle>
          <AlertDescription>
            Showing the last received queue; it may be out of date.
          </AlertDescription>
        </Alert>
      )}
      {queue.data && (filter.trim() || stateFilter !== "all") && (
        <p className="text-sm text-muted-foreground">
          {rows.length} of {queue.data.length} admitted tasks match
        </p>
      )}
      <DataState
        isLoading={queue.isLoading}
        error={queue.data ? null : queue.error}
        isEmpty={!rows.length}
        emptyLabel={
          queue.data?.length
            ? "No admitted documents match the filter."
            : "No admitted documents."
        }
      >
        <DataTable
          columns={columns}
          data={rows}
          getRowId={(entry) => `${entry.source_id}/${entry.request_id}`}
        />
      </DataState>
    </div>
  );
}
