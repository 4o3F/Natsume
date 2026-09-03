import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
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

type Binding = components["schemas"]["BindingResponse"];

export function BindingsPage() {
  const session = useSession().data;
  const queryClient = useQueryClient();
  const bindings = useQuery({
    queryKey: ["bindings"],
    queryFn: async () => unwrap<Binding[]>(await api.GET("/api/v2/bindings")),
    refetchInterval: LIST_POLL_MS,
  });
  const unbind = useMutation({
    mutationFn: async (deviceId: string) =>
      unwrap<void>(
        await api.DELETE("/api/v2/devices/{device_id}/binding", {
          params: { path: { device_id: deviceId } },
        }),
      ),
    onSuccess: async (_, deviceId) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["bindings"] }),
        queryClient.invalidateQueries({
          queryKey: ["device-convergence", deviceId],
        }),
      ]);
    },
  });

  const columns: ColumnDef<Binding>[] = [
    { accessorKey: "seat_id", header: "Seat ID" },
    { accessorKey: "device_id", header: "Device ID" },
    { accessorKey: "binding_id", header: "Binding ID" },
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
              <AlertDialogTitle>Unbind this device?</AlertDialogTitle>
              <AlertDialogDescription>
                The device will return to binding negotiation.
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
    </div>
  );
}
