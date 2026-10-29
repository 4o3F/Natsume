import { expect, it } from "vitest";

import {
  categoryPresets,
  defaultEventFilters,
  deviceFilterFromText,
  eventQuery,
  isLiveRange,
  kindsFor,
  parseEventFilters,
  rangeCaption,
  resolveEventRange,
  serializeEventFilters,
  withCursor,
  type EventFilters,
} from "./event-filters";

const device = "01912345-6789-7abc-8def-0123456789ab";
const operator = "01912345-6789-7abc-8def-0123456789ac";

it("defaults to the device preset with no other filters", () => {
  expect(parseEventFilters(new URLSearchParams(), true)).toEqual(
    defaultEventFilters,
  );
  expect(serializeEventFilters(defaultEventFilters).toString()).toBe("");
});

it("round-trips administrator filters through the URL", () => {
  const filters: EventFilters = {
    preset: "all",
    range: "custom",
    from: 1_000,
    to: 2_000,
    deviceId: device,
    seatPrefix: null,
    kinds: ["disconnected", "operator_request"],
    severities: ["warning", "error"],
    operatorId: operator,
  };
  const params = serializeEventFilters(filters);
  expect(params.getAll("kind")).toEqual(["disconnected", "operator_request"]);
  expect(parseEventFilters(params, true)).toEqual(filters);
});

it("drops restricted presets, kinds and operator filters for viewers", () => {
  const params = new URLSearchParams(
    `category=audit&kind=operator_request&kind=disconnected&operator=${operator}`,
  );
  const filters = parseEventFilters(params, false);
  expect(filters.preset).toBe("device");
  expect(filters.kinds).toEqual(["disconnected"]);
  expect(filters.operatorId).toBeNull();
  expect(categoryPresets(false).map(({ value }) => value)).toEqual(["device"]);
  expect(categoryPresets(true).map(({ value }) => value)).toContain("all");
});

it("ignores malformed values and kinds outside the preset", () => {
  const filters = parseEventFilters(
    new URLSearchParams(
      "category=journal&range=week&kind=disconnected&kind=nope&kind=server_started&severity=fatal&device_id=ABC&from=1&seat_prefix=%07",
    ),
    true,
  );
  expect(filters).toEqual({
    ...defaultEventFilters,
    preset: "journal",
    kinds: ["server_started"],
  });
  expect(kindsFor("auth")).toContain("auth_failed");
  expect(kindsFor("auth")).not.toContain("disconnected");
});

it("orders reversed custom bounds and only keeps them for custom ranges", () => {
  expect(
    parseEventFilters(new URLSearchParams("range=custom&from=20&to=10"), true),
  ).toMatchObject({ from: 10, to: 20 });
  expect(
    parseEventFilters(new URLSearchParams("range=1h&from=20"), true).from,
  ).toBeNull();
});

it("reads the device field as an exact ID or a seat prefix", () => {
  expect(deviceFilterFromText(` ${device.toUpperCase()} `)).toEqual({
    deviceId: device,
    seatPrefix: null,
  });
  expect(deviceFilterFromText("A-0")).toEqual({
    deviceId: null,
    seatPrefix: "A-0",
  });
  expect(deviceFilterFromText("  ")).toEqual({
    deviceId: null,
    seatPrefix: null,
  });
});

it("resolves range presets in the server offset and pauses fixed ranges", () => {
  const now = Date.UTC(2026, 9, 28, 23, 30);
  const base = { from: null, to: null };
  expect(resolveEventRange({ ...base, range: "15m" }, now, 0)).toEqual({
    from: now - 900_000,
    to: null,
  });
  expect(resolveEventRange({ ...base, range: "today" }, now, 480)).toEqual({
    from: Date.UTC(2026, 9, 28, 16),
    to: null,
  });
  expect(
    resolveEventRange({ range: "custom", from: 5, to: 9 }, now, 0),
  ).toEqual({ from: 5, to: 9 });
  expect(isLiveRange("today")).toBe(true);
  expect(isLiveRange("custom")).toBe(false);
});

it("builds server queries with API parameter names and cursors", () => {
  const query = eventQuery(
    {
      ...defaultEventFilters,
      kinds: ["disconnected"],
      severities: ["error"],
      seatPrefix: "A-",
      operatorId: operator,
    },
    { from: 10, to: null },
  );
  expect(query.toString()).toBe(
    `category=device&kind=disconnected&severity=error&seat_prefix=A-&actor_operator_id=${operator}&from=10`,
  );
  expect(
    eventQuery(
      { ...defaultEventFilters, preset: "all" },
      {
        from: null,
        to: null,
      },
    ).toString(),
  ).toBe("");
  expect(withCursor(query, { after: 4, before: 9 }).get("after")).toBe("4");
  expect(withCursor(query, { before: null }).has("before")).toBe(false);
  expect(query.has("after")).toBe(false);
});

it("captions the anchored bounds so relative presets do not drift silently", () => {
  const now = Date.UTC(2026, 9, 28, 6, 5);
  const anchored = resolveEventRange(
    { range: "15m", from: null, to: null },
    now,
    480,
  );
  expect(rangeCaption(anchored, 480)).toBe(
    "Since 2026-10-28 13:50:00 UTC+08:00",
  );
  expect(rangeCaption({ from: now, to: now + 60_000 }, 0)).toBe(
    "2026-10-28 06:05:00 – 2026-10-28 06:06:00 UTC+00:00",
  );
  expect(rangeCaption({ from: null, to: now }, 0)).toBe(
    "Until 2026-10-28 06:05:00 UTC+00:00",
  );
  expect(rangeCaption({ from: null, to: null }, 0)).toBeNull();
});
