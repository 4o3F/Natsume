import { expect, it } from "vitest";

import {
  formatDuration,
  formatOffset,
  formatTimestamp,
  fromDateTimeLocal,
  startOfDay,
  toDateTimeLocal,
} from "./time";

// 2026-10-28T23:30:15Z
const instant = Date.UTC(2026, 9, 28, 23, 30, 15);

it("formats timestamps in the server offset rather than the browser zone", () => {
  expect(formatTimestamp(instant, 0)).toBe("2026-10-28 23:30:15");
  expect(formatTimestamp(instant, 480)).toBe("2026-10-29 07:30:15");
  expect(formatTimestamp(instant, -330)).toBe("2026-10-28 18:00:15");
});

it("labels offsets with sign, hours and minutes", () => {
  expect(formatOffset(0)).toBe("UTC+00:00");
  expect(formatOffset(480)).toBe("UTC+08:00");
  expect(formatOffset(-330)).toBe("UTC−05:30");
});

it("formats durations with the two most significant units", () => {
  expect(formatDuration(0)).toBe("0s");
  expect(formatDuration(-5_000)).toBe("0s");
  expect(formatDuration(59_999)).toBe("59s");
  expect(formatDuration(61_000)).toBe("1m 1s");
  expect(formatDuration(3 * 3_600_000 + 4 * 60_000 + 5_000)).toBe("3h 4m");
  expect(formatDuration(2 * 86_400_000 + 3 * 3_600_000)).toBe("2d 3h");
});

it("round-trips datetime-local values in the server offset", () => {
  expect(toDateTimeLocal(instant, 480)).toBe("2026-10-29T07:30");
  expect(fromDateTimeLocal("2026-10-29T07:30", 480)).toBe(instant - 15_000);
  expect(fromDateTimeLocal("2026-10-29T07:30:15", 480)).toBe(instant);
  expect(fromDateTimeLocal("", 480)).toBeNull();
  expect(fromDateTimeLocal("2026-10-29 07:30", 480)).toBeNull();
});

it("finds the start of the server-local day", () => {
  expect(startOfDay(instant, 0)).toBe(Date.UTC(2026, 9, 28));
  // 07:30 on the 29th at UTC+8 starts at 00:00 local = 16:00Z on the 28th.
  expect(startOfDay(instant, 480)).toBe(Date.UTC(2026, 9, 28, 16));
  expect(startOfDay(instant, -330)).toBe(Date.UTC(2026, 9, 28, 5, 30));
});
