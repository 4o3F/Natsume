import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import {
  CheckboxFilter,
  CheckboxFilterOption,
} from "@/components/checkbox-filter";
import { StatusIcon } from "@/components/device-status";

import { TeamName, TeamSchool, type Team } from "@/components/roster-team";

type Account = components["schemas"]["AccountResponse"];
type Binding = components["schemas"]["BindingResponse"];
type SeatFacts = components["schemas"]["SeatResponse"];
type Seat = SeatFacts & {
  team?: Team | null;
  bound: boolean;
};

const columns: ColumnDef<Seat>[] = [
  {
    accessorKey: "seat_code",
    header: "Seat code",
    enableSorting: true,
    cell: ({ row }) => (
      <span className="font-medium tabular-nums">{row.original.seat_code}</span>
    ),
  },
  {
    accessorKey: "bound",
    header: "Binding",
    cell: ({ row }) => (
      <span className="flex items-center gap-2">
        <StatusIcon
          icon={row.original.bound ? "linked" : "unlinked"}
          tone={row.original.bound ? "text-emerald-700" : "text-amber-700"}
          label={row.original.bound ? "Binding: bound" : "Binding: unbound"}
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
    header: "School / Logo",
    cell: ({ row }) => <TeamSchool team={row.original.team} />,
  },
  {
    accessorKey: "seat_id",
    header: "Seat ID",
    cell: ({ row }) => (
      <code
        className="text-xs text-muted-foreground"
        title={row.original.seat_id}
      >
        {row.original.seat_id.length > 8
          ? `${row.original.seat_id.slice(0, 8)}…`
          : row.original.seat_id}
      </code>
    ),
  },
];

export function SeatsPage() {
  const { api } = useSessionScope();
  const [bindingFilter, setBindingFilter] = useState({
    bound: true,
    unbound: true,
  });
  const seats = useQuery({
    queryKey: ["seats"],
    queryFn: async () => unwrap<SeatFacts[]>(await api.GET("/api/v2/seats")),
    refetchInterval: LIST_POLL_MS,
  });

  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: async () => unwrap<Account[]>(await api.GET("/api/v2/accounts")),
    refetchInterval: LIST_POLL_MS,
  });
  const bindings = useQuery({
    queryKey: ["bindings"],
    queryFn: async () => unwrap<Binding[]>(await api.GET("/api/v2/bindings")),
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
  const visibleRows = rows.filter((seat) =>
    seat.bound ? bindingFilter.bound : bindingFilter.unbound,
  );

  return (
    <div className="min-w-0 space-y-4">
      <div className="space-y-1">
        <h1 className="text-2xl font-semibold tracking-tight">Seats</h1>
        <p className="text-sm text-muted-foreground">
          Review seat assignments and find seats that still need a device.
        </p>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <CheckboxFilter label="Binding state">
          {(["bound", "unbound"] as const).map((state) => (
            <CheckboxFilterOption
              key={state}
              label={state === "bound" ? "Bound" : "Unbound"}
              checked={bindingFilter[state]}
              onCheckedChange={(checked) =>
                setBindingFilter({ ...bindingFilter, [state]: checked })
              }
            />
          ))}
        </CheckboxFilter>
        {seats.data && bindings.data && (
          <p className="text-sm text-muted-foreground">
            {visibleRows.length} of {seats.data.length} seats
          </p>
        )}
      </div>
      <DataState
        isLoading={seats.isLoading || accounts.isLoading || bindings.isLoading}
        error={
          (seats.data ? null : seats.error) ??
          (accounts.data ? null : accounts.error) ??
          (bindings.data ? null : bindings.error)
        }
        isEmpty={!visibleRows.length}
        emptyLabel="No seats match the binding filter."
      >
        <DataTable
          columns={columns}
          data={visibleRows}
          getRowId={(seat) => seat.seat_id}
          rowClassName={(seat) =>
            seat.bound
              ? "bg-emerald-500/5 hover:bg-emerald-500/10"
              : "bg-amber-500/10 hover:bg-amber-500/15"
          }
        />
      </DataState>
    </div>
  );
}
