import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { Alert, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

type Binding = components["schemas"]["BindingResponse"];
type Seat = components["schemas"]["SeatResponse"];
type BindingRow = Binding & { seat_code: string | null };

export function BindingsPage() {
  const { api } = useSessionScope();
  const session = useSession().data;
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const bindings = useQuery({
    queryKey: ["bindings"],
    queryFn: async () => unwrap<Binding[]>(await api.GET("/api/v2/bindings")),
    refetchInterval: LIST_POLL_MS,
  });
  const seats = useQuery({
    queryKey: ["seats"],
    queryFn: async () => unwrap<Seat[]>(await api.GET("/api/v2/seats")),
    refetchInterval: LIST_POLL_MS,
  });
  const rows = useMemo(() => {
    const seatCodes = new Map(
      seats.data?.map((seat) => [seat.seat_id, seat.seat_code]),
    );
    const query = search.trim().toLowerCase();
    return (bindings.data ?? [])
      .map((binding) => ({
        ...binding,
        seat_code: seatCodes.get(binding.seat_id) ?? null,
      }))
      .filter(
        (binding) => !query || binding.seat_code?.toLowerCase().includes(query),
      )
      .sort((a, b) =>
        (a.seat_code ?? "").localeCompare(b.seat_code ?? "", undefined, {
          numeric: true,
        }),
      );
  }, [bindings.data, seats.data, search]);
  const unbind = useMutation({
    mutationFn: async (deviceId: string) =>
      unwrap<void>(
        await api.DELETE("/api/v2/devices/{device_id}/binding", {
          params: { path: { device_id: deviceId } },
        }),
      ),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["bindings"] }),
        queryClient.invalidateQueries({ queryKey: ["devices"] }),
      ]);
    },
  });

  const columns: ColumnDef<BindingRow>[] = [
    {
      accessorKey: "seat_code",
      header: "Seat",
      enableSorting: true,
      sortingFn: "alphanumeric",
      cell: ({ row }) => (
        <span className="font-semibold" title={row.original.seat_id}>
          {row.original.seat_code ?? "Unknown seat"}
        </span>
      ),
    },
    {
      accessorKey: "device_id",
      header: "Device",
      cell: ({ row }) => (
        <code
          className="text-xs text-muted-foreground"
          title={row.original.device_id}
        >
          {row.original.device_id}
        </code>
      ),
    },
  ];
  if (session?.role === "admin") {
    columns.push({
      id: "actions",
      header: "Actions",
      cell: ({ row }) => (
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={unbind.isPending}
            >
              Unbind
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {row.original.seat_code
                  ? `Unbind seat ${row.original.seat_code}?`
                  : "Unbind this device?"}
              </AlertDialogTitle>
              <AlertDialogDescription>
                The device will return to seat selection.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction
                className="bg-destructive text-white hover:bg-destructive/90"
                onClick={() => unbind.mutate(row.original.device_id)}
              >
                Unbind
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      ),
    });
  }

  return (
    <div className="space-y-4">
      <div className="space-y-1">
        <h1 className="text-2xl font-semibold tracking-tight">Bindings</h1>
        <p className="text-sm text-muted-foreground">
          Find a seat to review or remove its device binding.
        </p>
      </div>
      {unbind.error && (
        <Alert variant="destructive">
          <AlertTitle>
            {unbind.error instanceof ApiError
              ? unbind.error.title
              : "Unbind failed"}
          </AlertTitle>
        </Alert>
      )}
      <DataState
        isLoading={bindings.isLoading || seats.isLoading}
        error={
          (bindings.data ? null : bindings.error) ??
          (seats.data ? null : seats.error)
        }
        isEmpty={!bindings.data?.length}
        emptyLabel="No bindings found."
      >
        <div className="space-y-2">
          <div className="flex items-center justify-between gap-4">
            <Input
              type="search"
              aria-label="Search seats"
              placeholder="Find a seat, e.g. A-01"
              className="max-w-xs"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
            <p className="shrink-0 text-sm text-muted-foreground">
              {rows.length} of {bindings.data?.length} bindings
            </p>
          </div>
          <DataTable
            columns={columns}
            data={rows}
            getRowId={(row) => row.binding_id}
          />
        </div>
      </DataState>
    </div>
  );
}
