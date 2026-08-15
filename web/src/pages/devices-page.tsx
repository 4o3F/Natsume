import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";

type Device = components["schemas"]["DeviceResponse"];

const columns: ColumnDef<Device>[] = [
  { accessorKey: "device_id", header: "Device ID" },
  { accessorKey: "state", header: "State" },
  {
    accessorKey: "hardware_identity_quality",
    header: "Hardware identity quality",
  },
];

export function DevicesPage() {
  const devices = useQuery({
    queryKey: ["devices"],
    queryFn: async () => unwrap<Device[]>(await api.GET("/api/v2/devices")),
    refetchInterval: LIST_POLL_MS,
  });

  return (
    <DataState
      isLoading={devices.isLoading}
      error={devices.data ? null : devices.error}
      isEmpty={!devices.data?.length}
      emptyLabel="No devices found."
    >
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          {devices.data?.length} devices
        </p>
        <DataTable columns={columns} data={devices.data ?? []} />
      </div>
    </DataState>
  );
}
