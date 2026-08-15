import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";

type Seat = components["schemas"]["SeatResponse"];

const columns: ColumnDef<Seat>[] = [
  { accessorKey: "seat_code", header: "Seat code" },
  { accessorKey: "seat_id", header: "Seat ID" },
];

export function SeatsPage() {
  const seats = useQuery({
    queryKey: ["seats"],
    queryFn: async () => unwrap<Seat[]>(await api.GET("/api/v2/seats")),
    refetchInterval: LIST_POLL_MS,
  });

  return (
    <DataState
      isLoading={seats.isLoading}
      error={seats.data ? null : seats.error}
      isEmpty={!seats.data?.length}
      emptyLabel="No seats found."
    >
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          {seats.data?.length} seats
        </p>
        <DataTable columns={columns} data={seats.data ?? []} />
      </div>
    </DataState>
  );
}
