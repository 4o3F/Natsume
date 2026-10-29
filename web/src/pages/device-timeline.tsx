import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { CircleAlert, TriangleAlert } from "lucide-react";

import { unwrap } from "@/api/errors";
import type {
  DeviceTimeline as Timeline,
  TimelineGap,
  TimelineSession,
} from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { DataState } from "@/components/data-state";
import {
  disconnectReasonLabels,
  gapKindLabels,
  labelFor,
  sessionEndLabels,
  shortId,
} from "@/components/event-labels";
import { JournalHealthNotice } from "@/components/journal-health";
import { Badge } from "@/components/ui/badge";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { formatDuration, formatOffset, formatTimestamp } from "@/lib/time";

const HOUR_MS = 3_600_000;
const windows = [
  { value: "1h", label: "1h", ms: HOUR_MS },
  { value: "6h", label: "6h", ms: 6 * HOUR_MS },
  { value: "24h", label: "24h", ms: 24 * HOUR_MS },
  { value: "7d", label: "7d", ms: 7 * 24 * HOUR_MS },
] as const;
type WindowValue = (typeof windows)[number]["value"];

type Entry =
  | { type: "session"; at: number; session: TimelineSession }
  | { type: "gap"; at: number; gap: TimelineGap };

/** Server-reconstructed connection sessions and evidence gaps for one device. */
export function DeviceTimeline({ deviceId }: { deviceId: string }) {
  const { api } = useSessionScope();
  const [range, setRange] = useState<WindowValue>("24h");
  const span = windows.find((item) => item.value === range)!.ms;
  const timeline = useQuery({
    queryKey: ["journal", "timeline", deviceId, span],
    queryFn: async ({ signal }) => {
      const to = Date.now();
      return unwrap<Timeline>(
        await api.getDeviceTimeline(
          deviceId,
          { from: to - span, to },
          { signal },
        ),
      );
    },
    refetchInterval: LIST_POLL_MS,
  });
  const data = timeline.data;
  const entries: Entry[] = data
    ? [
        ...data.sessions.map((session): Entry => ({
          type: "session",
          at: session.visible_from,
          session,
        })),
        ...data.gaps.map((gap): Entry => ({ type: "gap", at: gap.from, gap })),
      ].sort((a, b) => b.at - a.at)
    : [];
  const offset = data?.journal.server_utc_offset_minutes ?? 0;

  return (
    <section aria-label="Connection timeline" className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <p className="text-sm font-semibold">Connection timeline</p>
          {data && (
            <p className="text-xs text-muted-foreground">
              Times in server time ({formatOffset(offset)})
            </p>
          )}
        </div>
        <ToggleGroup
          type="single"
          value={range}
          onValueChange={(value) => value && setRange(value as WindowValue)}
          aria-label="Timeline window"
        >
          {windows.map((item) => (
            <ToggleGroupItem
              key={item.value}
              value={item.value}
              aria-label={`Last ${item.label}`}
            >
              {item.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>
      {data && <JournalHealthNotice journal={data.journal} />}
      {timeline.isError && data && (
        <p role="status" className="text-xs text-destructive">
          Refresh failed; showing the last received timeline.
        </p>
      )}
      <DataState
        isLoading={timeline.isLoading}
        error={data ? null : timeline.error}
        isEmpty={entries.length === 0}
        emptyLabel="No retained sessions in this window"
        emptyDescription="Sessions appear after the device connects while the journal is recording."
      >
        <ol className="divide-y border bg-card">
          {entries.map((entry, index) =>
            entry.type === "session" ? (
              <SessionRow
                key={`s-${entry.session.session_id}`}
                session={entry.session}
                offset={offset}
              />
            ) : (
              <GapRow
                // Equivalent gaps can repeat a kind and start; the position tells them apart.
                key={`g-${entry.gap.kind}-${entry.gap.from}-${index}`}
                gap={entry.gap}
                offset={offset}
              />
            ),
          )}
        </ol>
      </DataState>
    </section>
  );
}

function SessionRow({
  session,
  offset,
}: {
  session: TimelineSession;
  offset: number;
}) {
  const end = session.end;
  const badge =
    end.state === "ongoing"
      ? ("success" as const)
      : end.state === "unknown"
        ? ("warning" as const)
        : ("muted" as const);
  return (
    <li className="space-y-2 px-4 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={badge} dot>
          {sessionEndLabels[end.state]}
        </Badge>
        {end.state === "disconnected" && (
          <span className="text-sm">
            {end.reason === null
              ? "Replaced by a new connection"
              : labelFor(disconnectReasonLabels, end.reason)}
          </span>
        )}
        <span className="ml-auto font-mono text-xs text-muted-foreground">
          {session.seat_code ?? "Seat unknown"}
        </span>
      </div>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
        <dt className="text-muted-foreground">Started</dt>
        <dd className="tabular-nums">
          {formatTimestamp(session.started_at, offset)}
          {session.truncated_start && (
            <span className="text-muted-foreground"> · before this window</span>
          )}
        </dd>
        <dt className="text-muted-foreground">Ended</dt>
        <dd className="tabular-nums">
          {end.state === "disconnected"
            ? formatTimestamp(end.at, offset)
            : end.state === "ongoing"
              ? "Still connected"
              : end.no_later_than === null
                ? "Unknown; no later evidence"
                : `Unknown; no later than ${formatTimestamp(end.no_later_than, offset)}`}
          {session.truncated_end && end.state === "disconnected" && (
            <span className="text-muted-foreground"> · after this window</span>
          )}
        </dd>
        <dt className="text-muted-foreground">Duration</dt>
        <dd className="flex flex-wrap items-center gap-x-2 tabular-nums">
          {session.duration_ms === null
            ? "Unknown"
            : `${formatDuration(session.duration_ms)}${session.truncated_start || session.truncated_end ? " within this window" : ""}`}
          {session.imprecise && (
            <span className="inline-flex items-center gap-1 text-warning-strong">
              <TriangleAlert aria-hidden="true" className="size-3" />
              Imprecise: the server clock moved
            </span>
          )}
        </dd>
        <dt className="text-muted-foreground">Session</dt>
        <dd className="font-mono" title={session.session_id}>
          {shortId(session.session_id)}
        </dd>
      </dl>
    </li>
  );
}

function GapRow({ gap, offset }: { gap: TimelineGap; offset: number }) {
  return (
    <li className="flex items-start gap-3 bg-warning-subtle px-4 py-3 text-sm">
      <CircleAlert
        aria-hidden="true"
        className="mt-0.5 size-4 shrink-0 text-warning-strong"
      />
      <div className="min-w-0">
        <p className="font-medium">{gapKindLabels[gap.kind]}</p>
        <p className="text-xs tabular-nums">
          {formatTimestamp(gap.from, offset)} –{" "}
          {gap.to === null ? "unresolved" : formatTimestamp(gap.to, offset)}
          {" · "}sessions may be missing or incomplete
        </p>
      </div>
    </li>
  );
}
