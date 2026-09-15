import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { Badge } from "@/components/ui/badge";

import { TeamName, TeamSchool, type Team } from "@/components/roster-team";

type Account = components["schemas"]["AccountResponse"];
type Binding = components["schemas"]["BindingResponse"];
type SeatFacts = components["schemas"]["SeatResponse"];
type Seat = SeatFacts & {
  team?: Team | null;
  bound: boolean;
};

const columns: ColumnDef<Seat>[] = [
  { accessorKey: "seat_code", header: "Seat code", enableSorting: true },
  {
    accessorKey: "bound",
    header: "Binding",
    cell: ({ row }) =>
      row.original.bound ? (
        <Badge variant="secondary">Bound</Badge>
      ) : (
        <Badge variant="outline">Unbound</Badge>
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
  { accessorKey: "seat_id", header: "Seat ID" },
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
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          {visibleRows.length} of {seats.data?.length ?? 0} seats
        </p>
        <fieldset className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
          <legend className="font-medium">Binding state</legend>
          {(["bound", "unbound"] as const).map((state) => (
            <label key={state} className="flex items-center gap-2 capitalize">
              <input
                type="checkbox"
                checked={bindingFilter[state]}
                onChange={(event) =>
                  setBindingFilter({
                    ...bindingFilter,
                    [state]: event.target.checked,
                  })
                }
              />
              {state}
            </label>
          ))}
        </fieldset>
        <DataTable
          columns={columns}
          data={visibleRows}
          rowClassName={(seat) =>
            seat.bound
              ? "bg-emerald-50/50 dark:bg-emerald-950/20"
              : "bg-amber-50/70 dark:bg-amber-950/30"
          }
        />
      </div>
    </DataState>
  );
}
