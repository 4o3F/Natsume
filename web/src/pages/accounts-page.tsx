import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { Input } from "@/components/ui/input";

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
  const [filters, setFilters] = useState({
    username: "",
    seat: "",
    team: "",
    school: "",
  });
  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: async () => unwrap<Account[]>(await api.GET("/api/v2/accounts")),
    refetchInterval: LIST_POLL_MS,
  });
  const rows = useMemo(() => {
    const normalized = Object.fromEntries(
      Object.entries(filters).map(([key, value]) => [
        key,
        value.trim().toLocaleLowerCase(),
      ]),
    ) as typeof filters;
    const contains = (value: string | undefined, query: string) =>
      !query || value?.toLocaleLowerCase().includes(query);
    return (accounts.data ?? []).filter(
      (account) =>
        contains(account.domjudge_username, normalized.username) &&
        contains(account.team?.seat_code, normalized.seat) &&
        [account.team?.team_name_zh, account.team?.team_name_en].some((value) =>
          contains(value, normalized.team),
        ) &&
        [account.team?.school_name_zh, account.team?.school_name_en].some(
          (value) => contains(value, normalized.school),
        ),
    );
  }, [accounts.data, filters]);

  return (
    <DataState
      isLoading={accounts.isLoading}
      error={accounts.data ? null : accounts.error}
      isEmpty={!accounts.data?.length}
      emptyLabel="No accounts found."
    >
      <div className="space-y-3">
        <p className="text-sm text-muted-foreground">
          {rows.length} of {accounts.data?.length ?? 0} accounts
        </p>
        <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
          {(
            [
              ["username", "Username"],
              ["seat", "Seat"],
              ["team", "Team name"],
              ["school", "School"],
            ] as const
          ).map(([key, label]) => (
            <Input
              key={key}
              value={filters[key]}
              onChange={(event) =>
                setFilters({ ...filters, [key]: event.target.value })
              }
              aria-label={`Filter by ${label}`}
              placeholder={`Filter by ${label}`}
            />
          ))}
        </div>
        <DataTable columns={columns} data={rows} />
      </div>
    </DataState>
  );
}
