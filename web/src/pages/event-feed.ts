import type {
  EventPage,
  JournalHealth,
  JournalRecord,
  OperatorDisplayName,
} from "../api/generated/types";

/** Head polling cadence for the live event list. */
export const EVENT_POLL_MS = 10_000;
/**
 * Bound for shown rows, held-back rows and one head catch-up. Trimmed or
 * skipped rows remain reachable through "Load older" because the cursor
 * moves to the last kept row.
 */
export const MAX_EVENT_ROWS = 2_000;

export interface EventFeed {
  /** Shown rows, newest first. */
  records: JournalRecord[];
  /** Newer rows held back while the reader is scrolled away from the top. */
  pending: JournalRecord[];
  /**
   * True when more new rows arrived than `pending` keeps, so `pending` is not
   * contiguous with `records`; showing it replaces the list.
   */
  pendingTruncated: boolean;
  /**
   * True after newer rows replaced the list because too many arrived at once;
   * the events in between are only reachable through "Load older".
   */
  skipped: boolean;
  /**
   * True after "Load older" pushed the newest rows out of the bounded window.
   * The shown rows are then no longer the head of the list: new rows are held
   * back as non-contiguous `pending` and replace the list when shown.
   */
  trimmedTop: boolean;
  /**
   * Newest ID known before the top was trimmed, so head polling keeps asking
   * only for rows newer than it; `null` while the first shown row is newest.
   */
  newest: number | null;
  /** Display names keyed by operator ID; `null` when the account is unknown. */
  operators: ReadonlyMap<string, string | null>;
  journal: JournalHealth | null;
  /** `before` cursor for the next older page; `null` when none remain. */
  older: number | null;
}

export const emptyFeed: EventFeed = {
  records: [],
  pending: [],
  pendingTruncated: false,
  skipped: false,
  trimmedTop: false,
  newest: null,
  operators: new Map(),
  journal: null,
  older: null,
};

export function recordId(record: JournalRecord) {
  return record.state === "readable" ? record.header.id : record.id;
}

/** Newest known ID, used as the `after` cursor for head polling. */
export function headId(feed: EventFeed) {
  if (feed.pending.length > 0) return recordId(feed.pending[0]);
  if (feed.newest !== null) return feed.newest;
  const newest = feed.records[0];
  return newest ? recordId(newest) : null;
}

function names(
  current: ReadonlyMap<string, string | null>,
  operators: OperatorDisplayName[],
) {
  if (operators.length === 0) return current;
  const next = new Map(current);
  for (const { operator_id, display_name } of operators)
    next.set(operator_id, display_name);
  return next;
}

/** Concatenates descending pages, dropping IDs already present. */
function mergeDescending(...lists: JournalRecord[][]) {
  const seen = new Set<number>();
  const merged: JournalRecord[] = [];
  for (const record of lists.flat()) {
    const id = recordId(record);
    if (seen.has(id)) continue;
    seen.add(id);
    merged.push(record);
  }
  return merged;
}

function trimmed(feed: EventFeed): EventFeed {
  if (feed.records.length <= MAX_EVENT_ROWS) return feed;
  const records = feed.records.slice(0, MAX_EVENT_ROWS);
  return { ...feed, records, older: recordId(records[records.length - 1]) };
}

/** Replaces the feed with the newest page for new filters. */
export function feedFromPage(page: EventPage): EventFeed {
  return {
    ...emptyFeed,
    records: page.events,
    operators: names(new Map(), page.operators),
    journal: page.journal,
    older: page.has_more ? page.next_before : null,
  };
}

/**
 * Appends an older page fetched with `before`. A response whose `before` is no
 * longer the feed's cursor (the list was replaced or trimmed meanwhile) is
 * discarded, so it cannot overwrite the cursor or reattach to other rows. Rows
 * beyond `MAX_EVENT_ROWS` are evicted from the newest end; the page just
 * loaded is kept.
 */
export function appendOlder(
  feed: EventFeed,
  page: EventPage,
  before: number,
): EventFeed {
  if (feed.older !== before) return feed;
  const merged = mergeDescending(feed.records, page.events);
  const evicted = merged.length > MAX_EVENT_ROWS;
  return {
    ...feed,
    records: evicted ? merged.slice(-MAX_EVENT_ROWS) : merged,
    // Held-back rows no longer touch the shown rows once the top is evicted.
    pendingTruncated: feed.pendingTruncated || evicted,
    trimmedTop: feed.trimmedTop || evicted,
    newest: evicted ? headId(feed) : feed.newest,
    operators: names(feed.operators, page.operators),
    journal: page.journal,
    older: page.has_more ? page.next_before : null,
  };
}

/**
 * Applies newer rows from head polling. At the top they are shown directly;
 * otherwise they wait in `pending` so the reader's position does not move.
 */
export function applyHead(
  feed: EventFeed,
  head: Head,
  atTop: boolean,
): EventFeed {
  // A truncated catch-up is not contiguous with anything held so far.
  const merged = head.truncated
    ? head.events
    : mergeDescending(head.events, feed.pending);
  const next = {
    ...feed,
    operators: names(feed.operators, head.operators),
    journal: head.journal,
    pending: merged.slice(0, MAX_EVENT_ROWS),
    pendingTruncated:
      head.truncated ||
      feed.pendingTruncated ||
      feed.trimmedTop ||
      merged.length > MAX_EVENT_ROWS,
  };
  // After the top was trimmed the shown rows are not the newest; replacing
  // them is the reader's choice.
  return atTop && !feed.trimmedTop ? showPending(next) : next;
}

/**
 * Moves held-back rows into view, e.g. from the "N new events" affordance.
 * Truncated rows replace the list; the cursor continues below them.
 */
export function showPending(feed: EventFeed): EventFeed {
  if (feed.pending.length === 0) return feed;
  if (feed.pendingTruncated)
    return {
      ...feed,
      records: feed.pending,
      pending: [],
      pendingTruncated: false,
      skipped: true,
      trimmedTop: false,
      newest: null,
      older: recordId(feed.pending[feed.pending.length - 1]),
    };
  return trimmed({
    ...feed,
    records: mergeDescending(feed.pending, feed.records),
    pending: [],
  });
}

/** Rows newer than the head cursor, possibly cut short at the catch-up bound. */
interface Head {
  events: JournalRecord[];
  operators: OperatorDisplayName[];
  journal: JournalHealth;
  /** True when more new rows exist below `events` than one catch-up fetches. */
  truncated: boolean;
}

/**
 * Fetches events newer than `after`. A head page is capped, so while the
 * server reports `has_more` this continues downwards with `before` inside the
 * same `after` bound, until the gap is closed or `maxRows` rows are held; the
 * newest rows are kept and the result is marked truncated.
 */
export async function fetchHead(
  after: number,
  fetchPage: (cursor: {
    after: number;
    before: number | null;
  }) => Promise<EventPage>,
  maxRows: number,
): Promise<Head> {
  const events: JournalRecord[] = [];
  const operators: OperatorDisplayName[] = [];
  let before: number | null = null;
  for (;;) {
    const page = await fetchPage({ after, before });
    events.push(...page.events);
    operators.push(...page.operators);
    const next = page.next_before;
    if (
      !page.has_more ||
      next === null ||
      next <= after ||
      (before !== null && next >= before)
    )
      return { events, operators, journal: page.journal, truncated: false };
    if (events.length >= maxRows)
      return { events, operators, journal: page.journal, truncated: true };
    before = next;
  }
}
