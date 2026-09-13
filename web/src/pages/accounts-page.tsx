import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";

import { TeamName, TeamSchool } from "@/components/roster-team";

type Account = components["schemas"]["AccountResponse"];

const columns: ColumnDef<Account>[] = [
  { accessorKey: "domjudge_username", header: "DOMjudge username" },
  { accessorKey: "team.seat_code", header: "Seat", enableSorting: true },
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
  { accessorKey: "credential_revision", header: "Credential revision" },
  { accessorKey: "account_id", header: "Account ID" },
];

export function AccountsPage() {
  const { api } = useSessionScope();
  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: async () => unwrap<Account[]>(await api.GET("/api/v2/accounts")),
    refetchInterval: LIST_POLL_MS,
  });

  return (
    <DataState
      isLoading={accounts.isLoading}
      error={accounts.data ? null : accounts.error}
      isEmpty={!accounts.data?.length}
      emptyLabel="No accounts found."
    >
      <div className="space-y-2">
        <p className="text-sm text-muted-foreground">
          {accounts.data?.length} accounts
        </p>
        <DataTable columns={columns} data={accounts.data ?? []} />
      </div>
    </DataState>
  );
}
