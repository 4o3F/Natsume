import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";

type Binding = components["schemas"]["BindingResponse"];

const columns: ColumnDef<Binding>[] = [
  { accessorKey: "seat_id", header: "Seat ID" },
  { accessorKey: "device_id", header: "Device ID" },
  { accessorKey: "binding_revision", header: "Binding revision" },
];

export function BindingsPage() {
  const bindings = useQuery({
    queryKey: ["bindings"],
    queryFn: async () => unwrap<Binding[]>(await api.GET("/api/v2/bindings")),
    refetchInterval: LIST_POLL_MS,
  });

  return (
    <DataState
      isLoading={bindings.isLoading}
      error={bindings.data ? null : bindings.error}
      isEmpty={!bindings.data?.length}
      emptyLabel="No bindings found."
    >
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          {bindings.data?.length} bindings
        </p>
        <DataTable columns={columns} data={bindings.data ?? []} />
      </div>
    </DataState>
  );
}
