import { expect, test, type Page, type Route } from "@playwright/test";

import {
  operator,
  device,
  fulfillJson,
  selectDeviceStates,
  mockTargets,
  selectTarget,
  openTargets,
  confirmTarget,
  confirmAll,
  targetFleet,
} from "./fixtures/devices";
import {
  closeDetails,
  deviceRow,
  scrollToTableRow,
  targetFact,
} from "./fixtures/ui";

const fleetActions = (page: Page) =>
  page.getByRole("region", { name: "All device actions", exact: true });
const receipt = (page: Page) =>
  page.getByRole("region", { name: "Target submission", exact: true });
const panel = (page: Page, name: "Session control" | "Home") =>
  page.getByRole("dialog").getByRole("region", { name, exact: true });
const epoch = (page: Page, name: "Terminate" | "Reset") =>
  targetFact(page, `${name} epoch (target / completed)`);
const reportNotice =
  "States reflect the last received device report. A submitted target is complete only when the device reports convergence.";

const refreshTimestamp = (page: Page) =>
  page
    .getByRole("paragraph")
    .filter({ hasText: /^Last successful refresh:/ })
    .locator("span")
    .first();
const reportExplanation = (page: Page) =>
  page.getByRole("paragraph").filter({ hasText: reportNotice });

async function targetStateGeometry(page: Page) {
  const dimensions = await page
    .locator('table[aria-label="Device targets"]')
    .evaluate((table) => {
      const result: Record<string, number> = {};
      const headers = Array.from(
        table.querySelectorAll<HTMLTableCellElement>("thead th"),
      );
      for (const [index, header] of headers.entries()) {
        const bounds = header.getBoundingClientRect();
        result[`column${index}Left`] = bounds.left;
        result[`column${index}Width`] = bounds.width;
      }
      for (const name of ["Session", "Home"]) {
        const header = headers.find(
          (cell) => cell.textContent?.trim() === name,
        );
        const cell = table
          .querySelector<HTMLTableRowElement>("tbody tr")
          ?.cells.item(header?.cellIndex ?? -1);
        const badge = cell?.querySelector('[data-slot="badge"]');
        if (!badge) throw new Error(`Missing ${name} state badge`);
        const bounds = badge.getBoundingClientRect();
        result[`${name}BadgeLeft`] = bounds.left;
        result[`${name}BadgeWidth`] = bounds.width;
      }
      return result;
    });
  if (await page.getByRole("dialog").isVisible()) {
    for (const name of ["Session control", "Home"] as const) {
      const badge = panel(page, name).locator('[data-slot="badge"]');
      const bounds = (await badge.boundingBox())!;
      dimensions[`${name}DetailBadgeLeft`] = bounds.x;
      dimensions[`${name}DetailBadgeWidth`] = bounds.width;
    }
  }
  return dimensions;
}

async function expectTargetStateGeometry(
  page: Page,
  before: Record<string, number>,
) {
  const current = await targetStateGeometry(page);
  for (const key of Object.keys(before)) {
    expect(Math.abs(current[key]! - before[key]!), key).toBeLessThanOrEqual(1);
  }
}

for (const [name, label, action] of [
  [
    "Show waiting screen",
    "Waiting screen",
    { kind: "set_foreground", foreground_target: "waiting" },
  ],
  [
    "Show contest desktop",
    "Contest desktop",
    { kind: "set_foreground", foreground_target: "contest" },
  ],
  ["Terminate", "Terminate", { kind: "terminate_session" }],
  ["Reset home", "Reset home", { kind: "reset_home" }],
] as const) {
  test(`bulk ${name} sends one server batch independent of the search filter`, async ({
    page,
    context,
  }) => {
    const fleet = targetFleet();
    fleet[1].convergence.connection_state = "offline";
    fleet[3].state = "disabled";
    fleet[4].state = "revoked";
    const api = await mockTargets(context, fleet);
    await page.goto("/targets");
    await expect(page.locator("tbody tr")).toHaveCount(4);
    await expect(page.locator("tbody tr").nth(1)).toHaveClass(
      /bg-destructive-subtle/,
    );
    await expect(page.locator("tbody tr").nth(3)).toHaveClass(
      /text-muted-foreground/,
    );
    await page
      .getByRole("searchbox", { name: "Search devices", exact: true })
      .fill("A-01");
    await expect(page.locator("tbody tr")).toHaveCount(1);
    await fleetActions(page)
      .getByRole("button", { name: label, exact: true })
      .click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toHaveAccessibleName(
      `${name} on all enabled devices?`,
    );
    await expect(dialog).toContainText("Estimated scope: 3 devices");
    await expect(dialog).toContainText("1 offline devices");
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    expect(api.writes).toHaveLength(0);
    await confirmAll(page, name);
    await expect(receipt(page)).toContainText("3 submitted, 0 rejected");
    expect(api.writes).toHaveLength(1);
    expect(api.writes[0]).toMatchObject({
      scope: { kind: "all_enabled" },
      action,
    });
    expect(api.writes[0].operation_id).toMatch(/^[0-9a-f-]{36}$/);
    await page
      .getByRole("searchbox", { name: "Search devices", exact: true })
      .clear();
    await expect(
      page.getByRole("img", {
        name: "Target submitted; check convergence for completion",
        exact: true,
      }),
    ).toHaveCount(3);
  });
}

test("power-off confirmation resets on cancel and submits only the online enabled scope", async ({
  page,
  context,
}) => {
  const fleet = targetFleet();
  fleet[1].convergence.connection_state = "offline";
  fleet[2].convergence.connection_state = "awaiting_fresh_state";
  fleet[3].state = "disabled";
  fleet[4].state = "revoked";
  const api = await mockTargets(context, fleet);
  await page.goto("/targets");
  await selectDeviceStates(page, ["revoked"]);
  await expect(
    page.getByRole("cell", { name: "A-05", exact: true }),
  ).toBeVisible();
  const trigger = fleetActions(page).getByRole("button", {
    name: "Power off online devices (1)",
    exact: true,
  });
  await trigger.click();
  const dialog = page.getByRole("alertdialog");
  const input = dialog.getByRole("textbox", {
    name: "Power-off confirmation",
    exact: true,
  });
  const submit = dialog.getByRole("button", {
    name: "Power off 1 device",
    exact: true,
  });
  await expect(dialog).toContainText("60 seconds");
  await expect(submit).toBeDisabled();
  await input.fill("POWER OFF 2");
  await expect(submit).toBeDisabled();
  await input.fill("POWER OFF 1");
  await expect(submit).toBeEnabled();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(api.writes).toHaveLength(0);
  await trigger.click();
  await expect(input).toHaveValue("");
  await expect(submit).toBeDisabled();
  await input.fill("POWER OFF 1");
  await submit.click();
  await expect(receipt(page)).toContainText("1 submitted, 0 rejected");
  await expect(receipt(page)).toContainText("All online enabled devices");
  expect(api.writes).toHaveLength(1);
  expect(api.writes[0]).toMatchObject({
    scope: { kind: "all_online_enabled" },
    action: { kind: "power_off" },
  });
  api.rejections.set(fleet[0].device_id, "Target epoch is exhausted");
  await trigger.click();
  await input.fill("POWER OFF 1");
  await submit.click();
  await expect(receipt(page)).toContainText("0 submitted, 1 rejected");
  await expect(
    page.getByRole("button", { name: "Retry failed devices", exact: true }),
  ).toHaveCount(0);
});

test("the Server determines the enabled device set after the preview", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(3);
  fleet[2].state = "disabled";
  const api = await mockTargets(context, fleet);
  await page.goto("/targets");
  await fleetActions(page)
    .getByRole("button", { name: "Reset home", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "Estimated scope: 2 devices",
  );
  fleet[2].state = "enabled";
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to all enabled devices", exact: true })
    .click();
  await expect(receipt(page)).toContainText("3 submitted, 0 rejected");
  expect(api.writes).toHaveLength(1);
  expect(
    fleet.map((device) => device.convergence.home.target_reset_epoch),
  ).toEqual([1, 1, 1]);
});

test("only rejected devices are retried with a new operation ID", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(3);
  const api = await mockTargets(context, fleet);
  api.rejections.set(fleet[1].device_id, "Target epoch is exhausted");
  await page.goto("/targets");
  await confirmAll(page, "Reset home");
  await expect(receipt(page)).toContainText("2 submitted, 1 rejected");
  await expect(
    deviceRow(page, "machine-02").getByRole("img", {
      name: "Rejected: Target epoch is exhausted",
      exact: true,
    }),
  ).toBeVisible();
  api.rejections.clear();
  await confirmTarget(page, "Retry failed devices");
  await expect(receipt(page)).toContainText("1 submitted, 0 rejected");
  expect(api.writes).toHaveLength(2);
  expect(api.writes[1].operation_id).not.toBe(api.writes[0].operation_id);
  expect(api.writes[1]).toMatchObject({
    action: { kind: "reset_home" },
    scope: { kind: "devices", device_ids: [fleet[1].device_id] },
  });
  expect(
    fleet.map((device) => device.convergence.home.target_reset_epoch),
  ).toEqual([1, 1, 1]);
});

test("submission results have a compact summary and filterable device receipts", async ({
  page,
  context,
}, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 1050 });
  const fleet = targetFleet(8);
  fleet[2].convergence.connection_state = "offline";
  fleet[2].convergence.session_control.status = "awaiting_actual";
  const api = await mockTargets(context, fleet);
  api.rejections.set(fleet[1].device_id, "Target epoch is exhausted");
  api.rejections.set(
    fleet[5].device_id,
    "Device state does not allow this target",
  );
  await page.goto("/targets");
  await confirmAll(page, "Reset home");
  const summary = receipt(page);
  await expect(summary).toContainText("6 submitted, 2 rejected");
  await expect(
    summary.getByText("Partially submitted", { exact: true }),
  ).toBeVisible();
  for (const [label, count] of [
    ["Devices in scope", 8],
    ["Submitted", 6],
    ["Rejected", 2],
  ] as const)
    await expect(
      summary.getByText(label, { exact: true }).locator("..").locator("dd"),
    ).toHaveText(String(count));
  await expect(summary.getByRole("table")).not.toBeVisible();
  await expect(
    summary.getByText(
      "Server acceptance does not confirm completion. Check device convergence.",
      { exact: true },
    ),
  ).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("targets-summary.png"),
    fullPage: true,
  });
  await summary.locator("summary").click();
  await expect(summary.getByRole("table").locator("tbody tr")).toHaveCount(8);
  const submitted = summary
    .getByRole("table")
    .getByText("Submitted", { exact: true })
    .first();
  const rejected = summary
    .getByRole("table")
    .getByText("Rejected", { exact: true })
    .first();
  for (const status of [submitted, rejected]) {
    await expect(status).toHaveCSS("border-top-width", "0px");
    await expect(status).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  }
  const submittedBounds = (await submitted.boundingBox())!,
    rejectedBounds = (await rejected.boundingBox())!;
  expect(rejectedBounds.height).toBe(submittedBounds.height);
  expect(rejectedBounds.x).toBe(submittedBounds.x);
  expect((await rejected.locator("svg").boundingBox())!.x).toBe(
    (await submitted.locator("svg").boundingBox())!.x,
  );
  await summary.screenshot({
    path: testInfo.outputPath("targets-aligned-results.png"),
  });
  await summary
    .getByRole("button", { name: "Rejected (2)", exact: true })
    .click();
  await expect(summary.getByRole("table").locator("tbody tr")).toHaveCount(2);
  await expect(
    summary.getByRole("cell", {
      name: "Target epoch is exhausted",
      exact: true,
    }),
  ).toBeVisible();
  await selectDeviceStates(page, ["disabled"]);
  await expect(
    summary.getByRole("cell", { name: "A-02 machine-02", exact: true }),
  ).toBeVisible();
  await summary
    .getByRole("button", { name: "All results (8)", exact: true })
    .click();
  await expect(summary.getByRole("table").locator("tbody tr")).toHaveCount(8);
  await selectDeviceStates(page, ["enabled", "disabled"]);
  for (const width of [1024, 390]) {
    await page.setViewportSize({ width, height: 1050 });
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    await page.screenshot({
      path: testInfo.outputPath(`targets-results-${width}.png`),
      fullPage: true,
    });
  }
  api.rejections.clear();
  await confirmTarget(page, "Retry failed devices");
  await expect(
    summary.getByText("Targets submitted", { exact: true }),
  ).toBeVisible();
  await expect(summary.getByRole("table")).not.toBeVisible();
  await summary.locator("summary").click();
  await expect(summary.getByRole("table").locator("tbody tr")).toHaveCount(2);
  expect(api.writes[1].scope).toEqual({
    kind: "devices",
    device_ids: [fleet[1].device_id, fleet[5].device_id],
  });
  await page.setViewportSize({ width: 1440, height: 1050 });
  await summary.locator("summary").click();
  await summary.screenshot({
    path: testInfo.outputPath("targets-submitted.png"),
  });
  await selectTarget(page, fleet[0].device_id);
  await expect(targetFact(page, "Target foreground")).toHaveText("contest");
  await page
    .getByRole("dialog")
    .screenshot({ path: testInfo.outputPath("targets-device-details.png") });
});

test("empty and fully rejected submission receipts are not shown as success", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(2);
  const api = await mockTargets(context, fleet);
  for (const device of fleet)
    api.rejections.set(device.device_id, "Target epoch is exhausted");
  await page.goto("/targets");
  await confirmAll(page, "Reset home");
  const summary = receipt(page);
  await expect(
    summary.getByText("Submission rejected", { exact: true }),
  ).toBeVisible();
  await expect(summary).toContainText("0 submitted, 2 rejected");
  await fleetActions(page)
    .getByRole("button", { name: "Reset home", exact: true })
    .click();
  for (const device of fleet) device.state = "disabled";
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to all enabled devices", exact: true })
    .click();
  await expect(
    summary.getByText("No devices in scope", { exact: true }),
  ).toBeVisible();
  await expect(summary).toContainText("No eligible devices were included");
  await expect(summary.locator("summary")).toHaveCount(0);
  await expect(
    summary.getByRole("button", { name: "Retry failed devices", exact: true }),
  ).toHaveCount(0);
});

test("a lost response survives refresh and replays the same operation without another reset", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(2);
  const api = await mockTargets(context, fleet);
  let loseResponse = true;
  await context.route("**/api/v2/target-submissions", async (route) => {
    if (!loseResponse) return route.fallback();
    api.apply(route.request().postDataJSON());
    loseResponse = false;
    await route.abort("failed");
  });
  await page.clock.install();
  await page.goto("/targets");
  await confirmAll(page, "Reset home");
  await expect(page.getByRole("alert")).toContainText(
    "Submission result not confirmed",
  );
  await expect(
    fleetActions(page).getByRole("button", { name: "Terminate", exact: true }),
  ).toBeDisabled();
  await page.clock.runFor(10_000);
  expect(api.writes).toHaveLength(1);
  await page.reload();
  await expect(page.getByRole("alert")).toContainText(
    "Submission result not confirmed",
  );
  await selectTarget(page, fleet[1].device_id);
  await expect(
    panel(page, "Home").getByRole("button", {
      name: "Reset home",
      exact: true,
    }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Retry original request", exact: true })
    .click();
  await expect(receipt(page)).toContainText("2 submitted, 0 rejected");
  expect(api.writes).toHaveLength(2);
  expect(api.writes[1]).toEqual(api.writes[0]);
  expect(
    fleet.map((device) => device.convergence.home.target_reset_epoch),
  ).toEqual([1, 1]);
  await expect(
    panel(page, "Home").getByRole("button", {
      name: "Reset home",
      exact: true,
    }),
  ).toBeEnabled();
  await confirmTarget(page, "Reset home");
  await expect(epoch(page, "Reset")).toHaveText("2 / none");
  expect(api.writes[2].operation_id).not.toBe(api.writes[0].operation_id);
});

test("one pending submission blocks other device writes but not browsing", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(2);
  const api = await mockTargets(context, fleet);
  let held: Route | undefined;
  await context.route("**/api/v2/target-submissions", (route) => {
    held = route;
  });
  await page.goto("/targets");
  await selectTarget(page, fleet[0].device_id);
  await page
    .getByRole("button", { name: "Show waiting screen", exact: true })
    .click();
  await expect.poll(() => Boolean(held)).toBe(true);
  await selectTarget(page, fleet[1].device_id);
  await expect(targetFact(page, "Target foreground")).toHaveText("contest");
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeDisabled();
  await closeDetails(page);
  await expect(
    fleetActions(page).getByRole("button", { name: "Reset home", exact: true }),
  ).toBeDisabled();
  await selectTarget(page, fleet[1].device_id);
  await held!.fallback();
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeEnabled();
  await expect(targetFact(page, "Target foreground")).toHaveText("contest");
  await expect(receipt(page)).toContainText("A-01");
  expect(api.writes).toHaveLength(1);
  expect(api.writes[0].scope).toEqual({
    kind: "devices",
    device_ids: [fleet[0].device_id],
  });
  await selectTarget(page, fleet[0].device_id);
  await expect(targetFact(page, "Target foreground")).toHaveText("waiting");
});

for (const width of [1024, 1440]) {
  test(`targets retain a 200-device sorted viewport on refresh at ${width}`, async ({
    page,
    context,
  }, testInfo) => {
    const fleet = targetFleet(200);
    fleet[1].convergence.connection_state = "offline";
    fleet[1].convergence.session_control.actual = null;
    fleet[1].convergence.session_control.status = "awaiting_actual";
    fleet[1].convergence.home.actual = null;
    fleet[1].convergence.home.status = "awaiting_actual";
    fleet[2].state = "disabled";
    await mockTargets(context, fleet);
    await page.setViewportSize({ width, height: 900 });
    await page.clock.install();
    await page.goto("/targets");
    const table = page.getByRole("table", {
      name: "Device targets",
      exact: true,
    });
    await expect(table).toHaveAttribute("aria-rowcount", "201");
    await expect
      .poll(() => table.locator("tbody > tr[data-index]").count())
      .toBeLessThan(100);
    await page.getByRole("button", { name: "Seat", exact: true }).click();
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    await page.screenshot({ path: testInfo.outputPath("targets.png") });
    const row = deviceRow(page, fleet[150].machine_hardware_id);
    await scrollToTableRow(page, table, 150);
    await expect(row).toBeVisible();
    const before = await page.evaluate(() => window.scrollY);
    expect(before).toBeGreaterThan(0);
    fleet[150].convergence.session_control.target!.foreground_target =
      "waiting";
    await page.clock.runFor(10_000);
    const foreground = row.getByRole("cell").nth(3);
    await expect(foreground).toContainText("waiting");
    await expect(foreground).toContainText("contest");
    expect(
      Math.abs((await page.evaluate(() => window.scrollY)) - before),
    ).toBeLessThan(2);
    await expect(
      table.getByRole("columnheader", { name: "Seat", exact: true }),
    ).toHaveAttribute("aria-sort", "ascending");
  });
}

for (const width of [390, 1440]) {
  for (const surface of ["list", "details"] as const) {
    test(`targets ${surface} stays still during a held refresh at ${width}px`, async ({
      page,
      context,
    }) => {
      const fleet = targetFleet(60);
      await mockTargets(context, fleet);
      await page.setViewportSize({ width, height: 720 });
      await page.clock.install({ time: new Date("2030-01-01T01:01:00Z") });
      await page.goto("/targets");
      const table = page.locator('table[aria-label="Device targets"]');
      await expect(table.locator("tbody tr")).toHaveCount(60);
      if (surface === "details") {
        await selectTarget(page, fleet[0].device_id);
        await page.getByRole("dialog").evaluate(async (element) => {
          await Promise.all(
            element.getAnimations().map((animation) => animation.finished),
          );
        });
      }
      await page.evaluate(() => document.fonts.ready.then(() => undefined));
      const timestamp = refreshTimestamp(page);
      await expect(reportExplanation(page)).toBeVisible();
      const drawerBody = page
        .getByRole("dialog")
        .locator('[data-slot="sheet-body"]');
      const tableNode = await table.elementHandle();
      const geometry = async (): Promise<Record<string, number>> => {
        const dimensions = await targetStateGeometry(page);
        const tableTop = await table.evaluate(
          (element) => element.getBoundingClientRect().top + window.scrollY,
        );
        const headerHeight = await page
          .locator("main header")
          .first()
          .evaluate((element) => element.getBoundingClientRect().height);
        const pageScroll = await page.evaluate(() => window.scrollY);
        if (surface === "list")
          return { ...dimensions, tableTop, headerHeight, pageScroll };
        return {
          ...dimensions,
          tableTop,
          headerHeight,
          pageScroll,
          drawerScroll: await drawerBody.evaluate(
            (element) => element.scrollTop,
          ),
          panelTop: await panel(page, "Session control").evaluate(
            (element) => element.getBoundingClientRect().top,
          ),
        };
      };
      let held: Route | undefined;
      await context.route(/\/api\/v2\/devices(?:\?.*)?$/, (route) => {
        if (
          new URL(route.request().url()).searchParams.get("state") ===
          "enabled,disabled"
        ) {
          held = route;
          return;
        }
        return route.fallback();
      });
      for (const scrolled of [false, true]) {
        if (scrolled) {
          if (surface === "list") {
            await table.locator("tbody tr").nth(40).scrollIntoViewIfNeeded();
            expect(await page.evaluate(() => window.scrollY)).toBeGreaterThan(
              0,
            );
          } else {
            await page.setViewportSize({ width, height: 520 });
            await drawerBody.evaluate((element) => {
              element.scrollTop = 120;
            });
            expect(
              await drawerBody.evaluate((element) => element.scrollTop),
            ).toBeGreaterThan(0);
          }
        }
        const before = await geometry();
        const receivedAt = await timestamp.textContent();
        const expectStable = async () => {
          const current = await geometry();
          for (const key of Object.keys(before)) {
            expect(
              Math.abs(current[key]! - before[key]!),
              `${surface} ${key} at ${width}px`,
            ).toBeLessThanOrEqual(1);
          }
          expect(
            await table.evaluate(
              (element, original) => element === original,
              tableNode,
            ),
          ).toBe(true);
        };
        held = undefined;
        await page.clock.runFor(10_000);
        await expect.poll(() => Boolean(held)).toBe(true);
        const refreshing = page
          .getByRole("status")
          .filter({ hasText: "Refreshing. Showing previous result." });
        await expect(refreshing).toHaveText(
          "Refreshing. Showing previous result.",
        );
        await expectStable();
        await expect(timestamp).toHaveText(receivedAt!);
        await expect(refreshing).toHaveCSS("position", "absolute");
        const statusBounds = (await refreshing.boundingBox())!;
        expect(statusBounds.width).toBeLessThanOrEqual(1);
        expect(statusBounds.height).toBeLessThanOrEqual(1);
        await page.clock.runFor(3_000);
        await expectStable();
        await fulfillJson(held!, 200, fleet);
        await expect(timestamp).not.toHaveText(receivedAt!);
        await expect(timestamp.locator("..").getByRole("status")).toBeEmpty();
        await expectStable();
      }
      const stableStates = await targetStateGeometry(page);
      held = undefined;
      await page.clock.runFor(10_000);
      await expect.poll(() => Boolean(held)).toBe(true);
      await fulfillJson(held!, 503, {
        title: "Unavailable",
        status: 503,
        code: "SERVICE_UNAVAILABLE",
      });
      await expect(page.getByRole("alert")).toContainText(
        "Showing the last successful result; it may be outdated.",
      );
      await expectTargetStateGeometry(page, stableStates);
      let heldSubmission: Route | undefined;
      await context.route("**/api/v2/target-submissions", (route) => {
        heldSubmission = route;
      });
      if (surface === "details") await confirmTarget(page, "Reset home");
      else await confirmAll(page, "Reset home");
      await expect.poll(() => Boolean(heldSubmission)).toBe(true);
      await expect(receipt(page)).toContainText(
        "Submitting target. Showing previous device states.",
      );
      await expectTargetStateGeometry(page, stableStates);
      await heldSubmission!.abort("failed");
      await expect(
        page
          .getByRole("alert")
          .filter({ hasText: "Submission result not confirmed" }),
      ).toBeVisible();
      await expectTargetStateGeometry(page, stableStates);
    });
  }
}

for (const width of [390, 1440]) {
  for (const submitted of [false, true]) {
    test(`targets keeps background feedback when opening details at ${width}px with receipt ${submitted}`, async ({
      page,
      context,
    }) => {
      const fleet = targetFleet(3);
      await mockTargets(context, fleet);
      await page.setViewportSize({ width, height: 720 });
      await page.clock.install();
      await page.goto("/targets");
      await expect(
        page.locator('table[aria-label="Device targets"] tbody tr'),
      ).toHaveCount(3);
      if (submitted) {
        await confirmAll(page, "Reset home");
        await expect(receipt(page)).toContainText("3 submitted, 0 rejected");
        await receipt(page).locator("summary").click();
        await expect(receipt(page).getByRole("table")).toBeVisible();
      }
      await page.evaluate(() => document.fonts.ready.then(() => undefined));
      const main = page.locator("main");
      const timestamp = main.getByText(/^Last successful refresh:/);
      const timestampNode = await timestamp.elementHandle();
      const feedback = timestamp.locator("../..");
      const geometry = async () => {
        const bounds = (await feedback.boundingBox())!;
        return {
          feedbackTop: bounds.y + (await page.evaluate(() => window.scrollY)),
          feedbackHeight: bounds.height,
          tableTop: await main
            .locator('table[aria-label="Device targets"]')
            .evaluate(
              (element) => element.getBoundingClientRect().top + window.scrollY,
            ),
        };
      };
      const before = await geometry();
      const expectStable = async () => {
        await expect(timestamp).toBeVisible();
        await expect(
          main.getByText(reportNotice, { exact: true }),
        ).toBeVisible();
        expect(
          await timestamp.evaluate(
            (element, original) => element === original,
            timestampNode,
          ),
        ).toBe(true);
        const current = await geometry();
        for (const key of Object.keys(before) as (keyof typeof before)[]) {
          expect(Math.abs(current[key] - before[key]), key).toBeLessThanOrEqual(
            1,
          );
        }
      };
      await selectTarget(page, fleet[0].device_id);
      await page.getByRole("dialog").evaluate(async (element) => {
        await Promise.all(
          element.getAnimations().map((animation) => animation.finished),
        );
      });
      await expectStable();
      await expect(refreshTimestamp(page)).toHaveCount(1);
      await expect(reportExplanation(page)).toHaveCount(1);
      if (submitted) await expect(receipt(page)).toHaveCount(1);
      await closeDetails(page);
      await expectStable();
      await expect(refreshTimestamp(page)).toHaveCount(1);
      if (submitted)
        await expect(receipt(page).getByRole("table")).toBeVisible();
    });
  }
}

test("disabled devices and viewers cannot submit targets", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(2);
  fleet[0].state = "disabled";
  const api = await mockTargets(context, fleet);
  await page.goto("/targets");
  await selectTarget(page, fleet[0].device_id);
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeDisabled();
  await context.route("**/api/v2/session", (route) =>
    fulfillJson(route, 200, { ...operator, role: "viewer" }),
  );
  await page.reload();
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await expect(fleetActions(page)).toHaveCount(0);
  await expect(page.getByRole("checkbox", { name: /^Select/ })).toHaveCount(0);
  await selectTarget(page, fleet[0].device_id);
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toHaveCount(0);
  expect(api.writes).toHaveLength(0);
});

test("target controls send the unified generated API request", async ({
  page,
  context,
}) => {
  const current = structuredClone(device);
  const api = await mockTargets(context, current);
  await openTargets(page);
  for (const [name, foreground] of [
    ["Show waiting screen", "waiting"],
    ["Show contest desktop", "contest"],
  ]) {
    await page.getByRole("button", { name, exact: true }).click();
    await expect(targetFact(page, "Target foreground")).toHaveText(foreground);
  }
  await confirmTarget(page, "Terminate");
  await expect(epoch(page, "Terminate")).toHaveText("1 / none");
  await confirmTarget(page, "Reset home");
  await expect(epoch(page, "Reset")).toHaveText("1 / none");
  expect(api.writes.map((request) => request.action)).toEqual([
    { kind: "set_foreground", foreground_target: "waiting" },
    { kind: "set_foreground", foreground_target: "contest" },
    { kind: "terminate_session" },
    { kind: "reset_home" },
  ]);
  expect(
    api.writes.every(
      (request) =>
        request.scope.kind === "devices" &&
        request.scope.device_ids.length === 1 &&
        request.scope.device_ids[0] === device.device_id,
    ),
  ).toBe(true);
  expect(new Set(api.writes.map((request) => request.operation_id)).size).toBe(
    4,
  );
});

test("ready desktops still wait for binding before contest presentation", async ({
  page,
  context,
}) => {
  const current = structuredClone(device);
  current.convergence.binding = {
    status: "converged",
    target: {
      state: "unbound",
      negotiation_id: "01912345-6789-7abc-8def-0123456789ae",
      evaluation: null,
    },
    actual: {
      assignment_state: "absent",
      credential_state: "absent",
      context: null,
    },
  };
  current.convergence.session_control.status = "reconciling";
  current.convergence.session_control.actual!.foreground = "waiting";
  await mockTargets(context, current);
  await openTargets(page);
  for (const [label, value] of [
    ["Target foreground", "contest"],
    ["Actual foreground", "waiting"],
    ["Actual session", "running"],
    ["Waiting display ready", "true"],
    ["Contest desktop ready", "true"],
  ])
    await expect(targetFact(page, label)).toHaveText(value);
  const session = panel(page, "Session control");
  await expect(
    session.getByText(
      "Waiting for binding before showing the contest desktop.",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(session.getByText("reconciling", { exact: true })).toBeVisible();
  await expect(session.getByText("converged", { exact: true })).toHaveCount(0);
});

test("target submission stays distinct from reported progress, failure and completion", async ({
  page,
  context,
}) => {
  const current = structuredClone(device);
  await mockTargets(context, current);
  await page.clock.install();
  await openTargets(page);
  await confirmTarget(page, "Terminate");
  await expect(epoch(page, "Terminate")).toHaveText("1 / none");
  await confirmTarget(page, "Reset home");
  await expect(epoch(page, "Reset")).toHaveText("1 / none");
  const sessionPanel = panel(page, "Session control"),
    homePanel = panel(page, "Home");
  await expect(
    page.getByRole("status").filter({ hasText: "submitted," }),
  ).toHaveText(
    "1 submitted, 0 rejected. Check device convergence for completion.",
  );
  await expect(
    sessionPanel.getByText("reconciling", { exact: true }),
  ).toBeVisible();
  await expect(homePanel.getByText("drifted", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("dialog").getByText("converged", { exact: true }),
  ).toHaveCount(0);
  const { session_control: session, home } = current.convergence;
  session.actual = {
    foreground: "waiting",
    waiting_ready: true,
    contest_ready: false,
    session_state: "terminating",
    completed_terminate_epoch: null,
  };
  home.actual = { state: "resetting", completed_reset_epoch: null };
  home.status = "reconciling";
  await page.clock.runFor(10_000);
  await expect(targetFact(page, "Actual session")).toHaveText("terminating");
  await expect(targetFact(page, "Actual home")).toHaveText("resetting");
  await expect(
    homePanel.getByText("reconciling", { exact: true }),
  ).toBeVisible();
  session.actual.session_state = "error";
  session.status = "failed";
  home.actual.state = "recovery_required";
  home.status = "failed";
  await page.clock.runFor(10_000);
  await expect(sessionPanel.getByText("failed", { exact: true })).toBeVisible();
  await expect(
    sessionPanel.getByText("The device reported a session error.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(homePanel.getByText("failed", { exact: true })).toBeVisible();
  await expect(
    homePanel.getByText("The device requires Home recovery.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("dialog").getByText("converged", { exact: true }),
  ).toHaveCount(0);
  session.actual = {
    session_state: "running",
    completed_terminate_epoch: 1,
    foreground: "contest",
    waiting_ready: true,
    contest_ready: true,
  };
  session.status = "converged";
  home.actual = { state: "steady", completed_reset_epoch: 1 };
  home.status = "converged";
  current.convergence.received_at_unix_ms = 1_700_000_200_000;
  await page.clock.runFor(10_000);
  await expect(epoch(page, "Terminate")).toHaveText("1 / 1");
  await expect(epoch(page, "Reset")).toHaveText("1 / 1");
  await expect(
    page.getByRole("dialog").getByText("converged", { exact: true }),
  ).toHaveCount(2);
  const reportTime = await page.evaluate(
    (timestamp) => new Date(timestamp).toLocaleString(),
    current.convergence.received_at_unix_ms,
  );
  await expect(
    page.getByText(`Last report ${reportTime}`, { exact: true }),
  ).toBeVisible();
});

test("offline and reconnecting devices wait for fresh Actual after submission", async ({
  page,
  context,
}) => {
  const current = structuredClone(device),
    state = current.convergence;
  state.connection_state = "offline";
  state.received_at_unix_ms = null;
  state.session_control = {
    target: null,
    actual: null,
    status: "awaiting_actual",
  };
  state.home.actual = null;
  state.home.status = "awaiting_actual";
  await mockTargets(context, current);
  await page.clock.install();
  await openTargets(page);
  await expect(targetFact(page, "Target foreground")).toHaveText(
    "not initialized",
  );
  await confirmTarget(page, "Reset home");
  await expect(epoch(page, "Reset")).toHaveText("1 / none");
  await expect(
    page.getByRole("status").filter({ hasText: "submitted," }),
  ).toBeVisible();
  await expect(
    page.getByText("Waiting for a connection and a fresh state report.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByText("Last report not received", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("dialog").getByText("awaiting actual", { exact: true }),
  ).toHaveCount(2);
  await expect(
    page.getByRole("dialog").getByText("converged", { exact: true }),
  ).toHaveCount(0);
  state.connection_state = "awaiting_fresh_state";
  await page.clock.runFor(10_000);
  await expect(
    page.getByText("Connected. Waiting for a fresh state report.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(targetFact(page, "Actual home")).toHaveText("not received");
  await expect(epoch(page, "Reset")).toHaveText("1 / none");
  await expect(
    page.getByRole("dialog").getByText("awaiting actual", { exact: true }),
  ).toHaveCount(2);
});

test("refresh failures mark cached completion as old and retry reads the submitted target", async ({
  page,
  context,
}) => {
  const current = structuredClone(device);
  await mockTargets(context, current);
  await page.clock.install();
  await openTargets(page);
  const lastRefresh = await refreshTimestamp(page).textContent();
  let failRefresh = true;
  await context.route(/\/api\/v2\/devices(?:\?.*)?$/, (route) =>
    failRefresh
      ? fulfillJson(route, 503, {
          code: "unavailable",
          title: "Temporarily unavailable",
          status: 503,
        })
      : route.fallback(),
  );
  await page.clock.runFor(10_000);
  await expect(page.getByRole("alert")).toContainText("Refresh failed");
  await expect(page.getByRole("alert")).toContainText(
    "Showing the last successful result; it may be outdated.",
  );
  await expect(refreshTimestamp(page)).toHaveText(lastRefresh!);
  await expect(reportExplanation(page)).toBeVisible();
  await expect(
    page.getByRole("dialog").getByText("converged", { exact: true }),
  ).toHaveCount(2);
  await confirmTarget(page, "Reset home");
  await expect(
    page.getByRole("status").filter({ hasText: "submitted," }),
  ).toBeVisible();
  await expect(epoch(page, "Reset")).toHaveText("none / none");
  await expect(
    page.getByRole("dialog").getByText("converged", { exact: true }),
  ).toHaveCount(2);
  await expect(receipt(page)).toContainText(
    "Server acceptance does not confirm completion. Check device convergence.",
  );
  await expect(page.getByRole("alert")).toContainText("Refresh failed");
  failRefresh = false;
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(epoch(page, "Reset")).toHaveText("1 / none");
  await expect(
    panel(page, "Home").getByText("drifted", { exact: true }),
  ).toBeVisible();
  await expect(refreshTimestamp(page)).not.toHaveText(lastRefresh!);
});

test("a delayed pre-mutation poll cannot replace the refreshed target", async ({
  page,
  context,
}) => {
  const current = structuredClone(device);
  await mockTargets(context, current);
  await page.clock.install();
  await openTargets(page);
  const heldReads: Route[] = [];
  await context.route(/\/api\/v2\/devices(?:\?.*)?$/, (route) => {
    if (
      new URL(route.request().url()).searchParams.get("state") !==
      "enabled,disabled"
    )
      return route.fallback();
    heldReads.push(route);
  });
  await page.clock.runFor(10_000);
  await expect.poll(() => heldReads.length).toBe(1);
  await expect(
    page
      .getByRole("status")
      .filter({ hasText: "Refreshing. Showing previous result." }),
  ).toHaveText("Refreshing. Showing previous result.");
  await confirmTarget(page, "Reset home");
  await expect.poll(() => heldReads.length).toBe(2);
  for (const name of ["Reset home", "Terminate", "Show waiting screen"])
    await expect(page.getByRole("button", { name, exact: true })).toBeEnabled();
  await expect(reportExplanation(page)).toBeVisible();
  await expect(
    page.getByRole("dialog").getByText("converged", { exact: true }),
  ).toHaveCount(2);
  await fulfillJson(heldReads[1], 200, [current]);
  await expect(epoch(page, "Reset")).toHaveText("1 / none");
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toBeEnabled();
  await fulfillJson(heldReads[0], 200, [device]);
  await page.clock.runFor(100);
  await expect(epoch(page, "Reset")).toHaveText("1 / none");
});

test("foreground targets follow another operator within one polling interval", async ({
  page,
  context,
  browser,
}) => {
  const current = structuredClone(device);
  await mockTargets(context, current);
  await page.clock.install();
  await openTargets(page);
  const otherContext = await browser.newContext();
  try {
    await mockTargets(
      otherContext,
      current,
      "01912345-6789-7abc-8def-0123456789ac",
    );
    const other = await otherContext.newPage();
    await other.clock.install();
    await openTargets(other);
    await confirmTarget(other, "Terminate");
    await expect(epoch(other, "Terminate")).toHaveText("1 / none");
    await confirmTarget(other, "Reset home");
    await expect(epoch(other, "Reset")).toHaveText("1 / none");
    await page.bringToFront();
    await page.clock.runFor(10_000);
    await expect(epoch(page, "Terminate")).toHaveText("1 / none");
    await expect(epoch(page, "Reset")).toHaveText("1 / none");
    await confirmTarget(page, "Terminate");
    await expect(epoch(page, "Terminate")).toHaveText("2 / none");
    await other.bringToFront();
    await other.clock.runFor(10_000);
    await expect(epoch(other, "Terminate")).toHaveText("2 / none");
  } finally {
    await otherContext.close();
  }
});

test("Targets all-device actions retain global Enabled scope even with an empty revoked filter", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(2);
  const api = await mockTargets(context, fleet);
  await page.goto("/targets");
  await selectDeviceStates(page, ["revoked"]);
  await expect(
    page.getByText("No devices found", { exact: true }),
  ).toBeVisible();
  await expect(fleetActions(page)).toContainText("2 enabled");
  await expect(
    fleetActions(page).getByRole("button", { name: "Reset home", exact: true }),
  ).toBeEnabled();
  await confirmAll(page, "Reset home");
  await expect(receipt(page)).toContainText("2 submitted, 0 rejected");
  expect(api.writes).toHaveLength(1);
  expect(api.writes[0].scope).toEqual({ kind: "all_enabled" });
  expect(api.reads).toContain("enabled");
  await expect(
    page.getByText("No devices found", { exact: true }),
  ).toBeVisible();
});
