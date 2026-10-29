import { expect, it, vi } from "vitest";

import type {
  EventPage,
  JournalHealth,
  JournalRecord,
} from "../api/generated/types";
import {
  MAX_EVENT_ROWS,
  appendOlder,
  applyHead,
  feedFromPage,
  fetchHead,
  headId,
  recordId,
  showPending,
} from "./event-feed";

const journal: JournalHealth = {
  status: "ready",
  paused: false,
  dropped_total: 0,
  dropped_since: null,
  oldest_retained_id: 1,
  oldest_retained_at: 0,
  server_utc_offset_minutes: 0,
};

function record(id: number): JournalRecord {
  return id % 7 === 0
    ? { state: "unreadable", id, kind: "unknown" }
    : {
        state: "readable",
        header: {
          id,
          occurred_at_ms: id * 1_000,
          boot_id: "boot",
          category: "device",
          severity: "info",
          device_id: null,
          session_id: null,
          actor_operator_id: null,
          request_id: null,
          operation_id: null,
          trace_id: null,
          seat_code: null,
        },
        details: { v: 1, kind: "auth_succeeded" },
      };
}

function page(ids: number[], next_before: number | null = null): EventPage {
  return {
    events: ids.map(record),
    has_more: next_before !== null,
    next_before,
    operators: [],
    journal,
  };
}

const ids = (records: JournalRecord[]) => records.map(recordId);

it("tracks the older cursor only while more pages remain", () => {
  const feed = feedFromPage(page([10, 9, 8], 8));
  expect(feed.older).toBe(8);
  expect(headId(feed)).toBe(10);
  const older = appendOlder(feed, page([8, 7, 6]), 8);
  expect(ids(older.records)).toEqual([10, 9, 8, 7, 6]);
  expect(older.older).toBeNull();
});

it("shows new rows at the top and holds them back when scrolled away", () => {
  const feed = feedFromPage(page([10, 9]));
  const away = applyHead(
    feed,
    { events: page([12, 11]).events, operators: [], journal, truncated: false },
    false,
  );
  expect(ids(away.records)).toEqual([10, 9]);
  expect(ids(away.pending)).toEqual([12, 11]);
  expect(headId(away)).toBe(12);
  const more = applyHead(
    away,
    { events: page([13]).events, operators: [], journal, truncated: false },
    false,
  );
  expect(ids(more.pending)).toEqual([13, 12, 11]);
  expect(ids(showPending(more).records)).toEqual([13, 12, 11, 10, 9]);
  const top = applyHead(
    more,
    { events: page([14]).events, operators: [], journal, truncated: false },
    true,
  );
  expect(ids(top.records)).toEqual([14, 13, 12, 11, 10, 9]);
  expect(top.pending).toEqual([]);
});

it("trims to the row cap and moves the older cursor to the last kept row", () => {
  const all = Array.from(
    { length: MAX_EVENT_ROWS },
    (_, i) => MAX_EVENT_ROWS - i,
  );
  const feed = feedFromPage(page(all));
  const next = applyHead(
    feed,
    {
      events: page([MAX_EVENT_ROWS + 2, MAX_EVENT_ROWS + 1]).events,
      operators: [],
      journal,
      truncated: false,
    },
    true,
  );
  expect(next.records).toHaveLength(MAX_EVENT_ROWS);
  expect(next.skipped).toBe(false);
  expect(next.older).toBe(3);
});

it("merges operator names across pages", () => {
  const first = feedFromPage({
    ...page([2], 2),
    operators: [{ operator_id: "a", display_name: "alice" }],
  });
  const next = appendOlder(
    first,
    {
      ...page([1]),
      operators: [{ operator_id: "b", display_name: null }],
    },
    2,
  );
  expect([...next.operators]).toEqual([
    ["a", "alice"],
    ["b", null],
  ]);
});

it("continues head fetching while has_more within the same after bound", async () => {
  const fetchPage = vi
    .fn<
      (cursor: { after: number; before: number | null }) => Promise<EventPage>
    >()
    .mockResolvedValueOnce(page([30, 29], 29))
    .mockResolvedValueOnce(page([28, 27], 27))
    .mockResolvedValueOnce(page([26]));
  const head = await fetchHead(25, fetchPage, 100);
  expect(fetchPage.mock.calls.map(([cursor]) => cursor)).toEqual([
    { after: 25, before: null },
    { after: 25, before: 29 },
    { after: 25, before: 27 },
  ]);
  expect(ids(head.events)).toEqual([30, 29, 28, 27, 26]);
});

it("stops head fetching when the cursor would not advance", async () => {
  const fetchPage = vi
    .fn<
      (cursor: { after: number; before: number | null }) => Promise<EventPage>
    >()
    .mockResolvedValue(page([30], 30));
  await fetchHead(25, fetchPage, 100);
  expect(fetchPage).toHaveBeenCalledTimes(2);
});

const range = (from: number, to: number) =>
  Array.from({ length: from - to + 1 }, (_, i) => from - i);

it("bounds head catch-up and keeps the newest rows", async () => {
  const fetchPage = vi
    .fn<
      (cursor: { after: number; before: number | null }) => Promise<EventPage>
    >()
    .mockResolvedValueOnce(page(range(1000, 901), 901))
    .mockResolvedValueOnce(page(range(900, 801), 801))
    .mockResolvedValueOnce(page(range(800, 701), 701));
  const head = await fetchHead(10, fetchPage, 150);
  expect(fetchPage).toHaveBeenCalledTimes(2);
  expect(head.truncated).toBe(true);
  expect(ids(head.events)).toEqual(range(1000, 801));
});

it("replaces the list with a truncated catch-up and keeps older rows reachable", () => {
  const feed = feedFromPage(page([10, 9]));
  const events = page(range(MAX_EVENT_ROWS + 600, 501)).events;
  const head = { events, operators: [], journal, truncated: true };
  const top = applyHead(feed, head, true);
  expect(top.records).toHaveLength(MAX_EVENT_ROWS);
  expect(recordId(top.records[0])).toBe(MAX_EVENT_ROWS + 600);
  expect(top.skipped).toBe(true);
  // Load older continues below the newest kept row, through the skipped gap.
  expect(top.older).toBe(601);
  expect(top.pending).toEqual([]);

  const away = applyHead(feed, head, false);
  expect(ids(away.records)).toEqual([10, 9]);
  expect(away.pending).toHaveLength(MAX_EVENT_ROWS);
  expect(away.pendingTruncated).toBe(true);
  expect(headId(away)).toBe(MAX_EVENT_ROWS + 600);
  const shown = showPending(away);
  expect(shown).toMatchObject({ skipped: true, older: 601, pending: [] });
  expect(shown.pendingTruncated).toBe(false);
});

it("caps held-back rows and marks the overflow", () => {
  const feed = feedFromPage(page([10, 9]));
  const first = applyHead(
    feed,
    {
      events: page(range(MAX_EVENT_ROWS + 10, 11)).events,
      operators: [],
      journal,
      truncated: false,
    },
    false,
  );
  expect(first.pending).toHaveLength(MAX_EVENT_ROWS);
  expect(first.pendingTruncated).toBe(false);
  const second = applyHead(
    first,
    {
      events: page([MAX_EVENT_ROWS + 11]).events,
      operators: [],
      journal,
      truncated: false,
    },
    false,
  );
  expect(second.pending).toHaveLength(MAX_EVENT_ROWS);
  expect(recordId(second.pending[0])).toBe(MAX_EVENT_ROWS + 11);
  expect(second.pendingTruncated).toBe(true);
  expect(ids(second.records)).toEqual([10, 9]);
  const shown = showPending(second);
  expect(shown.skipped).toBe(true);
  expect(shown.older).toBe(12);
});

it("discards an older page whose cursor no longer matches the feed", () => {
  const feed = feedFromPage(page(range(1000, 501), 501));
  const stale = appendOlder(feed, page(range(500, 401)), 501);
  expect(stale.older).toBeNull();
  // A truncated catch-up replaced the list while the older request was out.
  const replaced = showPending(
    applyHead(
      feed,
      {
        events: page(range(5000, 3001)).events,
        operators: [],
        journal,
        truncated: true,
      },
      true,
    ),
  );
  expect(replaced.older).toBe(3001);
  const late = appendOlder(replaced, page(range(500, 401)), 501);
  expect(late).toBe(replaced);
  expect(late.older).toBe(3001);
  expect(ids(late.records)).toEqual(range(5000, 3001));
});

it("applies an older page and a head catch-up in either order", () => {
  const feed = feedFromPage(page(range(1000, 901), 901));
  const head = {
    events: page([1002, 1001]).events,
    operators: [],
    journal,
    truncated: false,
  };
  const olderPage = page(range(900, 801), 801);
  const headFirst = appendOlder(applyHead(feed, head, true), olderPage, 901);
  const olderFirst = applyHead(appendOlder(feed, olderPage, 901), head, true);
  expect(ids(headFirst.records)).toEqual(range(1002, 801));
  expect(ids(olderFirst.records)).toEqual(ids(headFirst.records));
  expect(olderFirst.older).toBe(801);
  expect(headFirst.older).toBe(801);
});

it("evicts the newest rows when older pages pass the cap and keeps the page", () => {
  const feed = feedFromPage(page(range(MAX_EVENT_ROWS + 100, 101), 101));
  expect(feed.records).toHaveLength(MAX_EVENT_ROWS);
  const next = appendOlder(feed, page(range(100, 51), 51), 101);
  expect(next.records).toHaveLength(MAX_EVENT_ROWS);
  expect(recordId(next.records[next.records.length - 1])).toBe(51);
  expect(recordId(next.records[0])).toBe(MAX_EVENT_ROWS + 100 - 50);
  expect(next.trimmedTop).toBe(true);
  expect(next.older).toBe(51);
  // Polling keeps asking only for rows newer than the evicted head.
  expect(headId(next)).toBe(MAX_EVENT_ROWS + 100);
});

it("holds new rows as non-contiguous once the top was evicted", () => {
  const feed = appendOlder(
    feedFromPage(page(range(MAX_EVENT_ROWS + 100, 101), 101)),
    page(range(100, 51), 51),
    101,
  );
  const head = {
    events: page([MAX_EVENT_ROWS + 102, MAX_EVENT_ROWS + 101]).events,
    operators: [],
    journal,
    truncated: false,
  };
  // Even at the top of the window the reader chooses to jump to the newest.
  const held = applyHead(feed, head, true);
  expect(ids(held.records)).toEqual(ids(feed.records));
  expect(held.pendingTruncated).toBe(true);
  expect(headId(held)).toBe(MAX_EVENT_ROWS + 102);
  const shown = showPending(held);
  expect(ids(shown.records)).toEqual([
    MAX_EVENT_ROWS + 102,
    MAX_EVENT_ROWS + 101,
  ]);
  expect(shown).toMatchObject({
    trimmedTop: false,
    newest: null,
    older: MAX_EVENT_ROWS + 101,
  });
});

it("keeps older rows reachable after a pending truncation", () => {
  const feed = feedFromPage(page([10, 9], 9));
  const head = {
    events: page(range(MAX_EVENT_ROWS + 600, 501)).events,
    operators: [],
    journal,
    truncated: true,
  };
  const away = applyHead(feed, head, false);
  expect(away.pendingTruncated).toBe(true);
  const older = appendOlder(away, page([8, 7]), 9);
  expect(ids(older.records)).toEqual([10, 9, 8, 7]);
  expect(older.pendingTruncated).toBe(true);
  expect(older.pending).toHaveLength(MAX_EVENT_ROWS);
  expect(showPending(older).older).toBe(601);
});
