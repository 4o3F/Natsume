import { useState } from "react";

import type { components } from "@/api/generated/schema";

export type Team = components["schemas"]["TeamResponse"];

export function TeamName({ team }: { team?: Team | null }) {
  return team ? (
    <div className="max-w-sm whitespace-normal break-words">
      <p>{team.team_name_zh || team.team_name_en}</p>
      {team.team_name_zh && team.team_name_en && (
        <p className="text-sm text-muted-foreground">{team.team_name_en}</p>
      )}
    </div>
  ) : (
    <span className="text-muted-foreground">No roster profile</span>
  );
}

export function TeamSchool({ team }: { team?: Team | null }) {
  const [failedSchool, setFailedSchool] = useState<string | null>(null);
  if (!team) return <span className="text-muted-foreground">—</span>;
  return (
    <div className="flex items-center gap-3">
      {failedSchool !== team.organization_id ? (
        <img
          src={`/api/v2/organizations/${encodeURIComponent(team.organization_id)}/logo`}
          alt={`${team.school_name_zh || team.school_name_en} logo`}
          loading="lazy"
          className="size-12 shrink-0 object-contain"
          onError={() => setFailedSchool(team.organization_id)}
        />
      ) : (
        <span className="w-12 shrink-0 whitespace-normal text-center text-xs text-amber-700 dark:text-amber-400">
          Logo unavailable
        </span>
      )}
      <div className="max-w-sm whitespace-normal break-words">
        <p>{team.school_name_zh || team.school_name_en}</p>
        {team.school_name_zh && team.school_name_en && (
          <p className="text-sm text-muted-foreground">{team.school_name_en}</p>
        )}
        <p className="text-xs text-muted-foreground">{team.organization_id}</p>
      </div>
    </div>
  );
}
