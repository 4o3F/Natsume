import { useMemo, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ChevronRight,
  FileCode2,
  FileText,
  Info,
  Search,
  X,
} from "lucide-react";

import { unwrap } from "@/api/errors";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { CopyButton } from "@/components/copy-button";
import { usePageToast } from "@/components/use-page-toast";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { RefreshCountdown, StatusDot } from "@/components/device-status";
import { tones } from "@/components/device-status-style";
import { PageHeader } from "@/components/page-header";
import { StatGrid, StatTile } from "@/components/stat-tile";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { InputGroup } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";
import { useOperatorAction } from "@/operator/use-operator-action";
import { printManagementError } from "@/pages/print-management";
import {
  executionCounts,
  executionRows,
  type PrintRow,
} from "@/pages/print-execution";
import {
  countdown,
  formatBytes,
  outcomeLabels,
  stageOrder,
  stages,
  type StageKey,
} from "@/pages/print-status";

const rowId = (entry: PrintRow) => `${entry.source_id}/${entry.request_id}`;
const stageOf = (entry: PrintRow): StageKey =>
  entry.execution?.state ?? "unknown";
const CODE_FILE = /\.(c|cc|cpp|cxx|h|hpp|java|kt|py|rs|go|js|ts|pas|cs)$/i;

const columns: DataTableColumnDef<PrintRow>[] = [
  {
    accessorKey: "ready_sequence",
    header: "#",
    enableSorting: true,
    cell: ({ row }) => (
      <span className="font-mono text-xs text-muted-foreground tabular-nums">
        {row.original.ready_sequence}
      </span>
    ),
  },
  {
    accessorKey: "original_filename",
    header: "Document",
    cell: ({ row }) => <DocumentCell entry={row.original} />,
  },
  {
    accessorKey: "team_name",
    header: "Team",
    cell: ({ row }) => (
      <div className="flex min-w-0 items-center gap-2.5">
        <Badge variant="outline" className="shrink-0 font-mono">
          {row.original.seat_label ?? "—"}
        </Badge>
        <div className="min-w-0 max-w-56 leading-tight">
          <p className="truncate" title={row.original.team_name}>
            {row.original.team_name}
          </p>
          <p className="truncate font-mono text-xs text-muted-foreground">
            {row.original.account}
          </p>
        </div>
      </div>
    ),
  },
  {
    accessorKey: "source_name",
    header: "Source",
    cell: ({ row }) => (
      <span className="text-sm text-muted-foreground">
        {row.original.source_name}
      </span>
    ),
  },
  {
    id: "status",
    header: "Status",
    cell: ({ row }) => <StatusCell entry={row.original} />,
  },
  {
    id: "open",
    header: () => <span className="sr-only">Details</span>,
    cell: () => (
      <ChevronRight
        aria-hidden="true"
        className="size-4 text-muted-foreground"
      />
    ),
  },
];

export function PrintQueuePage() {
  const { api } = useSessionScope();
  const [filter, setFilter] = useState("");
  const [stageFilter, setStageFilter] = useState<StageKey | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
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
        (!stageFilter || stageOf(entry) === stageFilter) &&
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
  }, [allRows, filter, stageFilter]);
  const pages = allRows.reduce((total, entry) => total + entry.pages, 0);
  const openRow = allRows.find((entry) => rowId(entry) === openId) ?? null;
  const filtered = Boolean(filter.trim() || stageFilter);

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Print queue"
        description="Admitted documents and where each one is in execution."
        actions={
          <>
            <ReadingGuide />
            <RefreshCountdown
              resource="print queue"
              updatedAt={Math.min(
                Math.max(queue.dataUpdatedAt, queue.errorUpdatedAt),
                Math.max(executions.dataUpdatedAt, executions.errorUpdatedAt),
              )}
              isFetching={queue.isFetching || executions.isFetching}
              isPaused={queue.isPaused}
              isError={queue.isError || executions.isError}
              onRefresh={() => {
                void queue.refetch();
                void executions.refetch();
              }}
            />
          </>
        }
      />

      {queue.data && (
        <section aria-label="Queue summary" className="space-y-3">
          <div className="flex flex-wrap items-end justify-between gap-3">
            <p className="flex items-baseline gap-2">
              <span className="text-headline font-light tabular-nums">
                {allRows.length}
              </span>
              <span className="text-sm text-muted-foreground">
                admitted {allRows.length === 1 ? "document" : "documents"} ·{" "}
                {pages.toLocaleString()} {pages === 1 ? "page" : "pages"}
              </span>
            </p>
            <p className="text-xs text-muted-foreground" aria-live="polite">
              {counts.queued + counts.active + counts.waiting} still in flight
            </p>
          </div>
          <StageBar counts={counts} total={allRows.length} />
          <StatGrid>
            {stageOrder.map((key) => {
              const stage = stages[key];
              const Icon = stage.icon;
              if (key === "unknown" && !counts.unknown) return null;
              return (
                <StatTile
                  key={key}
                  label={stage.short}
                  value={counts[key]}
                  tone={counts[key] ? stage.tone : "muted"}
                  icon={<Icon aria-hidden="true" />}
                  active={stageFilter === key}
                  onClick={() =>
                    setStageFilter(stageFilter === key ? null : key)
                  }
                />
              );
            })}
          </StatGrid>
        </section>
      )}

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
      {queue.data && queue.isError && (
        <Alert variant="destructive">
          <AlertTitle>Queue refresh failed</AlertTitle>
          <AlertDescription>
            Showing the last received queue; it may be out of date.
          </AlertDescription>
        </Alert>
      )}

      <section aria-label="Admitted documents" className="space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <InputGroup
            type="search"
            aria-label="Filter print queue"
            placeholder="Team, account, seat, file, or request ID"
            icon={<Search aria-hidden="true" />}
            className="w-full sm:w-80"
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
          {stageFilter && (
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={() => setStageFilter(null)}
            >
              {stages[stageFilter].short}
              <X aria-hidden="true" />
              <span className="sr-only">Clear state filter</span>
            </Button>
          )}
          {queue.data && filtered && (
            <p
              role="status"
              className="ml-auto text-xs text-muted-foreground tabular-nums"
            >
              {rows.length} of {allRows.length} match
            </p>
          )}
        </div>
        <DataState
          isLoading={queue.isLoading}
          error={queue.data ? null : queue.error}
          isEmpty={!rows.length}
          emptyLabel={allRows.length ? "Nothing matches" : "The queue is empty"}
          emptyDescription={
            allRows.length
              ? "Try a different search or clear the state filter."
              : "Admitted documents appear here once a print source submits them."
          }
        >
          <DataTable
            label="Admitted documents"
            columns={columns}
            data={rows}
            getRowId={rowId}
            onRowClick={(entry) => setOpenId(rowId(entry))}
            activeRowId={openId}
            rowClassName={(entry) =>
              stageOf(entry) === "exhausted"
                ? "bg-destructive-subtle/40 hover:bg-destructive-subtle/70"
                : ""
            }
          />
        </DataState>
      </section>

      <Sheet
        open={openRow !== null}
        onOpenChange={(open) => !open && setOpenId(null)}
      >
        <SheetContent className="sm:max-w-md">
          {openRow && <DocumentDetails key={rowId(openRow)} entry={openRow} />}
        </SheetContent>
      </Sheet>
    </div>
  );
}

function StageBar({
  counts,
  total,
}: {
  counts: Record<StageKey, number>;
  total: number;
}) {
  if (!total) return null;
  return (
    <div
      role="img"
      aria-label={stageOrder
        .filter((key) => counts[key])
        .map((key) => `${counts[key]} ${stages[key].short}`)
        .join(", ")}
      className="flex h-2 gap-px overflow-hidden bg-surface-2"
    >
      {stageOrder.map(
        (key) =>
          counts[key] > 0 && (
            <div
              key={key}
              className={cn(
                "h-full transition-[flex-grow] duration-500",
                key === "queued"
                  ? "bg-muted-foreground/40"
                  : tones[stages[key].tone].dot,
              )}
              style={{ flexGrow: counts[key] }}
            />
          ),
      )}
    </div>
  );
}

function ReadingGuide() {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button type="button" variant="ghost" size="sm">
          <Info aria-hidden="true" />
          Reading this queue
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 space-y-2 text-sm">
        <p className="font-semibold">What the states mean</p>
        <ul className="list-disc space-y-1.5 pl-4 text-muted-foreground">
          <li>Ready to claim does not mean printed.</li>
          <li>The admission number (#) is not the live retry order.</li>
          <li>
            Printed means the spooler reported completion; it does not prove the
            paper reached the team.
          </li>
          <li>
            Requests still preparing and generation failures are not in this
            queue.
          </li>
        </ul>
      </PopoverContent>
    </Popover>
  );
}

function DocumentCell({ entry }: { entry: PrintRow }) {
  const Icon = CODE_FILE.test(entry.original_filename) ? FileCode2 : FileText;
  return (
    <div className="flex min-w-0 items-center gap-2.5">
      <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
        <Icon aria-hidden="true" className="size-4" />
      </span>
      <div className="min-w-0 max-w-64 leading-tight">
        <p
          className="truncate font-mono text-sm font-medium"
          title={entry.original_filename}
        >
          {entry.original_filename}
        </p>
        <p className="text-xs text-muted-foreground tabular-nums">
          {entry.pages} {entry.pages === 1 ? "page" : "pages"} ·{" "}
          {formatBytes(entry.bytes)}
        </p>
      </div>
    </div>
  );
}

function StageBadge({ stage }: { stage: StageKey }) {
  const { short, icon: Icon, tone } = stages[stage];
  return (
    <Badge variant={tones[tone].badge}>
      {stage === "active" ? (
        <StatusDot tone="info" pulse />
      ) : (
        <Icon aria-hidden="true" />
      )}
      {short}
    </Badge>
  );
}

function RetryIn({ at }: { at: number }) {
  const now = useNow();
  const left = countdown(at, now);
  return (
    <span className="tabular-nums">
      {left ? `retry in ${left}` : "retry due"}
    </span>
  );
}

function StatusCell({ entry }: { entry: PrintRow }) {
  const execution = entry.execution;
  const stage = stageOf(entry);
  return (
    <div className="flex flex-col items-start gap-1">
      <StageBadge stage={stage} />
      {execution &&
        (execution.attempts_used > 0 || execution.requeue_count > 0) && (
          <span className="text-xs text-muted-foreground">
            {execution.requeue_count > 0 && (
              <>Round {execution.requeue_count + 1} · </>
            )}
            {execution.attempts_used}{" "}
            {execution.attempts_used === 1 ? "attempt" : "attempts"}
            {execution.retry_at_unix_ms != null ? (
              <>
                {" · "}
                <RetryIn at={execution.retry_at_unix_ms} />
              </>
            ) : (
              execution.last_outcome && (
                <> · {outcomeLabels[execution.last_outcome].toLowerCase()}</>
              )
            )}
          </span>
        )}
    </div>
  );
}

function DocumentDetails({ entry }: { entry: PrintRow }) {
  const { api, identity, queryClient } = useSessionScope();
  const toast = usePageToast();
  const action = useOperatorAction(printManagementError);
  const [confirmation, setConfirmation] = useState<number | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const execution = entry.execution;
  const stage = stageOf(entry);
  const isAdmin = identity?.role === "admin";
  const unchanged =
    stage === "exhausted" && execution?.requeue_count === confirmation;
  const requeue = () => {
    if (!isAdmin || !unchanged || !acknowledged || confirmation === null)
      return;
    void action.run(
      async (signal) => {
        try {
          return unwrap(
            await api.requeuePrintExecution(
              entry.ready_sequence,
              {
                expected_requeue_count: confirmation,
              },
              { signal },
            ),
          );
        } finally {
          if (!signal.aborted)
            void queryClient.invalidateQueries({
              queryKey: ["print-executions"],
            });
        }
      },
      () => {
        setConfirmation(null);
        toast.success("Document requeued", {
          description:
            "A new round allows up to 3 attempts. This does not confirm printing.",
        });
      },
    );
  };
  return (
    <>
      <SheetHeader>
        <SheetTitle className="font-mono break-all">
          {entry.original_filename}
        </SheetTitle>
        <SheetDescription>
          {entry.team_name} · seat {entry.seat_label ?? "unassigned"}
        </SheetDescription>
      </SheetHeader>
      <SheetBody>
        <section
          aria-label="Execution"
          className="space-y-3 rounded-lg border bg-muted/40 p-4"
        >
          <div className="flex items-center justify-between gap-2">
            <StageBadge stage={stage} />
            <span className="text-xs text-muted-foreground">
              {stages[stage].label}
            </span>
          </div>
          <dl className="grid grid-cols-2 gap-3 text-sm">
            <Fact label="Attempts used (this round)">
              {execution ? `${execution.attempts_used} of 3` : "Unknown"}
            </Fact>
            <Fact label="Attempt round">
              {execution ? execution.requeue_count + 1 : "Unknown"}
            </Fact>
            <Fact label="Last outcome">
              {execution
                ? execution.last_outcome
                  ? outcomeLabels[execution.last_outcome]
                  : "None reported"
                : "Unknown"}
            </Fact>
            {execution?.retry_at_unix_ms != null && (
              <Fact label="Retry eligible">
                <RetryIn at={execution.retry_at_unix_ms} />
                <span className="block text-xs text-muted-foreground">
                  {new Date(execution.retry_at_unix_ms).toLocaleTimeString()}
                </span>
              </Fact>
            )}
          </dl>
        </section>
        <dl className="space-y-3 text-sm">
          <Fact label="Document">
            {entry.pages} {entry.pages === 1 ? "page" : "pages"} ·{" "}
            {formatBytes(entry.bytes)} ({entry.bytes.toLocaleString()} bytes)
          </Fact>
          <Fact label="Admission number">#{entry.ready_sequence}</Fact>
          <Fact label="Account">
            <span className="font-mono">{entry.account}</span>
          </Fact>
          <Fact label="Source">
            {entry.source_name}
            <IdLine value={entry.source_id} label="Copy source ID" />
          </Fact>
          <Fact label="Request ID">
            <IdLine value={entry.request_id} label="Copy request ID" />
          </Fact>
        </dl>
      </SheetBody>
      <AlertDialog
        open={confirmation !== null}
        onOpenChange={(open) => {
          if (action.pending) return;
          if (open && isAdmin && stage === "exhausted" && execution) {
            setAcknowledged(false);
            setConfirmation(execution.requeue_count);
          } else if (!open) setConfirmation(null);
        }}
      >
        {isAdmin && stage === "exhausted" && execution && (
          <SheetFooter>
            <AlertDialogTrigger asChild>
              <Button
                type="button"
                className="min-h-12"
                disabled={action.pending}
              >
                Requeue document
              </Button>
            </AlertDialogTrigger>
          </SheetFooter>
        )}
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Requeue this document?</AlertDialogTitle>
            <AlertDialogDescription>
              {entry.original_filename} will be added to the back of the ready
              queue with up to 3 new attempts. The original document and attempt
              history are preserved. Previous attempts may already have printed;
              requeuing can produce duplicate copies. Check the physical output
              and Windows queue first.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <label className="flex min-h-12 items-start gap-2 text-sm">
            <Checkbox
              checked={acknowledged}
              disabled={action.pending}
              onCheckedChange={(value) => setAcknowledged(value === true)}
            />
            <span>
              I have checked the output and accept the risk of duplicate
              printing.
            </span>
          </label>
          {action.error && (
            <Alert variant="destructive">
              <AlertTitle>{action.error}</AlertTitle>
            </Alert>
          )}
          {!unchanged && (
            <Alert>
              <AlertTitle>
                Print state changed. Close this confirmation and review the
                current state.
              </AlertTitle>
            </Alert>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel className="min-h-12" disabled={action.pending}>
              Cancel
            </AlertDialogCancel>
            <Button
              type="button"
              loading={action.pending}
              className="min-h-12"
              disabled={!acknowledged || !unchanged || !isAdmin}
              onClick={requeue}
            >
              Requeue document
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col-reverse gap-0.5">
      <dd>{children}</dd>
      <dt className="text-xs text-muted-foreground">{label}</dt>
    </div>
  );
}

function IdLine({ value, label }: { value: string; label: string }) {
  return (
    <span className="flex items-center gap-1">
      <code className="font-mono text-xs break-all text-muted-foreground">
        {value}
      </code>
      <CopyButton value={value} label={label} />
    </span>
  );
}
