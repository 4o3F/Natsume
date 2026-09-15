import { expect, test, type Route } from "@playwright/test";

test("seat refresh tracks all three sources and preserves sorting and filters", async ({
  page,
  context,
}, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.clock.install({ time: new Date("2030-01-01T00:00:00Z") });
  await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
  const seats = [
    { seat_id: "seat-b", seat_code: "B-02" },
    { seat_id: "seat-a", seat_code: "A-01" },
  ];
  const accounts = [
    {
      account_id: "account-b",
      domjudge_username: "team-b",
      credential_revision: 1,
      team: {
        seat_id: "seat-b",
        seat_code: "B-02",
        organization_id: "school-b",
        team_name_zh: "星河漫游",
        team_name_en: "Voyagers",
        school_name_zh: "示例大学",
        school_name_en: "Example University",
      },
    },
  ];
  const bindings = [
    { binding_id: "binding-a", seat_id: "seat-a", device_id: "device-a" },
  ];
  const responses = {
    "/api/v2/seats": seats,
    "/api/v2/accounts": accounts,
    "/api/v2/bindings": bindings,
  };
  const reads = new Map<string, number>();
  let holdBindings = false;
  let held: Route | undefined;
  let failPath: string | null = null;
  await page.route("**/api/v2/**", (route) => {
    expect(route.request().method()).toBe("GET");
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session")
      return route.fulfill({
        json: { operator_id: "operator", role: "admin" },
      });
    if (
      path === "/api/v2/seats" ||
      path === "/api/v2/accounts" ||
      path === "/api/v2/bindings"
    ) {
      reads.set(path, (reads.get(path) ?? 0) + 1);
      if (path === "/api/v2/bindings" && holdBindings) {
        holdBindings = false;
        held = route;
        return;
      }
      return path === failPath
        ? route.fulfill({
            status: 503,
            json: {
              code: "UNAVAILABLE",
              title: "Temporarily unavailable",
              status: 503,
            },
          })
        : route.fulfill({ json: responses[path] });
    }
    return route.fulfill({ status: 404, json: {} });
  });
  await page.goto("/seats");
  const timer = page.getByRole("timer", { name: "Next seat refresh" });
  const expectTimer = async (text: RegExp) => {
    await expect
      .poll(async () => {
        // Flush batched query notifications while the browser clock is paused.
        await page.clock.runFor(20);
        return (await timer.count()) ? timer.textContent() : "";
      })
      .toMatch(text);
  };
  await expectTimer(/^Refresh in 10s$/);
  const table = page.getByRole("table");
  await expect(table.locator("tbody tr")).toHaveCount(2);
  const timerBounds = (await timer.boundingBox())!;
  const tableBounds = (await table.boundingBox())!;
  expect(timerBounds.y + timerBounds.height).toBeLessThan(tableBounds.y);
  expect(
    Math.abs(
      timerBounds.x + timerBounds.width - tableBounds.x - tableBounds.width,
    ),
  ).toBeLessThanOrEqual(2);
  const header = table.getByRole("columnheader", {
    name: "Seat code",
    exact: true,
  });
  await header.getByRole("button").click();
  await expect(header).toHaveAttribute("aria-sort", "ascending");
  const tableNode = await table.elementHandle();
  const filter = page.getByRole("group", {
    name: "Binding state",
    exact: true,
  });
  const bound = filter.getByRole("checkbox", { name: "Bound", exact: true });
  const unbound = filter.getByRole("checkbox", {
    name: "Unbound",
    exact: true,
  });
  await bound.uncheck();
  await expect(table.locator("tbody tr td:first-child")).toHaveText(["B-02"]);
  await page.screenshot({
    path: testInfo.outputPath("seats-refresh.png"),
    fullPage: true,
  });

  await page.clock.runFor(7_000);
  await expectTimer(/^Refresh in 3s$/);
  expect([...reads.values()]).toEqual([1, 1, 1]);
  seats[0].seat_code = "B-03";
  seats.reverse();
  accounts[0].team.team_name_en = "Refreshed team";
  bindings.length = 0;
  holdBindings = true;
  await page.clock.runFor(3_000);
  await expect.poll(() => Boolean(held)).toBe(true);
  await expectTimer(/^Refreshing…$/);
  await expect(table).toContainText("Refreshed team");
  await expect(table.locator("tbody tr td:first-child")).toHaveText(["B-03"]);
  await page.clock.runFor(2_000);
  await expectTimer(/^Refreshing…$/);
  await held!.fulfill({ json: bindings });
  // Seats/accounts completed first, so the last response must not reset the
  // countdown to 10s. Allow the display's one-second tick granularity.
  await expectTimer(/^Refresh in [89]s$/);
  await expect(table.locator("tbody tr td:first-child")).toHaveText([
    "A-01",
    "B-03",
  ]);
  expect([...reads.values()]).toEqual([2, 2, 2]);

  for (const path of Object.keys(responses)) {
    failPath = path;
    await page.clock.runFor(10_000);
    await expectTimer(/^Retry in \d+s$/);
    await expect(timer).toHaveAttribute("title", /last received seat states/);
    await expect(table.locator("tbody tr td:first-child")).toHaveText([
      "A-01",
      "B-03",
    ]);
    await expect(bound).not.toBeChecked();
    await expect(header).toHaveAttribute("aria-sort", "ascending");
    expect(
      await table.evaluate(
        (element, original) => element === original,
        tableNode,
      ),
    ).toBe(true);
    failPath = null;
    await page.clock.runFor(10_000);
    await expectTimer(/^Refresh in \d+s$/);
  }

  await context.setOffline(true);
  await page.clock.runFor(10_000);
  await expectTimer(/^Refresh paused$/);
  await context.setOffline(false);
  await expectTimer(/^Refresh in \d+s$/);

  await unbound.uncheck();
  await expect(
    page.getByText("No seats match the binding filter.", { exact: true }),
  ).toBeVisible();
  await page.clock.runFor(10_000);
  await expectTimer(/^Refresh in \d+s$/);
  await expect(bound).not.toBeChecked();
  await expect(unbound).not.toBeChecked();
  await expect(page.getByRole("table")).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 900 });
  await expect(timer).toBeInViewport();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
});
