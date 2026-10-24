import { useState } from "react";

import type { TeamResponse } from "@/api/generated/types";

export type Team = TeamResponse;

export function TeamName({ team }: { team?: Team | null }) {
  return team ? (
    <div className="max-w-xs leading-tight whitespace-normal break-words">
      <p className="font-medium">{team.team_name_zh || team.team_name_en}</p>
      {team.team_name_zh && team.team_name_en && (
        <p className="text-xs text-muted-foreground">{team.team_name_en}</p>
      )}
    </div>
  ) : (
    <span className="text-muted-foreground">No roster profile</span>
  );
}

function monogram(name: string) {
  const words = name.split(/\s+/).filter(Boolean);
  if (words.length > 1)
    return words
      .slice(0, 2)
      .map((word) => word[0])
      .join("")
      .toUpperCase();
  return [...name].slice(0, 2).join("").toUpperCase();
}

export function SchoolLogo({
  team,
  className = "size-8",
}: {
  team: Team;
  className?: string;
}) {
  const [failedSchool, setFailedSchool] = useState<string | null>(null);
  const name = team.school_name_zh || team.school_name_en;
  return failedSchool !== team.organization_id ? (
    <img
      src={`/api/v2/organizations/${encodeURIComponent(team.organization_id)}/logo`}
      alt={`${name} logo`}
      loading="lazy"
      className={`${className} shrink-0 rounded-md border bg-card object-contain p-0.5`}
      onError={() => setFailedSchool(team.organization_id)}
    />
  ) : (
    <span
      role="img"
      aria-label={`${name} (logo unavailable)`}
      title="Logo unavailable"
      className={`${className} inline-flex shrink-0 items-center justify-center rounded-md bg-secondary text-xs font-semibold text-secondary-foreground ring-1 ring-border`}
    >
      {monogram(team.school_name_en || name)}
    </span>
  );
}

export function TeamSchool({ team }: { team?: Team | null }) {
  if (!team) return <span className="text-muted-foreground">—</span>;
  return (
    <div className="flex items-center gap-3">
      <SchoolLogo team={team} />
      <div className="max-w-xs leading-tight whitespace-normal break-words">
        <p>{team.school_name_zh || team.school_name_en}</p>
        {team.school_name_zh && team.school_name_en && (
          <p className="text-xs text-muted-foreground">{team.school_name_en}</p>
        )}
        <p className="font-mono text-xs text-muted-foreground">
          {team.organization_id}
        </p>
      </div>
    </div>
  );
}
