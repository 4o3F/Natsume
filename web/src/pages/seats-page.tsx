import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";

import { TeamName, TeamSchool, type Team } from "@/components/roster-team";

type Account = components["schemas"]["AccountResponse"];
type Seat = components["schemas"]["SeatResponse"] & { team?: Team | null };

const columns: ColumnDef<Seat>[] = [
  { accessorKey: "seat_code", header: "Seat code", enableSorting: true },
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
  const seats = useQuery({
    queryKey: ["seats"],
    queryFn: async () => unwrap<Seat[]>(await api.GET("/api/v2/seats")),
    refetchInterval: LIST_POLL_MS,
  });

  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: async () => unwrap<Account[]>(await api.GET("/api/v2/accounts")),
    refetchInterval: LIST_POLL_MS,
  });
  const rows = useMemo(() => {
    const teams = new Map(
      accounts.data?.flatMap((account) =>
        account.team ? [[account.team.seat_id, account.team] as const] : [],
      ),
    );
    return (
      seats.data?.map((seat) => ({ ...seat, team: teams.get(seat.seat_id) })) ??
      []
    );
  }, [seats.data, accounts.data]);

  return (
    <DataState
      isLoading={seats.isLoading || accounts.isLoading}
      error={
        (seats.data ? null : seats.error) ??
        (accounts.data ? null : accounts.error)
      }
      isEmpty={!seats.data?.length}
      emptyLabel="No seats found."
    >
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          {seats.data?.length} seats
        </p>
        <DataTable columns={columns} data={rows} />
      </div>
    </DataState>
  );
}
