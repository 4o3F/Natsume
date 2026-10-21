import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";

import { unwrap } from "@/api/errors";
import type { PrintQueueEntry } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { RefreshCountdown } from "@/components/device-status";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const columns: DataTableColumnDef<PrintQueueEntry>[] = [
  { accessorKey: "ready_sequence", header: "Queue sequence" },
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
  const queue = useQuery({
    queryKey: ["print-queue"],
    queryFn: async ({ signal }) => unwrap(await api.listPrintQueue({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const rows = useMemo(() => {
    const search = filter.trim().toLocaleLowerCase();
    return (queue.data ?? []).filter(
      (entry) =>
        !search ||
        [
          entry.team_name,
          entry.account,
          entry.seat_label,
          entry.source_name,
          entry.source_id,
          entry.original_filename,
          entry.request_id,
        ].some((value) => value?.toLocaleLowerCase().includes(search)),
    );
  }, [queue.data, filter]);
  const pages =
    queue.data?.reduce((total, entry) => total + entry.pages, 0) ?? 0;

  return (
    <div className="min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-2xl font-semibold tracking-tight">Print queue</h1>
        <p className="text-sm text-muted-foreground">
          Admitted documents in FIFO order. Queued does not mean printed.
          Preparing requests and generation failures are not in this queue.
        </p>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3">
        {queue.data && (
          <p className="text-sm tabular-nums" aria-live="polite">
            {queue.data.length.toLocaleString()} queued{" "}
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
            disabled={queue.isFetching}
            onClick={() => void queue.refetch()}
          >
            Refresh queue
          </Button>
        </div>
      </div>
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
      {queue.data && filter.trim() && (
        <p className="text-sm text-muted-foreground">
          {rows.length} of {queue.data.length} queued tasks match
        </p>
      )}
      <DataState
        isLoading={queue.isLoading}
        error={queue.data ? null : queue.error}
        isEmpty={!rows.length}
        emptyLabel={
          queue.data?.length
            ? "No queued documents match the filter."
            : "No queued documents."
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
