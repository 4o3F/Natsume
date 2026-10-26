import { expect, test, type Page } from "@playwright/test";
import type { PrintQueueEntry } from "../src/api/generated/types";
import {
  closeDetails,
  closeNavigation,
  openNavigation,
  refreshButton,
  refreshLabel,
} from "./fixtures/ui";

const entries: PrintQueueEntry[] = [
  {
    source_id: "source-a",
    source_name: "Primary judge",
    request_id: "shared-request",
    account: "team-1",
    team_name: "<script>alert('team')</script>",
    seat_label: "A-01",
    original_filename: "main.cpp",
    ready_sequence: 2,
    pages: 2,
    bytes: 500,
  },
  {
    source_id: "source-b",
    source_name: "Second judge",
    request_id: "shared-request",
    account: "team-2",
    team_name: "星河漫游",
    seat_label: null,
    original_filename: `${"很长的文件名".repeat(30)}.cpp`,
    ready_sequence: 5,
    pages: 3,
    bytes: 1234,
  },
];
const unavailable = {
  title: "Queue temporarily unavailable",
  status: 503,
  code: "UNAVAILABLE",
};
async function mockQueue(
  page: Page,
  reply: () => { status?: number; json: unknown },
  role = "admin",
) {
  await page.route("**/api/v2/**", (route) => {
    expect(route.request().method()).toBe("GET");
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session")
      return route.fulfill({
        json: { operator_id: "operator", username: "operator", role },
      });
    if (path === "/api/v2/print/executions")
      return route.fulfill({
        json: entries.map((entry) => ({
          ready_sequence: entry.ready_sequence,
          state: "queued",
          attempts_used: 0,
          requeue_count: 0,
          retry_at_unix_ms: null,
          last_outcome: null,
        })),
      });
    if (path === "/api/v2/print/queue") return route.fulfill(reply());
    return route.fulfill({ status: 404, json: {} });
  });
}
async function expectSummary(page: Page, documents: number, pages: number) {
  await expect(
    page.getByRole("region", { name: "Queue summary", exact: true }),
  ).toContainText(
    new RegExp(
      `${documents}\\s*admitted document${documents === 1 ? "" : "s"}\\s*·\\s*${pages}\\s*page${pages === 1 ? "" : "s"}`,
    ),
  );
}

for (const role of ["admin", "viewer"])
  for (const width of [1440, 390]) {
    test(`${role} can read the admitted list in admission order at ${width}px`, async ({
      page,
    }, testInfo) => {
      await page.setViewportSize({ width, height: 900 });
      await mockQueue(page, () => ({ json: entries }), role);
      await page.goto("/print");
      await expect(
        page.getByRole("heading", { name: "Print queue", exact: true }),
      ).toBeVisible();
      const navigation = await openNavigation(page);
      await expect(
        navigation.getByRole("link", { name: "Print queue", exact: true }),
      ).toHaveAttribute("aria-current", "page");
      if (role === "viewer")
        for (const name of ["Printers", "Print sources"])
          await expect(
            navigation.getByRole("link", { name, exact: true }),
          ).toHaveCount(0);
      await closeNavigation(page);
      await expectSummary(page, 2, 5);
      const table = page.getByRole("table", {
        name: "Admitted documents",
        exact: true,
      });
      await expect(table.locator("tbody tr")).toHaveCount(2);
      await expect(table.locator("tbody tr td:first-child")).toHaveText([
        "2",
        "5",
      ]);
      for (const entry of entries) {
        const row = table.getByRole("row").filter({ hasText: entry.account });
        for (const text of [
          entry.team_name,
          entry.source_name,
          entry.original_filename,
          entry.seat_label ?? "—",
        ])
          await expect(row).toContainText(text);
        expect((await row.boundingBox())!.height).toBeLessThan(500);
        await row.click();
        const details = page.getByRole("dialog");
        for (const text of [
          entry.team_name,
          entry.account,
          entry.source_id,
          entry.source_name,
          entry.request_id,
          entry.original_filename,
        ])
          await expect(details).toContainText(text);
        await expect(details).toContainText(entry.seat_label ?? "unassigned");
        await expect(details).toContainText(
          `${entry.bytes.toLocaleString()} bytes`,
        );
        await expect(details.locator("script")).toHaveCount(0);
        await expect(
          details.getByRole("button", { name: /cancel|reprint|reset|upload/i }),
        ).toHaveCount(0);
        await closeDetails(page);
      }
      await expect(table.locator("script")).toHaveCount(0);
      await page
        .getByRole("button", { name: "Reading this queue", exact: true })
        .click();
      await expect(
        page.getByText("Ready to claim does not mean printed.", {
          exact: true,
        }),
      ).toBeVisible();
      await expect(
        page.getByText(/admission number.*not the live retry order/),
      ).toBeVisible();
      await expect(
        page.getByText(/does not prove the paper reached the team/),
      ).toBeVisible();
      await page.keyboard.press("Escape");
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth),
      ).toBeLessThanOrEqual(width);
      await page.screenshot({
        path: testInfo.outputPath("print-queue.png"),
        fullPage: true,
      });
    });
  }

test("filtering preserves admission order and global totals, including no matches and an empty queue", async ({
  page,
}) => {
  let data = entries;
  await mockQueue(page, () => ({ json: data }));
  await page.goto("/print");
  const filter = page.getByRole("searchbox", {
    name: "Filter print queue",
    exact: true,
  });
  await expect(page.locator("tbody tr")).toHaveCount(2);
  for (const value of [
    "TEAM-2",
    "星河",
    " Second judge ",
    "source-b",
    "很长的文件名",
  ]) {
    await filter.fill(value);
    await expect(page.locator("tbody tr")).toHaveCount(1);
    await expect(page.locator("tbody tr td:first-child")).toHaveText(["5"]);
    await expectSummary(page, 2, 5);
    await expect(page.getByRole("status")).toHaveText("1 of 2 match");
  }
  await filter.fill("A-01");
  await expect(page.locator("tbody tr td:first-child")).toHaveText(["2"]);
  await filter.fill("shared-request");
  await expect(page.locator("tbody tr td:first-child")).toHaveText(["2", "5"]);
  await filter.fill("no match");
  await expect(
    page.getByText("Nothing matches", { exact: true }),
  ).toBeVisible();
  await expect(filter).toBeVisible();
  await refreshButton(page, "print queue").click();
  await expect(filter).toHaveValue("no match");
  await filter.clear();
  await expect(page.locator("tbody tr")).toHaveCount(2);
  data = [];
  await refreshButton(page, "print queue").click();
  await expectSummary(page, 0, 0);
  await expect(
    page.getByText("The queue is empty", { exact: true }),
  ).toBeVisible();
  await expect(filter).toBeVisible();
});

test("polling preserves last received rows and filters on failure, then recovers", async ({
  page,
}) => {
  await page.clock.install({ time: new Date("2030-01-01T00:00:00Z") });
  await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
  let failed = false,
    data = entries,
    reads = 0;
  await mockQueue(page, () => {
    reads++;
    return failed ? { status: 503, json: unavailable } : { json: data };
  });
  await page.goto("/print");
  const timer = page.getByRole("timer", {
    name: "Next print queue refresh",
    exact: true,
  });
  const flush = async () => {
    await page.clock.runFor(20);
    return refreshLabel(timer).textContent();
  };
  await expect.poll(flush).toMatch(/^Refresh in 10s$/);
  const filter = page.getByRole("searchbox", {
    name: "Filter print queue",
    exact: true,
  });
  await filter.fill("team-2");
  await expect
    .poll(async () => {
      await flush();
      return page.locator("tbody tr").count();
    })
    .toBe(1);
  const table = await page.getByRole("table").elementHandle();
  failed = true;
  await page.clock.runFor(10_000);
  await expect.poll(flush).toMatch(/^Retry in \d+s$/);
  await expect(page.getByRole("alert")).toContainText("Queue refresh failed");
  await expect(page.getByRole("alert")).toContainText(
    "Showing the last received queue",
  );
  await expectSummary(page, 2, 5);
  await expect(page.locator("tbody tr td:first-child")).toHaveText(["5"]);
  await expect(filter).toHaveValue("team-2");
  expect(
    await page
      .getByRole("table")
      .evaluate((element, original) => element === original, table),
  ).toBe(true);
  failed = false;
  data = [{ ...entries[1], pages: 4 }];
  await page.clock.runFor(10_000);
  await expect.poll(flush).toMatch(/^Refresh in \d+s$/);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expectSummary(page, 1, 4);
  await expect(filter).toHaveValue("team-2");
  expect(reads).toBe(3);
});

test("initial failure can be retried without reporting an empty queue as success", async ({
  page,
}) => {
  let failed = true;
  await mockQueue(page, () =>
    failed ? { status: 503, json: unavailable } : { json: [] },
  );
  await page.goto("/print");
  await expect(
    page.getByText(unavailable.title, { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("The queue is empty", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("region", { name: "Queue summary", exact: true }),
  ).toHaveCount(0);
  failed = false;
  await refreshButton(page, "print queue").click();
  await expect(
    page.getByText("The queue is empty", { exact: true }),
  ).toBeVisible();
  await expectSummary(page, 0, 0);
});

test("session expiry removes the queue and returns to login", async ({
  page,
}) => {
  let expired = false;
  const unauthorized = {
    title: "No valid session",
    status: 401,
    code: "AUTHENTICATION_FAILED",
  };
  await mockQueue(page, () =>
    expired ? { status: 401, json: unauthorized } : { json: entries },
  );
  await page.goto("/print");
  await expect(page.locator("tbody tr")).toHaveCount(2);
  expired = true;
  await page.route("**/api/v2/session", (route) =>
    route.fulfill({ status: 401, json: unauthorized }),
  );
  await refreshButton(page, "print queue").click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(
    page.getByRole("textbox", { name: "Login name", exact: true }),
  ).toBeVisible();
  await expect(page.locator("tbody tr")).toHaveCount(0);
  await expect(
    page.getByText(entries[0].team_name, { exact: true }),
  ).toHaveCount(0);
});
