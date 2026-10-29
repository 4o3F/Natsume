import {
  type FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useSearchParams } from "react-router";
import {
  ArrowUp,
  ChevronDown,
  CircleAlert,
  CircleX,
  Download,
  FileQuestion,
  Info,
  RotateCcw,
  Search,
  TriangleAlert,
  UserRound,
} from "lucide-react";

import { journalExportHref } from "@/api/client";
import { ApiError, unwrap } from "@/api/errors";
import type {
  EventPage,
  JournalRecord,
  OperatorAccountResponse,
  Severity,
} from "@/api/generated/types";
import { useSessionScope } from "@/auth/session-context";
import { useSession } from "@/auth/use-session";
import { DataState, EmptyState } from "@/components/data-state";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import {
  categoryLabels,
  describeEvent,
  eventKindLabels,
  labelFor,
  severityLabels,
  shortId,
  type EventKind,
} from "@/components/event-labels";
import {
  JournalHealthNotice,
  RetainedSince,
} from "@/components/journal-health";
import { MultiSelectFilter } from "@/components/multi-select-filter";
import { PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input, InputGroup } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  browserOffsetMinutes,
  formatOffset,
  formatTimestamp,
  fromDateTimeLocal,
  toDateTimeLocal,
} from "@/lib/time";
import { cn } from "@/lib/utils";
import {
  EVENT_POLL_MS,
  MAX_EVENT_ROWS,
  appendOlder,
  applyHead,
  emptyFeed,
  feedFromPage,
  fetchHead,
  headId,
  recordId,
  showPending,
  type EventFeed,
} from "./event-feed";
import {
  categoryPresets,
  defaultEventFilters,
  deviceFilterFromText,
  eventQuery,
  isLiveRange,
  kindsFor,
  parseEventFilters,
  rangeCaption,
  resolveEventRange,
  serializeEventFilters,
  severityOrder,
  timeRangeLabels,
  withCursor,
  type EventFilters,
  type EventRange,
  type TimeRange,
} from "./event-filters";

/** Scroll offset under which the reader counts as "at the newest events". */
const TOP_THRESHOLD_PX = 64;

interface InitialPage {
  query: URLSearchParams;
  range: EventRange;
  page: EventPage;
}

const severityBadges = {
  info: { variant: "muted", icon: Info },
  warning: { variant: "warning", icon: TriangleAlert },
  error: { variant: "destructive", icon: CircleX },
} as const satisfies Record<
  Severity,
  { variant: "muted" | "warning" | "destructive"; icon: typeof Info }
>;

const severityOptions = severityOrder.map((value) => ({
  value,
  label: severityLabels[value],
}));

function selection<T extends string>(all: readonly T[], selected: T[]) {
  return Object.fromEntries(
    all.map((value) => [
      value,
      selected.length === 0 || selected.includes(value),
    ]),
  ) as Record<T, boolean>;
}

function selected<T extends string>(
  all: readonly T[],
  value: Record<T, boolean>,
) {
  const chosen = all.filter((item) => value[item]);
  // Selecting everything or nothing both mean "no narrowing".
  return chosen.length === all.length ? [] : chosen;
}

export function EventsPage() {
  const { api } = useSessionScope();
  const isAdmin = useSession().data?.role === "admin";
  const [searchParams, setSearchParams] = useSearchParams();
  const filters = useMemo(
    () => parseEventFilters(searchParams, isAdmin),
    [searchParams, isAdmin],
  );
  const [browserOffset] = useState(() => browserOffsetMinutes(Date.now()));
  const [serverOffset, setServerOffset] = useState<number | null>(null);
  const offset = serverOffset ?? browserOffset;
  const filterKey = serializeEventFilters(filters).toString();

  const initial = useQuery({
    // "Today" depends on the server offset, which the first response reveals.
    queryKey: [
      "journal",
      "events",
      filterKey,
      filters.range === "today" ? offset : null,
    ],
    queryFn: async ({ signal }): Promise<InitialPage> => {
      const range = resolveEventRange(filters, Date.now(), offset);
      const query = eventQuery(filters, range);
      const page = await unwrap<EventPage>(
        await api.listJournalEvents(query, { signal }),
      );
      setServerOffset(page.journal.server_utc_offset_minutes);
      return { query, range, page };
    },
    staleTime: Infinity,
    gcTime: 0,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const data = initial.data;

  // Older pages and head polling extend the initial page for the same query.
  const [local, setLocal] = useState<{
    data: InitialPage;
    feed: EventFeed;
  } | null>(null);
  const initialFeed = useMemo(
    () => (data === undefined ? emptyFeed : feedFromPage(data.page)),
    [data],
  );
  const feed = local !== null && local.data === data ? local.feed : initialFeed;
  const dataRef = useRef(data);
  useEffect(() => {
    dataRef.current = data;
  });
  const update = useCallback(
    (owner: InitialPage, change: (feed: EventFeed) => EventFeed) =>
      setLocal((previous) =>
        // A late result for a replaced initial page must not take the slot.
        owner !== dataRef.current
          ? previous
          : {
              data: owner,
              feed: change(
                previous?.data === owner
                  ? previous.feed
                  : feedFromPage(owner.page),
              ),
            },
      ),
    [],
  );

  const olderAbort = useRef<AbortController>(null);
  // Leaving this page of results (filters, restart, unmount) cancels its
  // pending older request.
  useEffect(() => () => olderAbort.current?.abort(), [data]);
  const older = useMutation({
    mutationFn: async ({
      owner,
      before,
    }: {
      owner: InitialPage;
      before: number;
    }) => {
      const controller = new AbortController();
      olderAbort.current = controller;
      return unwrap<EventPage>(
        await api.listJournalEvents(withCursor(owner.query, { before }), {
          signal: controller.signal,
        }),
      );
    },
    onSuccess: (page, { owner, before }) =>
      update(owner, (current) => appendOlder(current, page, before)),
  });

  const live = isLiveRange(filters.range);
  const feedRef = useRef(feed);
  useEffect(() => {
    feedRef.current = feed;
  });
  const [poll, setPoll] = useState<{ owner: InitialPage; error: unknown }>();
  useEffect(() => {
    if (!live || !data) return;
    const controller = new AbortController();
    let running = false;
    const timer = window.setInterval(async () => {
      if (running || document.hidden) return;
      running = true;
      try {
        const head = await fetchHead(
          headId(feedRef.current) ?? 0,
          async (cursor) =>
            unwrap<EventPage>(
              await api.listJournalEvents(withCursor(data.query, cursor), {
                signal: controller.signal,
              }),
            ),
          MAX_EVENT_ROWS,
        );
        if (controller.signal.aborted) return;
        update(data, (current) =>
          applyHead(current, head, window.scrollY < TOP_THRESHOLD_PX),
        );
        setPoll({ owner: data, error: null });
      } catch (error) {
        if (!controller.signal.aborted) setPoll({ owner: data, error });
      } finally {
        running = false;
      }
    }, EVENT_POLL_MS);
    return () => {
      window.clearInterval(timer);
      controller.abort();
    };
  }, [live, data, api, update]);
  const pollError = poll?.owner === data ? poll?.error : null;
  const olderCurrent = older.variables?.owner === data;

  const setFilters = useCallback(
    (next: EventFilters) =>
      setSearchParams(serializeEventFilters(next), { replace: true }),
    [setSearchParams],
  );
  const journal = feed.journal;
  const shownOffset = journal?.server_utc_offset_minutes ?? offset;
  const hasActiveFilters = filterKey !== "";

  const columns = useMemo(
    () => eventColumns(feed.operators, shownOffset),
    [feed.operators, shownOffset],
  );

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Events"
        description="Journal of device connections, state changes and operator activity, newest first."
        actions={
          isAdmin &&
          (data ? (
            <Button asChild variant="outline" size="sm">
              <a href={journalExportHref(data.query)} download>
                <Download aria-hidden="true" />
                Export JSONL
              </a>
            </Button>
          ) : (
            // The link needs the resolved range, known with the first page.
            <Button type="button" variant="outline" size="sm" disabled>
              <Download aria-hidden="true" />
              Export JSONL
            </Button>
          ))
        }
      />

      {journal && <JournalHealthNotice journal={journal} />}

      <section aria-label="Event list" className="space-y-3">
        <EventFilterBar
          filters={filters}
          isAdmin={isAdmin}
          offset={offset}
          onChange={setFilters}
        />
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
          {journal && <RetainedSince journal={journal} />}
          <p
            role="status"
            className={cn(
              "ml-auto text-xs tabular-nums",
              pollError ? "text-destructive" : "text-muted-foreground",
            )}
          >
            {!live
              ? "Fixed range; live updates paused"
              : !data
                ? initial.isError
                  ? "Events unavailable; live updates not started"
                  : "Loading events"
                : pollError
                  ? "Live update failed; retrying"
                  : `Live; checks for new events every ${EVENT_POLL_MS / 1000}s`}
            {data && ` · ${feed.records.length} shown`}
          </p>
        </div>
        {data && (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-xs text-muted-foreground tabular-nums">
              {rangeCaption(data.range, shownOffset) ?? "All retained events"}
            </p>
            {live && data.range.from !== null && (
              // Relative presets stay anchored at load; this re-resolves them.
              <Button
                type="button"
                variant="ghost"
                size="xs"
                disabled={initial.isFetching}
                onClick={() => void initial.refetch()}
              >
                <RotateCcw aria-hidden="true" />
                Restart from now
              </Button>
            )}
          </div>
        )}
        {feed.skipped && (
          <p role="status" className="text-xs text-warning-strong">
            More events arrived than the list keeps at once, so it jumped to the
            newest {MAX_EVENT_ROWS}. Earlier events, including those in between,
            are available through Load older.
          </p>
        )}

        {feed.trimmedTop && (
          <div className="flex flex-wrap items-center gap-2">
            <p role="status" className="text-xs text-warning-strong">
              The list keeps {MAX_EVENT_ROWS} rows, so the newest were removed
              from the top while you browsed older events. New events wait
              behind the button above.
            </p>
            <Button
              type="button"
              variant="ghost"
              size="xs"
              disabled={initial.isFetching}
              onClick={() => {
                void initial.refetch();
                window.scrollTo({ top: 0 });
              }}
            >
              <ArrowUp aria-hidden="true" />
              Back to newest
            </Button>
          </div>
        )}

        {/* Always rendered with zero height so the affordance never moves rows. */}
        <div className="pointer-events-none sticky top-14 z-20 flex h-0 justify-center overflow-visible">
          {feed.pending.length > 0 && data && (
            <Button
              type="button"
              size="sm"
              className="pointer-events-auto"
              onClick={() => {
                update(data, showPending);
                window.scrollTo({ top: 0 });
              }}
            >
              <ArrowUp aria-hidden="true" />
              {feed.pending.length}
              {feed.pendingTruncated ? "+" : ""} new event
              {feed.pending.length === 1 && !feed.pendingTruncated ? "" : "s"}
            </Button>
          )}
        </div>

        {initial.error instanceof ApiError && initial.error.status === 403 ? (
          <EmptyState
            tone="danger"
            icon={<CircleAlert aria-hidden="true" />}
            title="These events need administrator access"
            description="Viewers can browse device events only."
            action={
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setFilters(defaultEventFilters)}
              >
                <RotateCcw aria-hidden="true" />
                Show device events
              </Button>
            }
          />
        ) : initial.error ? (
          // No automatic retry: the feed starts only from a successful page.
          <EmptyState
            tone="danger"
            icon={<CircleAlert aria-hidden="true" />}
            title={
              initial.error instanceof ApiError
                ? initial.error.title
                : "Loading events failed"
            }
            description="Events could not be loaded."
            action={
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={initial.isFetching}
                onClick={() => void initial.refetch()}
              >
                <RotateCcw aria-hidden="true" />
                Retry
              </Button>
            }
          />
        ) : (
          <DataState
            isLoading={initial.isLoading}
            error={null}
            isEmpty={feed.records.length === 0}
            emptyLabel="No events match"
            emptyDescription={
              hasActiveFilters
                ? "Try widening the time range or clearing filters."
                : "Events appear here as devices connect and operators act."
            }
            emptyAction={
              hasActiveFilters ? (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setFilters(defaultEventFilters)}
                >
                  <RotateCcw aria-hidden="true" />
                  Reset filters
                </Button>
              ) : undefined
            }
          >
            <DataTable
              label="Events"
              virtualized
              columns={columns}
              data={feed.records}
              getRowId={(record) => String(recordId(record))}
              rowClassName={(record) =>
                record.state === "unreadable" ? "text-muted-foreground" : ""
              }
            />
          </DataState>
        )}

        {data && feed.records.length > 0 && (
          <div className="flex flex-col items-center gap-2 py-2">
            {feed.older !== null ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={olderCurrent && older.isPending}
                onClick={() =>
                  feed.older !== null &&
                  older.mutate({ owner: data, before: feed.older })
                }
              >
                <ChevronDown aria-hidden="true" />
                Load older
              </Button>
            ) : (
              <p className="text-xs text-muted-foreground">
                No older matching events are retained.
              </p>
            )}
            {olderCurrent && older.isError && (
              <p role="alert" className="text-xs text-destructive">
                {older.error instanceof ApiError
                  ? older.error.title
                  : "Loading older events failed"}
              </p>
            )}
          </div>
        )}
      </section>
    </div>
  );
}

function eventColumns(
  operators: ReadonlyMap<string, string | null>,
  offset: number,
): DataTableColumnDef<JournalRecord>[] {
  return [
    {
      id: "time",
      header: `Time (${formatOffset(offset)})`,
      cell: ({ row }) => {
        const record = row.original;
        return record.state === "readable" ? (
          <span className="font-mono text-xs whitespace-nowrap tabular-nums">
            {formatTimestamp(record.header.occurred_at_ms, offset)}
          </span>
        ) : (
          <span className="text-xs">Unknown</span>
        );
      },
    },
    {
      id: "severity",
      header: "Severity",
      cell: ({ row }) => {
        const record = row.original;
        if (record.state === "unreadable")
          return (
            <Badge variant="muted">
              <FileQuestion aria-hidden="true" />
              Unknown
            </Badge>
          );
        const severity = record.header.severity;
        const style = severityBadges[severity] ?? severityBadges.info;
        const Icon = style.icon;
        return (
          <Badge variant={style.variant}>
            <Icon aria-hidden="true" />
            {labelFor(severityLabels, severity)}
          </Badge>
        );
      },
    },
    {
      id: "event",
      header: "Event",
      cell: ({ row }) => {
        const record = row.original;
        if (record.state === "unreadable")
          return (
            <div className="max-w-xl min-w-0 leading-tight">
              <p className="text-sm font-medium">Unreadable event</p>
              <p className="truncate text-xs">
                {labelFor(eventKindLabels, record.kind)} · Event #{record.id}
              </p>
            </div>
          );
        const summary = describeEvent(record.details);
        return (
          <div className="max-w-xl min-w-0 leading-tight">
            <p className="text-sm font-medium">
              {labelFor(eventKindLabels, record.details.kind)}
            </p>
            <p className="truncate text-xs text-muted-foreground">
              {[
                labelFor(categoryLabels, record.header.category),
                summary || null,
              ]
                .filter(Boolean)
                .join(" · ")}
            </p>
          </div>
        );
      },
    },
    {
      id: "device",
      header: "Device",
      cell: ({ row }) => {
        const record = row.original;
        if (record.state === "unreadable" || !record.header.device_id)
          return <span className="text-muted-foreground">—</span>;
        return (
          <div className="leading-tight">
            <span className="block font-mono text-sm font-semibold">
              {record.header.seat_code ?? "No seat"}
            </span>
            <code
              className="font-mono text-xs text-muted-foreground"
              title={record.header.device_id}
            >
              {shortId(record.header.device_id)}
            </code>
          </div>
        );
      },
    },
    {
      id: "operator",
      header: "Operator",
      cell: ({ row }) => {
        const record = row.original;
        const id =
          record.state === "readable" ? record.header.actor_operator_id : null;
        if (!id) return <span className="text-muted-foreground">—</span>;
        const name = operators.get(id);
        return name ? (
          <span className="text-sm" title={id}>
            {name}
          </span>
        ) : (
          <code className="font-mono text-xs" title={id}>
            {shortId(id)}
          </code>
        );
      },
    },
  ];
}

function EventFilterBar({
  filters,
  isAdmin,
  offset,
  onChange,
}: {
  filters: EventFilters;
  isAdmin: boolean;
  offset: number;
  onChange: (filters: EventFilters) => void;
}) {
  const presets = categoryPresets(isAdmin);
  const kinds = kindsFor(filters.preset);
  const kindOptions = kinds.map((value) => ({
    value,
    label: eventKindLabels[value],
  }));
  const deviceText = filters.deviceId ?? filters.seatPrefix ?? "";

  function applyDevice(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const text = new FormData(event.currentTarget).get("device");
    onChange({
      ...filters,
      ...deviceFilterFromText(typeof text === "string" ? text : ""),
    });
  }

  function setRange(range: TimeRange) {
    const now = Date.now();
    onChange({
      ...filters,
      range,
      // A new custom range starts from the last hour so both inputs are filled.
      from: range === "custom" ? (filters.from ?? now - 3_600_000) : null,
      to: range === "custom" ? (filters.to ?? now) : null,
    });
  }

  return (
    <div
      role="search"
      aria-label="Event filters"
      className="flex flex-wrap items-end gap-2"
    >
      {presets.length > 1 && (
        <ToggleGroup
          type="single"
          value={filters.preset}
          onValueChange={(value) =>
            value &&
            onChange({
              ...filters,
              preset: value as EventFilters["preset"],
              kinds: [],
            })
          }
          aria-label="Category"
          className="max-w-full overflow-x-auto"
        >
          {presets.map((preset) => (
            <ToggleGroupItem key={preset.value} value={preset.value}>
              {preset.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      )}
      <ToggleGroup
        type="single"
        value={filters.range}
        onValueChange={(value) => value && setRange(value as TimeRange)}
        aria-label="Time range"
        className="max-w-full overflow-x-auto"
      >
        {(Object.keys(timeRangeLabels) as TimeRange[]).map((range) => (
          <ToggleGroupItem key={range} value={range}>
            {timeRangeLabels[range]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {filters.range === "custom" && (
        <div className="flex flex-wrap items-end gap-2">
          <DateTimeField
            label={`From (${formatOffset(offset)})`}
            value={filters.from}
            offset={offset}
            onChange={(from) => onChange({ ...filters, from })}
          />
          <DateTimeField
            label={`To (${formatOffset(offset)})`}
            value={filters.to}
            offset={offset}
            onChange={(to) => onChange({ ...filters, to })}
          />
        </div>
      )}
      <form key={deviceText} onSubmit={applyDevice} className="w-full sm:w-72">
        <InputGroup
          type="search"
          name="device"
          aria-label="Seat prefix or device ID"
          placeholder="Seat prefix or device ID, then Enter"
          icon={<Search aria-hidden="true" />}
          defaultValue={deviceText}
          onBlur={(event) => event.currentTarget.form?.requestSubmit()}
        />
      </form>
      <MultiSelectFilter
        label="Kind"
        options={kindOptions}
        value={selection(kinds, filters.kinds)}
        onChange={(value) =>
          onChange({ ...filters, kinds: selected<EventKind>(kinds, value) })
        }
      />
      <MultiSelectFilter
        label="Severity"
        options={severityOptions}
        value={selection(severityOrder, filters.severities)}
        onChange={(value) =>
          onChange({
            ...filters,
            severities: selected<Severity>(severityOrder, value),
          })
        }
      />
      {isAdmin && (
        <OperatorFilter
          value={filters.operatorId}
          onChange={(operatorId) => onChange({ ...filters, operatorId })}
        />
      )}
      {serializeEventFilters(filters).toString() !== "" && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => onChange(defaultEventFilters)}
        >
          <RotateCcw aria-hidden="true" />
          Reset
        </Button>
      )}
    </div>
  );
}

function DateTimeField({
  label,
  value,
  offset,
  onChange,
}: {
  label: string;
  value: number | null;
  offset: number;
  onChange: (value: number | null) => void;
}) {
  const id = `events-${label.split(" ")[0].toLowerCase()}`;
  return (
    <div className="grid gap-1">
      <Label htmlFor={id}>{label}</Label>
      <Input
        id={id}
        type="datetime-local"
        className="h-8 w-52"
        value={value === null ? "" : toDateTimeLocal(value, offset)}
        onChange={(event) =>
          onChange(fromDateTimeLocal(event.target.value, offset))
        }
      />
    </div>
  );
}

/** Administrator-only actor filter backed by the operator account list. */
function OperatorFilter({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (value: string | null) => void;
}) {
  const { api } = useSessionScope();
  const accounts = useQuery({
    queryKey: ["operator-accounts"],
    queryFn: async ({ signal }) =>
      unwrap<OperatorAccountResponse[]>(
        await api.listOperatorAccounts({ signal }),
      ),
  });
  const current = accounts.data?.find(
    (account) => account.operator_id === value,
  );
  const summary =
    value === null ? "Any" : (current?.username ?? shortId(value));
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className={cn(
            "max-w-full min-w-0 justify-between",
            value !== null &&
              "border-primary bg-primary text-primary-foreground hover:bg-primary-hover dark:border-primary dark:text-primary-foreground",
          )}
          aria-label={`Operator: ${summary}`}
        >
          <UserRound aria-hidden="true" />
          <span className="shrink-0">Operator</span>
          {value !== null && (
            <>
              <span
                aria-hidden="true"
                className="h-3.5 w-px bg-current opacity-40"
              />
              <span className="max-w-40 truncate font-normal">{summary}</span>
            </>
          )}
          <ChevronDown aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" aria-label="Operator" className="w-60">
        <DropdownMenuLabel>Operator</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={value ?? ""}
          onValueChange={(next) => onChange(next === "" ? null : next)}
        >
          <DropdownMenuRadioItem value="">Any operator</DropdownMenuRadioItem>
          {accounts.data?.map((account) => (
            <DropdownMenuRadioItem
              key={account.operator_id}
              value={account.operator_id}
            >
              {account.username}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        {accounts.isError && (
          <p className="px-4 py-2 text-xs text-destructive">
            Operator list unavailable
          </p>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
