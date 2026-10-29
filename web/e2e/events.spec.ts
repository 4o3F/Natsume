import { expect, test, type Page } from "@playwright/test";

import type {
  DeviceSummaries,
  DeviceTimeline,
  EventPage,
  JournalHealth,
  JournalRecord,
} from "../src/api/generated/types";
import { device, fulfillJson, operator } from "./fixtures/devices";
import { deviceRow, openDeviceDetails } from "./fixtures/ui";

const now = Date.UTC(2026, 9, 28, 4, 0, 0);
const offset = 480;
const health: JournalHealth = {
  status: "ready",
  paused: false,
  dropped_total: 0,
  dropped_since: null,
  oldest_retained_id: 1,
  oldest_retained_at: now - 86_400_000,
  server_utc_offset_minutes: offset,
};
const session = "01923456-0000-7000-8000-000000000001";

function disconnected(id: number): JournalRecord {
  return {
    state: "readable",
    header: {
      id,
      occurred_at_ms: now - (1_000 - id) * 1_000,
      boot_id: "01923456-0000-7000-8000-0000000000b0",
      category: "device",
      severity: "warning",
      device_id: device.device_id,
      session_id: session,
      actor_operator_id: null,
      request_id: null,
      operation_id: null,
      trace_id: null,
      seat_code: "A-01",
    },
    details: {
      v: 1,
      kind: "disconnected",
      session_id: session,
      reason: "ping_silence",
    },
  };
}

const audit: JournalRecord = {
  state: "readable",
  header: {
    ...(disconnected(500) as Extract<JournalRecord, { state: "readable" }>)
      .header,
    category: "audit",
    severity: "info",
    device_id: null,
    seat_code: null,
    actor_operator_id: operator.operator_id,
  },
  details: {
    v: 1,
    kind: "operator_request",
    route: "device",
    method: "patch",
    targets: [{ device: device.device_id }],
    status: 204,
    api_code: null,
    cause: null,
  },
};

function eventPage(
  events: JournalRecord[],
  journal: JournalHealth = health,
): EventPage {
  return {
    events,
    has_more: false,
    next_before: null,
    operators: [
      { operator_id: operator.operator_id, display_name: "operator" },
    ],
    journal,
  };
}

async function mockJournal(
  page: Page,
  options: {
    role?: "admin" | "viewer";
    events?: (
      params: URLSearchParams,
    ) => EventPage | Promise<EventPage> | "unavailable";
  } = {},
) {
  const requests: URLSearchParams[] = [];
  await page.route("**/api/v2/**", async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/api/v2/session")
      return fulfillJson(route, 200, {
        ...operator,
        role: options.role ?? "admin",
      });
    if (url.pathname === "/api/v2/journal/events") {
      requests.push(url.searchParams);
      const reply = await (options.events?.(url.searchParams) ?? eventPage([]));
      // A request the page aborted meanwhile can no longer be fulfilled.
      return (
        reply === "unavailable"
          ? fulfillJson(route, 503, {
              title: "Journal unavailable",
              status: 503,
              code: "journal_unavailable",
            })
          : fulfillJson(route, 200, reply)
      ).catch(() => undefined);
    }
    if (url.pathname === "/api/v2/operator/accounts")
      return fulfillJson(route, 200, [
        {
          operator_id: operator.operator_id,
          username: "operator",
          role: "admin",
        },
      ]);
    return fulfillJson(route, 404, {});
  });
  return requests;
}

test("administrators read typed events, unreadable rows and export the current filters", async ({
  page,
}) => {
  const sentinel = "SENTINEL-free-text-must-not-render";
  const requests = await mockJournal(page, {
    events: () =>
      eventPage(
        [
          { state: "unreadable", id: 502, kind: sentinel },
          audit,
          disconnected(499),
        ],
        { ...health, paused: true, status: "paused", dropped_total: 3 },
      ),
  });
  await page.goto("/events?category=all&severity=warning");
  const table = page.getByRole("table", { name: "Events", exact: true });
  await expect(table).toContainText("Heartbeat lost · Session 01923456");
  await expect(table).toContainText("PATCH Device · HTTP 204");
  await expect(table).toContainText("Unreadable event");
  await expect(table).toContainText("Unrecognized · Event #502");
  await expect(
    page.getByRole("columnheader", { name: "Time (UTC+08:00)" }),
  ).toBeVisible();
  await expect(table.getByText("operator", { exact: true })).toBeVisible();
  await expect(page.locator("body")).not.toContainText(sentinel);
  await expect(
    page.getByRole("status").filter({ hasText: "Journal paused" }),
  ).toContainText("3 events dropped");
  expect(requests[0].has("category")).toBe(false);
  expect(requests[0].getAll("severity")).toEqual(["warning"]);
  const exportLink = page.getByRole("link", { name: "Export JSONL" });
  await expect(exportLink).toHaveAttribute("download", "");
  await expect(exportLink).toHaveAttribute(
    "href",
    "/api/v2/journal/export?severity=warning",
  );
});

test("viewers only see device events without admin controls", async ({
  page,
}) => {
  const requests = await mockJournal(page, {
    role: "viewer",
    events: () => eventPage([disconnected(10)]),
  });
  await page.goto(
    `/events?category=audit&kind=operator_request&operator=${operator.operator_id}`,
  );
  await expect(page.getByRole("table", { name: "Events" })).toContainText(
    "Disconnected",
  );
  expect(requests[0].getAll("category")).toEqual(["device"]);
  expect(requests[0].has("kind")).toBe(false);
  expect(requests[0].has("actor_operator_id")).toBe(false);
  await expect(page.getByRole("group", { name: "Category" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /^Operator/ })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Export JSONL" })).toHaveCount(0);
});

test("new events wait behind an affordance instead of shifting a scrolled list", async ({
  page,
}) => {
  await page.clock.install({ time: now });
  const initial = Array.from({ length: 60 }, (_, index) =>
    disconnected(100 - index),
  );
  await mockJournal(page, {
    events: (params) =>
      params.get("after") === "100"
        ? eventPage([disconnected(101)])
        : eventPage(params.has("after") ? [] : initial),
  });
  await page.goto("/events");
  const table = page.getByRole("table", { name: "Events" });
  await expect(table.getByRole("row")).toHaveCount(61);
  await page.evaluate(() => window.scrollTo(0, 800));
  const before = await table.getByRole("row").nth(1).textContent();
  await page.clock.runFor(10_000);
  const notice = page.getByRole("button", { name: "1 new event" });
  await expect(notice).toBeVisible();
  expect(await page.evaluate(() => window.scrollY)).toBe(800);
  await expect(table.getByRole("row").nth(1)).toHaveText(before ?? "");
  await notice.click();
  await expect(table.getByRole("row")).toHaveCount(62);
  await expect(notice).toHaveCount(0);
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
});

test("fixed custom ranges pause head polling", async ({ page }) => {
  await page.clock.install({ time: now });
  const requests = await mockJournal(page, {
    events: () => eventPage([disconnected(10)]),
  });
  await page.goto(`/events?range=custom&from=${now - 3_600_000}&to=${now}`);
  await expect(
    page.getByText("Fixed range; live updates paused"),
  ).toBeVisible();
  await expect(page.getByLabel("From (UTC+08:00)")).toHaveValue(
    "2026-10-28T11:00",
  );
  await page.clock.runFor(30_000);
  expect(requests).toHaveLength(1);
  expect(requests[0].get("from")).toBe(String(now - 3_600_000));
});

test("device table shows journal summaries and the sheet timeline keeps uncertainty explicit", async ({
  page,
}) => {
  const offline = structuredClone(device);
  offline.convergence.connection_state = "offline";
  const summaries: DeviceSummaries = {
    devices: [
      {
        device_id: device.device_id,
        last_connected_at: now - 7_200_000,
        last_disconnected_at: now - 3_600_000,
        last_disconnect_reason: "ping_silence",
        disconnects: 4,
      },
    ],
    journal: health,
  };
  const timeline: DeviceTimeline = {
    device_id: device.device_id,
    sessions: [
      {
        session_id: session,
        boot_id: "01923456-0000-7000-8000-0000000000b0",
        started_at: now - 7_200_000,
        end: { state: "unknown", no_later_than: now - 3_600_000 },
        visible_from: now - 7_200_000,
        visible_to: now - 3_600_000,
        duration_ms: null,
        truncated_start: false,
        truncated_end: false,
        imprecise: true,
        seat_code: null,
      },
    ],
    gaps: [{ kind: "dropped", from: now - 1_800_000, to: null }],
    journal: health,
  };
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session") return fulfillJson(route, 200, operator);
    if (path === "/api/v2/devices") return fulfillJson(route, 200, [offline]);
    if (path === "/api/v2/journal/device-summaries")
      return fulfillJson(route, 200, summaries);
    if (path === `/api/v2/journal/devices/${device.device_id}/timeline`)
      return fulfillJson(route, 200, timeline);
    return fulfillJson(route, 404, {});
  });
  await page.goto("/devices");
  const row = deviceRow(page, "machine-01");
  await expect(row).toContainText("4");
  await expect(page.getByRole("button", { name: /Flapping/ })).toContainText(
    "1",
  );
  const drawer = await openDeviceDetails(page, "machine-01");
  const tabs = drawer.getByRole("tablist", { name: "Device view" });
  await tabs.getByRole("tab", { name: "State" }).focus();
  await page.keyboard.press("ArrowRight");
  await expect(tabs.getByRole("tab", { name: "Timeline" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  const panel = drawer.getByRole("tabpanel");
  await expect(panel).toContainText("End unknown");
  await expect(panel).toContainText("Unknown; no later than");
  await expect(panel).toContainText("Imprecise: the server clock moved");
  await expect(panel).toContainText("Seat unknown");
  await expect(panel).toContainText("Events dropped");
  await expect(panel).toContainText("unresolved");
});

test("device journal columns degrade to Unknown when summaries fail", async ({
  page,
}) => {
  const offline = structuredClone(device);
  offline.convergence.connection_state = "offline";
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session") return fulfillJson(route, 200, operator);
    if (path === "/api/v2/devices") return fulfillJson(route, 200, [offline]);
    return fulfillJson(route, 503, {
      title: "Journal unavailable",
      status: 503,
      code: "journal_unavailable",
    });
  });
  await page.goto("/devices");
  const row = deviceRow(page, "machine-01");
  await expect(row.getByText("Unknown", { exact: true })).toHaveCount(2);
  await expect(page.getByRole("button", { name: /Flapping/ })).toHaveCount(0);
  await expect(page.getByText("Flapping", { exact: true })).toBeVisible();
});

test("events page fits a 390px viewport", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockJournal(page, { events: () => eventPage([disconnected(10)]) });
  await page.goto("/events");
  await expect(page.getByRole("table", { name: "Events" })).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});

function gate() {
  let open = () => {};
  const opened = new Promise<void>((resolve) => (open = resolve));
  return { opened, open };
}

const idRange = (from: number, to: number) =>
  Array.from({ length: from - to + 1 }, (_, i) => from - i);

function pageOf(ids: number[], nextBefore: number | null): EventPage {
  return {
    ...eventPage(ids.map(disconnected)),
    has_more: nextBefore !== null,
    next_before: nextBefore,
  };
}

test("a late older page cannot overwrite a catch-up that replaced the list", async ({
  page,
}) => {
  await page.clock.install({ time: now });
  const olderGate = gate();
  await mockJournal(page, {
    events: async (params) => {
      if (params.get("before") === "501") {
        await olderGate.opened;
        return pageOf(idRange(500, 401), null);
      }
      if (params.get("after") === "1000")
        return pageOf(idRange(5000, 3001), 3001);
      return params.has("after")
        ? eventPage([])
        : pageOf(idRange(1000, 501), 501);
    },
  });
  await page.goto("/events");
  const table = page.getByRole("table", { name: "Events" });
  await expect(table).toBeVisible();
  await page.getByRole("button", { name: "Load older" }).click();
  await page.clock.runFor(10_000);
  // Loading older rows scrolled away from the top, so the catch-up waits.
  await page.getByRole("button", { name: "2000+ new events" }).click();
  await expect(page.getByText("2000 shown")).toBeVisible();
  olderGate.open();
  await page.clock.runFor(1_000);
  // The cursor still points below the replaced list; nothing was lost.
  await expect(page.getByRole("button", { name: "Load older" })).toBeVisible();
  await expect(page.getByText("No older matching events")).toHaveCount(0);
  await expect(page.getByText("2000 shown")).toBeVisible();
});

test("an older page and a head catch-up both apply in either completion order", async ({
  page,
}) => {
  await page.clock.install({ time: now });
  const olderGate = gate();
  await mockJournal(page, {
    events: async (params) => {
      if (params.get("before") === "10") {
        await olderGate.opened;
        return pageOf([9, 8], null);
      }
      if (params.get("after") === "11") return pageOf([13, 12], null);
      return params.has("after") ? eventPage([]) : pageOf([11, 10], 10);
    },
  });
  await page.goto("/events");
  const table = page.getByRole("table", { name: "Events" });
  await expect(table.getByRole("row")).toHaveCount(3);
  await page.getByRole("button", { name: "Load older" }).click();
  await page.clock.runFor(10_000);
  // Head first: the older response then lands below the new rows.
  await expect(table.getByRole("row")).toHaveCount(5);
  olderGate.open();
  await expect(table.getByRole("row")).toHaveCount(7);
  await expect(page.getByText("No older matching events")).toBeVisible();
});

test("switching filters drops the old filter's late older page", async ({
  page,
}) => {
  const olderGate = gate();
  await mockJournal(page, {
    events: async (params) => {
      const narrowed = params.has("from");
      if (params.get("before") === "51") {
        await olderGate.opened;
        return pageOf(idRange(50, 1), null);
      }
      if (params.get("before") === "251")
        return pageOf(idRange(250, 201), null);
      return narrowed
        ? pageOf(idRange(300, 251), 251)
        : pageOf(idRange(100, 51), 51);
    },
  });
  await page.goto("/events");
  await expect(page.getByRole("button", { name: "Load older" })).toBeVisible();
  await page.getByRole("button", { name: "Load older" }).click();
  await page.getByRole("radio", { name: "15 min" }).click();
  await page.getByRole("button", { name: "Load older" }).click();
  await expect(page.getByText("No older matching events")).toBeVisible();
  olderGate.open();
  await page.waitForTimeout(300);
  // The second filter keeps the page it loaded instead of its first page.
  await expect(page.getByText("No older matching events")).toBeVisible();
  await expect(page.getByText("100 shown")).toBeVisible();
});

test("export stays disabled until the query range is resolved", async ({
  page,
}) => {
  const first = gate();
  await mockJournal(page, {
    events: async () => {
      await first.opened;
      return eventPage([disconnected(10)]);
    },
  });
  const from = now - 3_600_000;
  await page.goto(`/events?range=custom&from=${from}&to=${now}`);
  await expect(
    page.getByRole("button", { name: "Export JSONL" }),
  ).toBeDisabled();
  await expect(page.getByRole("link", { name: "Export JSONL" })).toHaveCount(0);
  first.open();
  const link = page.getByRole("link", { name: "Export JSONL" });
  await expect(link).toHaveAttribute("href", new RegExp(`from=${from}`));
  await expect(link).toHaveAttribute("href", new RegExp(`to=${now}`));
});

test("a failed first load offers Retry and starts live updates only afterwards", async ({
  page,
}) => {
  let failing = true;
  await mockJournal(page, {
    events: () => (failing ? "unavailable" : eventPage([disconnected(10)])),
  });
  await page.goto("/events");
  await expect(page.getByRole("alert")).toContainText("Journal unavailable");
  const status = page
    .getByRole("status")
    .filter({ hasText: /Events unavailable/ });
  await expect(status).toContainText("live updates not started");
  await expect(page.getByText("checks for new events")).toHaveCount(0);
  failing = false;
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByRole("table", { name: "Events" })).toContainText(
    "Disconnected",
  );
  await expect(page.getByText("checks for new events every 10s")).toBeVisible();
});

test.describe("touch viewports", () => {
  test.use({ hasTouch: true, isMobile: true });

  test("filter, list and tab controls have 48px hit areas", async ({
    page,
  }) => {
    await mockJournal(page, {
      events: () => pageOf(idRange(100, 51), 51),
    });
    await page.goto("/events");
    await expect(
      page.getByRole("button", { name: "Load older" }),
    ).toBeVisible();
    for (const control of [
      page.getByRole("radio", { name: "15 min" }),
      page.getByRole("button", { name: "Load older" }),
      page.getByRole("link", { name: "Export JSONL" }),
      page.getByRole("searchbox", { name: "Seat prefix or device ID" }),
    ]) {
      const box = await control.boundingBox();
      expect(box?.height).toBeGreaterThanOrEqual(48);
    }
  });
});

test("failed summary refreshes keep the last values marked as stale", async ({
  page,
}) => {
  await page.clock.install({ time: now });
  const offline = structuredClone(device);
  offline.convergence.connection_state = "offline";
  let summariesFail = false;
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session") return fulfillJson(route, 200, operator);
    if (path === "/api/v2/devices") return fulfillJson(route, 200, [offline]);
    if (path === "/api/v2/journal/device-summaries")
      return summariesFail
        ? fulfillJson(route, 503, {
            title: "Journal unavailable",
            status: 503,
            code: "journal_unavailable",
          })
        : fulfillJson(route, 200, {
            devices: [
              {
                device_id: device.device_id,
                last_connected_at: now - 7_200_000,
                last_disconnected_at: now - 3_600_000,
                last_disconnect_reason: "ping_silence",
                disconnects: 4,
              },
            ],
            journal: health,
          } satisfies DeviceSummaries);
    return fulfillJson(route, 404, {});
  });
  await page.goto("/devices");
  await expect(page.getByText("Journal summaries failed")).toHaveCount(0);
  await expect(deviceRow(page, "machine-01")).toContainText("4");
  summariesFail = true;
  await page.clock.runFor(10_000);
  await expect(
    page.getByRole("status").filter({ hasText: "failed to refresh" }),
  ).toContainText("2026-10-28 12:00:00 UTC+08:00");
  await expect(deviceRow(page, "machine-01")).toContainText("4");
  await expect(page.getByRole("button", { name: /Flapping/ })).toContainText(
    "Stale",
  );
});

test("timeline separates ongoing and exact disconnects, keeps repeated gaps, and tabs support Home and End", async ({
  page,
}) => {
  const offline = structuredClone(device);
  offline.convergence.connection_state = "offline";
  const base = {
    boot_id: "01923456-0000-7000-8000-0000000000b0",
    truncated_start: false,
    truncated_end: false,
    imprecise: false,
    seat_code: "A-01",
  };
  const timeline: DeviceTimeline = {
    device_id: device.device_id,
    sessions: [
      {
        ...base,
        session_id: session,
        started_at: now - 600_000,
        end: { state: "ongoing" },
        visible_from: now - 600_000,
        visible_to: now,
        duration_ms: 600_000,
      },
      {
        ...base,
        session_id: "01923456-0000-7000-8000-000000000002",
        started_at: now - 7_200_000,
        end: {
          state: "disconnected",
          at: now - 3_600_000,
          reason: "ping_silence",
        },
        visible_from: now - 7_200_000,
        visible_to: now - 3_600_000,
        duration_ms: 3_600_000,
      },
    ],
    gaps: [
      { kind: "unreadable", from: now - 1_800_000, to: now - 1_700_000 },
      { kind: "unreadable", from: now - 1_800_000, to: now - 1_600_000 },
    ],
    journal: health,
  };
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session") return fulfillJson(route, 200, operator);
    if (path === "/api/v2/devices") return fulfillJson(route, 200, [offline]);
    if (path === "/api/v2/journal/device-summaries")
      return fulfillJson(route, 200, { devices: [], journal: health });
    if (path === `/api/v2/journal/devices/${device.device_id}/timeline`)
      return fulfillJson(route, 200, timeline);
    return fulfillJson(route, 404, {});
  });
  await page.goto("/devices");
  const drawer = await openDeviceDetails(page, "machine-01");
  const tabs = drawer.getByRole("tablist", { name: "Device view" });
  const state = tabs.getByRole("tab", { name: "State" });
  const timelineTab = tabs.getByRole("tab", { name: "Timeline" });
  await state.focus();
  await page.keyboard.press("End");
  await expect(timelineTab).toHaveAttribute("aria-selected", "true");
  await expect(timelineTab).toBeFocused();
  await page.keyboard.press("Home");
  await expect(state).toHaveAttribute("aria-selected", "true");
  await expect(state).toBeFocused();
  await page.keyboard.press("End");
  const panel = drawer.getByRole("tabpanel");
  await expect(panel).toContainText("Ongoing");
  await expect(panel).toContainText("Still connected");
  await expect(panel).toContainText("Heartbeat lost");
  await expect(panel).toContainText("2026-10-28 11:00:00");
  await expect(panel.getByText("Unreadable events")).toHaveCount(2);
});

test.describe("touch device sheet", () => {
  test.use({ hasTouch: true, isMobile: true });

  test("view tabs have 48px hit areas", async ({ page }) => {
    const offline = structuredClone(device);
    offline.convergence.connection_state = "offline";
    await page.route("**/api/v2/**", (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === "/api/v2/session") return fulfillJson(route, 200, operator);
      if (path === "/api/v2/devices") return fulfillJson(route, 200, [offline]);
      return fulfillJson(route, 503, {
        title: "Journal unavailable",
        status: 503,
        code: "journal_unavailable",
      });
    });
    await page.goto("/devices");
    const drawer = await openDeviceDetails(page, "machine-01");
    for (const name of ["State", "Timeline"]) {
      const box = await drawer.getByRole("tab", { name }).boundingBox();
      expect(box?.height).toBeGreaterThanOrEqual(48);
      expect(box?.width).toBeGreaterThanOrEqual(48);
    }
  });
});
