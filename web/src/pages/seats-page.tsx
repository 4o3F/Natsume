import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router";
import {
  LayoutGrid,
  Link as LinkIcon,
  List,
  Search,
  Unlink,
  Users,
} from "lucide-react";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type {
  AccountResponse,
  BindingResponse,
  SeatResponse,
} from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import {
  CheckboxFilter,
  CheckboxFilterOption,
} from "@/components/checkbox-filter";
import { RefreshCountdown, StatusIcon } from "@/components/device-status";
import { tones } from "@/components/device-status-style";
import { PageHeader } from "@/components/page-header";
import { StatGrid, StatTile } from "@/components/stat-tile";
import { InputGroup } from "@/components/ui/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { TeamName, TeamSchool, type Team } from "@/components/roster-team";
import { cn } from "@/lib/utils";

type Account = AccountResponse;
type Binding = BindingResponse;
type SeatFacts = SeatResponse;
type Seat = SeatFacts & {
  team?: Team | null;
  bound: boolean;
};

const columns: DataTableColumnDef<Seat>[] = [
  {
    accessorKey: "seat_code",
    header: "Seat",
    enableSorting: true,
    cell: ({ row }) => (
      <span className="font-mono text-sm font-semibold">
        {row.original.seat_code}
      </span>
    ),
  },
  {
    accessorKey: "bound",
    header: "Binding",
    cell: ({ row }) => (
      <span className="flex items-center gap-1.5">
        <StatusIcon
          icon={row.original.bound ? "linked" : "unlinked"}
          tone={row.original.bound ? tones.success.text : tones.warning.text}
          label={row.original.bound ? "Binding: bound" : "Binding: unbound"}
          className="size-4"
        />
        {row.original.bound ? "Bound" : "Unbound"}
      </span>
    ),
  },
  {
    id: "team",
    header: "Team",
    cell: ({ row }) => <TeamName team={row.original.team} />,
  },
  {
    id: "school",
    header: "School",
    cell: ({ row }) => <TeamSchool team={row.original.team} />,
  },
  {
    accessorKey: "seat_id",
    header: "Seat ID",
    cell: ({ row }) => (
      <code
        className="font-mono text-xs text-muted-foreground"
        title={row.original.seat_id}
      >
        {row.original.seat_id.length > 8
          ? `${row.original.seat_id.slice(0, 8)}…`
          : row.original.seat_id}
      </code>
    ),
  },
];

function rowOf(seatCode: string) {
  return /^[A-Za-z]+/.exec(seatCode)?.[0].toUpperCase() ?? "#";
}

export function SeatsPage() {
  const { api } = useSessionScope();
  const [view, setView] = useState<"map" | "list">("map");
  const [search, setSearch] = useState("");
  const [bindingFilter, setBindingFilter] = useState({
    bound: true,
    unbound: true,
  });
  const seats = useQuery({
    queryKey: ["seats"],
    queryFn: async () => unwrap<SeatFacts[]>(await api.listSeats()),
    refetchInterval: LIST_POLL_MS,
  });

  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: async () => unwrap<Account[]>(await api.listAccounts()),
    refetchInterval: LIST_POLL_MS,
  });
  const bindings = useQuery({
    queryKey: ["bindings"],
    queryFn: async () => unwrap<Binding[]>(await api.listBindings()),
    refetchInterval: LIST_POLL_MS,
  });
  const rows = useMemo(() => {
    const teams = new Map(
      accounts.data?.flatMap((account) =>
        account.team ? [[account.team.seat_id, account.team] as const] : [],
      ),
    );
    const boundSeats = new Set(
      bindings.data?.map((binding) => binding.seat_id),
    );
    return (
      seats.data?.map((seat) => ({
        ...seat,
        team: teams.get(seat.seat_id),
        bound: boundSeats.has(seat.seat_id),
      })) ?? []
    );
  }, [seats.data, accounts.data, bindings.data]);
  const query = search.trim().toLowerCase();
  const visibleRows = rows.filter(
    (seat) =>
      (seat.bound ? bindingFilter.bound : bindingFilter.unbound) &&
      (!query ||
        [
          seat.seat_code,
          seat.team?.team_name_zh,
          seat.team?.team_name_en,
          seat.team?.school_name_zh,
          seat.team?.school_name_en,
        ].some((value) => value?.toLowerCase().includes(query))),
  );
  const groups = useMemo(() => {
    const map = new Map<string, Seat[]>();
    for (const seat of [...visibleRows].sort((a, b) =>
      a.seat_code.localeCompare(b.seat_code, undefined, { numeric: true }),
    )) {
      const key = rowOf(seat.seat_code);
      map.set(key, [...(map.get(key) ?? []), seat]);
    }
    return [...map.entries()];
  }, [visibleRows]);
  const queries = [seats, accounts, bindings];
  const boundCount = rows.filter((seat) => seat.bound).length;
  const withTeam = rows.filter((seat) => seat.team).length;
  const coverage = rows.length
    ? Math.round((boundCount / rows.length) * 100)
    : 0;
  const unboundOnly = !bindingFilter.bound && bindingFilter.unbound;
  const boundOnly = bindingFilter.bound && !bindingFilter.unbound;

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Seats"
        description="Seat map, team assignments, and seats that still need a device."
        actions={
          <RefreshCountdown
            resource="seat"
            updatedAt={Math.min(
              ...queries.map((query) =>
                Math.max(query.dataUpdatedAt, query.errorUpdatedAt),
              ),
            )}
            isFetching={queries.some((query) => query.isFetching)}
            isPaused={queries.some((query) => query.isPaused)}
            isError={queries.some((query) => query.isError)}
            onRefresh={() => queries.forEach((query) => void query.refetch())}
          />
        }
      />

      <StatGrid>
        <StatTile
          label="Seats"
          value={rows.length}
          icon={<LayoutGrid aria-hidden="true" />}
          hint={`${groups.length} rows shown`}
        />
        <StatTile
          label="Bound"
          value={boundCount}
          tone="success"
          icon={<LinkIcon aria-hidden="true" />}
          active={boundOnly}
          onClick={() =>
            setBindingFilter(
              boundOnly
                ? { bound: true, unbound: true }
                : { bound: true, unbound: false },
            )
          }
        />
        <StatTile
          label="Need a device"
          value={rows.length - boundCount}
          tone={rows.length - boundCount ? "warning" : "muted"}
          icon={<Unlink aria-hidden="true" />}
          active={unboundOnly}
          onClick={() =>
            setBindingFilter(
              unboundOnly
                ? { bound: true, unbound: true }
                : { bound: false, unbound: true },
            )
          }
        />
        <StatTile
          label="With roster team"
          value={withTeam}
          icon={<Users aria-hidden="true" />}
          hint={`of ${rows.length}`}
        />
        <div className="col-span-2 flex flex-col justify-center gap-3 border bg-card px-4 py-4">
          <div className="flex items-baseline justify-between">
            <span className="text-caption text-muted-foreground">
              Binding coverage
            </span>
            <span className="text-headline font-light tabular-nums">
              {coverage}%
            </span>
          </div>
          <div
            role="progressbar"
            aria-label="Binding coverage"
            aria-valuenow={coverage}
            aria-valuemin={0}
            aria-valuemax={100}
            className="h-1 overflow-hidden bg-surface-2"
          >
            <div
              className="h-full bg-success transition-[width] duration-500"
              style={{ width: `${coverage}%` }}
            />
          </div>
        </div>
      </StatGrid>

      <section aria-label="Seat list" className="space-y-3">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <InputGroup
            type="search"
            aria-label="Search seats"
            placeholder="Seat, team, or school"
            icon={<Search aria-hidden="true" />}
            className="w-full sm:w-64"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <CheckboxFilter label="Binding">
            {(["bound", "unbound"] as const).map((state) => (
              <CheckboxFilterOption
                key={state}
                label={state === "bound" ? "Bound" : "Unbound"}
                count={
                  state === "bound" ? boundCount : rows.length - boundCount
                }
                checked={bindingFilter[state]}
                onCheckedChange={(checked) =>
                  setBindingFilter({ ...bindingFilter, [state]: checked })
                }
              />
            ))}
          </CheckboxFilter>
          <div className="ml-auto flex items-center gap-3">
            {seats.data && bindings.data && (
              <p
                role="status"
                className="text-xs text-muted-foreground tabular-nums"
              >
                {visibleRows.length} of {seats.data.length} seats
              </p>
            )}
            <ToggleGroup
              type="single"
              value={view}
              onValueChange={(value) =>
                value && setView(value as "map" | "list")
              }
              aria-label="View"
            >
              <ToggleGroupItem value="map" aria-label="Map view">
                <LayoutGrid aria-hidden="true" />
                Map
              </ToggleGroupItem>
              <ToggleGroupItem value="list" aria-label="List view">
                <List aria-hidden="true" />
                List
              </ToggleGroupItem>
            </ToggleGroup>
          </div>
        </div>
        <DataState
          isLoading={
            seats.isLoading || accounts.isLoading || bindings.isLoading
          }
          error={
            (seats.data ? null : seats.error) ??
            (accounts.data ? null : accounts.error) ??
            (bindings.data ? null : bindings.error)
          }
          isEmpty={!visibleRows.length}
          emptyLabel="No seats match"
          emptyDescription="Try a different search or binding filter."
        >
          {view === "map" ? (
            <SeatMap groups={groups} />
          ) : (
            <DataTable
              label="Seats"
              columns={columns}
              data={visibleRows}
              getRowId={(seat) => seat.seat_id}
              rowClassName={(seat) =>
                seat.bound ? "" : "bg-warning-subtle/50 hover:bg-warning-subtle"
              }
            />
          )}
        </DataState>
      </section>
    </div>
  );
}

function SeatMap({ groups }: { groups: [string, Seat[]][] }) {
  return (
    <div className="space-y-4 border bg-card p-4 sm:p-6">
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>Select a seat to open its device.</span>
        <span className="flex items-center gap-4">
          <span className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-sm border border-success/40 bg-success-subtle" />
            Bound
          </span>
          <span className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-sm border border-dashed border-warning/60 bg-warning-subtle" />
            Needs a device
          </span>
        </span>
      </div>
      {groups.map(([row, seats]) => (
        <div
          key={row}
          role="group"
          aria-label={`Row ${row}`}
          className="flex gap-3"
        >
          <span className="flex w-6 shrink-0 items-center justify-center font-mono text-sm font-semibold text-muted-foreground">
            {row}
          </span>
          <div className="grid flex-1 grid-cols-[repeat(auto-fill,minmax(8.5rem,1fr))] gap-2">
            {seats.map((seat) => (
              <SeatTile key={seat.seat_id} seat={seat} />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function SeatTile({ seat }: { seat: Seat }) {
  const team = seat.team?.team_name_zh || seat.team?.team_name_en;
  const school = seat.team?.school_name_zh || seat.team?.school_name_en;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Link
          to={`/devices?q=${encodeURIComponent(seat.seat_code)}`}
          aria-label={`${seat.seat_code}, ${seat.bound ? "bound" : "unbound"}${team ? `, ${team}` : ""}`}
          className={cn(
            "group flex min-w-0 flex-col gap-0.5 rounded-lg border px-2.5 py-2 transition-colors outline-none hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
            seat.bound
              ? "border-success/30 bg-success-subtle/60 hover:border-success/60"
              : "border-dashed border-warning/50 bg-warning-subtle/60 hover:border-warning",
          )}
        >
          <span className="flex items-center justify-between gap-1">
            <span className="font-mono text-xs font-semibold">
              {seat.seat_code}
            </span>
            {seat.bound ? (
              <LinkIcon
                aria-hidden="true"
                className="size-3 text-success-strong"
              />
            ) : (
              <Unlink
                aria-hidden="true"
                className="size-3 text-warning-strong"
              />
            )}
          </span>
          <span
            className={cn(
              "truncate text-xs",
              team ? "text-foreground" : "text-muted-foreground",
            )}
          >
            {team ?? "No team"}
          </span>
        </Link>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-64">
        <p className="font-medium">{team ?? "No roster profile"}</p>
        {school && <p className="opacity-70">{school}</p>}
        <p className="mt-1 opacity-70">
          {seat.bound ? "Device bound" : "No device bound"}
        </p>
      </TooltipContent>
    </Tooltip>
  );
}
