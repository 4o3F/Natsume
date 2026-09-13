import type { ColumnDef } from "@tanstack/react-table";

import type { components } from "@/api/generated/schema";
import { DataTable } from "@/components/data-table";
import { Badge } from "@/components/ui/badge";

type Diff = components["schemas"]["ImportRedactedDiff"];
type Organization = components["schemas"]["ImportOrganizationResponse"];
type OrganizationChange =
  components["schemas"]["ImportOrganizationChangeResponse"];
type Team = components["schemas"]["ImportTeamResponse"];
type TeamChange = components["schemas"]["ImportTeamChangeResponse"];

function Names({ value }: { value: { name_zh: string; name_en: string } }) {
  return (
    <div className="min-w-32 max-w-sm whitespace-normal break-words">
      <div>{value.name_zh || value.name_en}</div>
      {value.name_zh && value.name_en && (
        <div className="text-sm text-muted-foreground">{value.name_en}</div>
      )}
    </div>
  );
}

function SchoolDetails({ value }: { value: Organization | null }) {
  return value ? (
    <div className="space-y-1">
      <Names value={value} />
      <div className="text-sm text-muted-foreground">
        {value.organization_id} · {value.country}
      </div>
    </div>
  ) : (
    <span className="text-muted-foreground">—</span>
  );
}

const schoolChangeColumns: ColumnDef<OrganizationChange>[] = [
  {
    id: "change",
    header: "Change",
    cell: ({ row }) =>
      !row.original.current
        ? "Added"
        : !row.original.candidate
          ? "Removed"
          : "Updated",
  },
  {
    id: "current",
    header: "Current school",
    cell: ({ row }) => <SchoolDetails value={row.original.current} />,
  },
  {
    id: "candidate",
    header: "After import",
    cell: ({ row }) => <SchoolDetails value={row.original.candidate} />,
  },
];

function TeamDetails({ value }: { value: Team | null }) {
  return value ? (
    <div className="space-y-1">
      <Names value={value} />
      <div className="text-sm text-muted-foreground">
        Seat {value.seat ?? "Unmapped"} · {value.organization_id} ·{" "}
        {value.category}
      </div>
    </div>
  ) : (
    <span className="text-muted-foreground">—</span>
  );
}

const teamChangeColumns: ColumnDef<TeamChange>[] = [
  { accessorKey: "account", header: "Account", enableSorting: true },
  {
    id: "current",
    header: "Current team",
    cell: ({ row }) => <TeamDetails value={row.original.current} />,
  },
  {
    id: "candidate",
    header: "After import",
    cell: ({ row }) => <TeamDetails value={row.original.candidate} />,
  },
];

export function RosterDiff({ diff }: { diff: Diff }) {
  return (
    <>
      <div className="grid gap-4 lg:grid-cols-3">
        {[
          { title: "Accounts added", accounts: diff.accounts_added },
          { title: "Accounts removed", accounts: diff.accounts_removed },
          { title: "Passwords changed", accounts: diff.passwords_changed },
        ].map(({ title, accounts }) => (
          <section
            key={title}
            aria-label={title}
            className="min-w-0 space-y-2 rounded-lg border p-4"
          >
            <h2 className="font-medium">
              {title} ({accounts.length})
            </h2>
            <div className="flex max-h-48 flex-wrap gap-2 overflow-auto">
              {accounts.length ? (
                accounts.map((account) => (
                  <Badge key={account} variant="secondary">
                    {account}
                  </Badge>
                ))
              ) : (
                <span className="text-sm text-muted-foreground">None</span>
              )}
            </div>
          </section>
        ))}
      </div>
      <section aria-label="Team changes" className="space-y-2">
        <h2 className="font-medium">
          Team changes ({diff.team_changes.length})
        </h2>
        <div className="max-h-96 overflow-auto">
          <DataTable
            columns={teamChangeColumns}
            data={diff.team_changes}
            getRowId={(row) => row.account}
          />
        </div>
      </section>
      <section aria-label="School changes" className="space-y-2">
        <h2 className="font-medium">
          School changes ({diff.organization_changes.length})
        </h2>
        <div className="max-h-96 overflow-auto">
          <DataTable
            columns={schoolChangeColumns}
            data={diff.organization_changes}
          />
        </div>
      </section>
    </>
  );
}
