import type { AccountResponse_Serialize as AccountResponse } from "@/api/generated/types";
import { TeamName } from "@/components/roster-team";

/** Roster team behind a Photo `account_id`, or a notice when it left the roster. */
export function PhotoTeam({
  account,
  externalAccount,
  fallback = "Not on the current roster",
}: {
  account: AccountResponse | undefined;
  externalAccount: string;
  fallback?: string;
}) {
  return (
    <div className="space-y-0.5">
      {account?.team ? (
        <TeamName team={account.team} />
      ) : (
        <p className="text-sm text-muted-foreground">
          {account ? "No roster profile" : fallback}
        </p>
      )}
      <p className="font-mono text-xs text-muted-foreground">
        {externalAccount}
      </p>
    </div>
  );
}
