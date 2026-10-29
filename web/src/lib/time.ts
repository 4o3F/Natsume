/**
 * Journal time display. Journal timestamps are Unix milliseconds rendered in
 * the Server's local offset (`server_utc_offset_minutes`), so every operator
 * sees contest-floor time regardless of browser time zone. The browser offset
 * is only a fallback before the first journal response arrives.
 */

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

function shifted(ms: number, offsetMinutes: number) {
  return new Date(ms + offsetMinutes * MINUTE_MS).toISOString();
}

/** `YYYY-MM-DD HH:mm:ss` in the given UTC offset. */
export function formatTimestamp(ms: number, offsetMinutes: number) {
  const iso = shifted(ms, offsetMinutes);
  return `${iso.slice(0, 10)} ${iso.slice(11, 19)}`;
}

/** `UTC+08:00` style label for an offset in minutes. */
export function formatOffset(offsetMinutes: number) {
  const sign = offsetMinutes < 0 ? "−" : "+";
  const total = Math.abs(offsetMinutes);
  const hours = String(Math.floor(total / 60)).padStart(2, "0");
  const minutes = String(total % 60).padStart(2, "0");
  return `UTC${sign}${hours}:${minutes}`;
}

/** Browser offset in minutes east of UTC at `now`. */
export function browserOffsetMinutes(now: number) {
  return -new Date(now).getTimezoneOffset();
}

/** Duration with its two most significant units, e.g. `3h 4m`. */
export function formatDuration(ms: number) {
  const total = Math.max(0, Math.floor(ms / 1000));
  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3_600);
  const minutes = Math.floor((total % 3_600) / 60);
  const seconds = total % 60;
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${minutes}m`;
  if (minutes) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

/** Value for `<input type="datetime-local">` showing `ms` in the given offset. */
export function toDateTimeLocal(ms: number, offsetMinutes: number) {
  return shifted(ms, offsetMinutes).slice(0, 16);
}

const DATE_TIME_LOCAL =
  /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/;

/** Parses a `datetime-local` value as wall time in the given offset. */
export function fromDateTimeLocal(value: string, offsetMinutes: number) {
  const match = DATE_TIME_LOCAL.exec(value);
  if (!match) return null;
  const [, year, month, day, hour, minute, second = "0"] = match;
  const utc = Date.UTC(
    Number(year),
    Number(month) - 1,
    Number(day),
    Number(hour),
    Number(minute),
    Number(second),
  );
  return Number.isNaN(utc) ? null : utc - offsetMinutes * MINUTE_MS;
}

/** Start of the calendar day containing `now` in the given offset. */
export function startOfDay(now: number, offsetMinutes: number) {
  const local = now + offsetMinutes * MINUTE_MS;
  return (
    local - (((local % DAY_MS) + DAY_MS) % DAY_MS) - offsetMinutes * MINUTE_MS
  );
}
