import { expect, test, type Page } from "@playwright/test";
import { mockTargets, targetFleet } from "./fixtures/devices";
import { closeDetails } from "./fixtures/ui";

async function expectVirtualRows(page: Page, name: string, total: number) {
  const table = page.getByRole("table", { name, exact: true });
  await expect(table).toHaveAttribute("aria-rowcount", String(total + 1));
  const rows = table.locator("tbody > tr[data-index]");
  await expect.poll(() => rows.count()).toBeGreaterThan(0);
  await expect.poll(() => rows.count()).toBeLessThan(100);
  return table;
}

// Measure in the browser so Playwright's accessibility queries are not timed.
async function measureNavigation(page: Page, name: "Devices" | "Targets") {
  return page.evaluate(
    ({ path, tableName }) =>
      new Promise<number>((resolve) => {
        const start = performance.now();
        const observer = new MutationObserver(() => {
          if (!document.querySelector(`table[aria-label="${tableName}"]`))
            return;
          observer.disconnect();
          requestAnimationFrame(() =>
            requestAnimationFrame(() => resolve(performance.now() - start)),
          );
        });
        observer.observe(document.body, { childList: true, subtree: true });
        document
          .querySelector<HTMLAnchorElement>(
            `nav[aria-label="Primary navigation"] a[href="${path}"]`,
          )!
          .click();
      }),
    {
      path: name === "Devices" ? "/devices" : "/targets",
      tableName: name === "Devices" ? "Devices" : "Device targets",
    },
  );
}

test("cached navigation bounds mounted rows with 418 devices and no new response", async ({
  page,
  context,
}, testInfo) => {
  await mockTargets(context, targetFleet(418));
  await page.goto("/devices");
  await expectVirtualRows(page, "Devices", 418);
  await page
    .getByRole("navigation", { name: "Primary navigation", exact: true })
    .getByRole("link", { name: "Targets", exact: true })
    .click();
  await expectVirtualRows(page, "Device targets", 418);

  // Cached pages must render without waiting for a fresh device response.
  await context.route(/\/api\/v2\/devices(?:\?.*)?$/, () => {});
  const session = process.env.PROFILE_NAVIGATION
    ? await context.newCDPSession(page)
    : null;
  if (session) {
    await session.send("Performance.enable");
    await session.send("Profiler.enable");
  }
  const measurements = [];
  for (const name of ["Devices", "Targets", "Devices", "Targets"] as const) {
    await session?.send("Profiler.start");
    const elapsed = await measureNavigation(page, name);
    const metrics = await session?.send("Performance.getMetrics");
    const profile = await session?.send("Profiler.stop");
    const table = await expectVirtualRows(
      page,
      name === "Devices" ? "Devices" : "Device targets",
      418,
    );
    measurements.push({
      name,
      elapsed,
      mountedRows: await table.locator("tbody > tr[data-index]").count(),
      ...(metrics ? { metrics: metrics.metrics } : {}),
    });
    if (profile) {
      await testInfo.attach(`${measurements.length}-${name}.cpuprofile`, {
        body: JSON.stringify(profile.profile),
        contentType: "application/json",
      });
    }
  }
  await testInfo.attach("navigation.json", {
    body: JSON.stringify(measurements, null, 2),
    contentType: "application/json",
  });
});

for (const [path, name] of [
  ["/devices", "Devices"],
  ["/targets", "Device targets"],
] as const) {
  test(`${path} virtual rows retain keyboard access, selection and complete-data sorting`, async ({
    page,
    context,
  }) => {
    const fleet = targetFleet(418);
    await mockTargets(context, fleet);
    await page.goto(path);
    const table = await expectVirtualRows(page, name, fleet.length);
    const first = table.locator('tbody > tr[data-index="0"]');
    await first.focus();
    await first.press("End");
    const last = table.locator('tbody > tr[data-index="417"]');
    await expect(last).toBeFocused();
    await expect(last).toContainText("A-418");
    await expect(last).toBeInViewport();
    await last.press("Enter");
    await expect(page.getByRole("dialog")).toContainText(fleet[417].device_id);
    await closeDetails(page);
    await expect(last).toBeFocused();
    await last.press("End");
    await last.getByRole("checkbox").check();
    await expect(last.getByRole("checkbox")).toBeFocused();
    await last.focus();
    await last.press("Home");
    await expect(first).toBeFocused();
    await expect(first.getByRole("checkbox")).not.toBeChecked();
    await first.getByRole("checkbox").check();
    await first.focus();
    await first.press("End");
    await expect(last.getByRole("checkbox")).toBeChecked();

    // Select all applies to the full filtered set, not just mounted rows.
    const selectAll = table.getByRole("checkbox", {
      name: "Select all visible rows",
      exact: true,
    });
    await selectAll.check();
    await expect(
      page.getByText("418 devices selected", { exact: true }),
    ).toBeVisible();
    await first.focus();
    await first.press("End");
    await expect(last.getByRole("checkbox")).toBeChecked();

    await table.getByRole("button", { name: "Seat", exact: true }).click();
    await expect(
      table.getByRole("columnheader", { name: "Seat", exact: true }),
    ).toHaveAttribute("aria-sort", "ascending");
    await table.getByRole("button", { name: "Seat", exact: true }).click();
    await expect(first).toContainText("A-418");
    await first.focus();
    await first.press("End");
    await expect(last).toBeFocused();
    await expect(last).toContainText("A-01");
    await last.press("ArrowUp");
    const previous = table.locator('tbody > tr[data-index="416"]');
    await expect(previous).toBeFocused();
    await previous.press("Tab");
    await expect(previous.getByRole("checkbox")).toBeFocused();
    await previous.getByRole("checkbox").press("Shift+Tab");
    await expect(previous).toBeFocused();
    const lastControl =
      path === "/devices"
        ? previous.getByRole("button", { name: /^Actions for / })
        : previous.getByRole("checkbox");
    await lastControl.focus();
    await lastControl.press("Tab");
    await expect(last).toBeFocused();
    await last.press("Shift+Tab");
    await expect(lastControl).toBeFocused();
    await previous.focus();
    await previous.press("Home");
    await expect(first).toBeFocused();
    await page.setViewportSize({ width: 390, height: 720 });
    await first.press("End");
    await expect(last).toBeFocused();
    await expect(last).toBeInViewport();
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(390);
    await last.press("Home");
    await expect(first).toBeFocused();

    // Filtering must still find a device that was never in the initial viewport.
    await page
      .getByRole("searchbox", { name: "Search devices", exact: true })
      .fill("machine-418");
    await expect(table.locator("tbody > tr")).toHaveCount(1);
    await expect(table).toContainText("A-418");
    await expect(
      table.getByRole("checkbox", {
        name: `Select row ${fleet[417].device_id}`,
        exact: true,
      }),
    ).toBeChecked();
  });
}
