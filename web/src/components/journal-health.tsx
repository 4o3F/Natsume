import { TriangleAlert } from "lucide-react";

import type { JournalHealth } from "@/api/generated/types";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { formatOffset, formatTimestamp } from "@/lib/time";
import { availabilityLabels } from "./event-labels";

/**
 * Explains why journal history may be incomplete. Rendered as a polite status
 * region because it is refreshed by polling and must not interrupt.
 */
export function JournalHealthNotice({
  journal,
  className,
}: {
  journal: JournalHealth;
  className?: string;
}) {
  const offset = journal.server_utc_offset_minutes;
  const lines: string[] = [];
  if (journal.status === "degraded")
    lines.push(
      "The journal cannot write right now; new events are being dropped until it recovers.",
    );
  else if (journal.paused)
    lines.push(
      "Recording is paused because the journal reached its disk budget; new events are being dropped.",
    );
  if (journal.dropped_total > 0)
    lines.push(
      `${journal.dropped_total} event${journal.dropped_total === 1 ? "" : "s"} dropped since this server started${
        journal.dropped_since === null
          ? ""
          : ` (first at ${formatTimestamp(journal.dropped_since, offset)} ${formatOffset(offset)})`
      }.`,
    );
  if (lines.length === 0) return null;
  return (
    <Alert variant="warning" role="status" className={className}>
      <TriangleAlert aria-hidden="true" />
      <AlertTitle>
        {journal.status === "ready"
          ? "Some journal events were dropped"
          : `Journal ${availabilityLabels[journal.status].toLowerCase()}`}
        ; history may be incomplete
      </AlertTitle>
      <AlertDescription>
        {lines.map((line) => (
          <p key={line}>{line}</p>
        ))}
      </AlertDescription>
    </Alert>
  );
}

/** Oldest retained event time, so pruned history is not mistaken for absence. */
export function RetainedSince({ journal }: { journal: JournalHealth }) {
  return (
    <span className="text-xs text-muted-foreground tabular-nums">
      {journal.oldest_retained_at === null
        ? "No events retained"
        : `History retained since ${formatTimestamp(
            journal.oldest_retained_at,
            journal.server_utc_offset_minutes,
          )} ${formatOffset(journal.server_utc_offset_minutes)}`}
    </span>
  );
}
