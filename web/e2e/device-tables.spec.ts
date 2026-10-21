import { expect, test, type Route } from "@playwright/test";

import {
  selectDeviceStates,
  expectDeviceStates,
  mockTargets,
  targetFleet,
} from "./fixtures/devices";

for (const path of ["/devices", "/targets"]) {
  test(`${path} expands details directly below the selected row`, async ({
    page,
    context,
  }, testInfo) => {
    const fleet = targetFleet(3);
    fleet[0].network = {
      client_ip: "192.0.2.1",
      server_observed_ip: "192.0.2.2",
      observed_at_unix_ms: 1_700_000_100_000,
    };
    await mockTargets(context, fleet);
    await page.goto(path);
    const action = path === "/devices" ? "View" : "Manage";
    const deviceRows = page.getByRole("row").filter({
      has: page.getByRole("button", { name: action, exact: true }),
    });
    const firstRow = deviceRows.filter({ hasText: "machine-01" });
    const secondRow = deviceRows.filter({ hasText: "machine-02" });
    const firstButton = firstRow.getByRole("button", {
      name: action,
      exact: true,
    });
    const secondButton = secondRow.getByRole("button", {
      name: action,
      exact: true,
    });
    const firstDetails = firstRow.locator("xpath=following-sibling::tr[1]");
    const secondDetails = secondRow.locator("xpath=following-sibling::tr[1]");

    await firstButton.click();
    await expect(firstDetails).toContainText(fleet[0].device_id);
    if (path === "/targets") {
      await expect(
        page.getByRole("button", { name: /^IP mismatch:/ }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("region", { name: "Network addresses" }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("button", { name: "Emergency", exact: true }),
      ).toHaveCount(0);
    }
    await expect(firstButton).toHaveAttribute("aria-expanded", "true");
    await expect(page.locator("tbody > tr")).toHaveCount(4);
    const detailCell = firstDetails.getByRole("cell");
    await expect(detailCell).toHaveAttribute(
      "colspan",
      String(await firstRow.getByRole("cell").count()),
    );
    for (const width of [768, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth),
      ).toBeLessThanOrEqual(width);
    }
    await page.screenshot({
      path: testInfo.outputPath("inline-details.png"),
      fullPage: true,
    });

    const sort = page.getByRole("button", { name: "Seat", exact: true });
    await sort.click();
    await sort.click();
    await expect(deviceRows.first()).toContainText("machine-03");
    await expect(firstDetails).toContainText(fleet[0].device_id);

    await secondButton.click();
    await expect(secondDetails).toContainText(fleet[1].device_id);
    await expect(firstButton).toHaveAttribute("aria-expanded", "false");
    await expect(
      page.getByText(fleet[0].device_id, { exact: true }),
    ).toHaveCount(0);
    await secondButton.click();
    await expect(secondButton).toHaveAttribute("aria-expanded", "false");
    await expect(page.locator("tbody > tr")).toHaveCount(3);

    if (path === "/targets") {
      await firstButton.click();
      await page
        .getByRole("searchbox", { name: "Search devices" })
        .fill("machine-02");
      await expect(
        page.getByText(fleet[0].device_id, { exact: true }),
      ).toHaveCount(0);
      await expect(deviceRows).toHaveCount(1);
    }
  });
}

for (const path of ["/devices", "/targets"]) {
  test(`${path} requests lifecycle filters and keeps filter controls on empty results`, async ({
    page,
    context,
  }) => {
    const fleet = targetFleet(3);
    fleet[1].state = "disabled";
    fleet[2].state = "revoked";
    const api = await mockTargets(context, fleet);
    await page.clock.install();
    await page.goto(path);
    const filter =
      path === "/devices"
        ? page.getByRole("button", { name: /^Lifecycle: / })
        : page.getByRole("group", { name: "Device state", exact: true });
    await expectDeviceStates(page, ["enabled", "disabled"]);
    await expect(page.locator("tbody tr")).toHaveCount(2);
    for (const [states, seats] of [
      [["enabled"], ["A-01"]],
      [["disabled"], ["A-02"]],
      [["revoked"], ["A-03"]],
      [
        ["enabled", "revoked"],
        ["A-01", "A-03"],
      ],
      [
        ["disabled", "revoked"],
        ["A-02", "A-03"],
      ],
      [
        ["enabled", "disabled", "revoked"],
        ["A-01", "A-02", "A-03"],
      ],
      [
        ["enabled", "disabled"],
        ["A-01", "A-02"],
      ],
    ] as const) {
      await selectDeviceStates(page, [...states]);
      await expect(page.locator("tbody tr td:first-child")).toHaveText([
        ...seats,
      ]);
      if (states.length === 1 && states[0] === "revoked")
        await expect(page.locator("tbody tr")).toHaveClass(/bg-muted\/60/);
    }
    expect(new Set(api.reads)).toEqual(
      new Set([
        "enabled,disabled",
        "enabled",
        "disabled",
        "revoked",
        "enabled,revoked",
        "disabled,revoked",
        "enabled,disabled,revoked",
      ]),
    );
    const readsBeforeEmpty = api.reads.length;
    await selectDeviceStates(page, []);
    await expect(
      page.getByText("No devices found.", { exact: true }),
    ).toBeVisible();
    const readsAfterEmpty = api.reads.length;
    // Intermediate checkbox changes may fetch; an empty lifecycle never does.
    expect(api.reads.slice(readsBeforeEmpty)).not.toContain("");
    await page.clock.runFor(10_000);
    if (path === "/devices") {
      expect(api.reads).toHaveLength(readsAfterEmpty);
      await expect(filter).toHaveAccessibleName("Lifecycle: None");
    } else {
      // Targets still polls the global Enabled scope for bulk actions.
      expect(api.reads.slice(readsAfterEmpty)).toEqual(["enabled"]);
    }
    await expect(filter).toBeVisible();
    fleet[2].state = "disabled";
    await selectDeviceStates(page, ["revoked"]);
    await expect(
      page.getByText("No devices found.", { exact: true }),
    ).toBeVisible();
    await expect(filter).toBeVisible();
    await selectDeviceStates(page, ["enabled"]);
    await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-01"]);
  });

  test(`${path} ignores a late response from a previous lifecycle filter`, async ({
    page,
    context,
  }) => {
    const fleet = targetFleet(2);
    fleet[1].state = "revoked";
    await mockTargets(context, fleet);
    let held: Route | undefined;
    await context.route(/\/api\/v2\/devices(?:\?.*)?$/, (route) => {
      if (
        new URL(route.request().url()).searchParams.get("state") ===
          "enabled" &&
        !held
      ) {
        held = route;
        return;
      }
      return route.fallback();
    });
    await page.clock.install();
    await page.goto(path);
    await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-01"]);
    await selectDeviceStates(page, ["enabled"]);
    await expect.poll(() => Boolean(held)).toBe(true);
    await selectDeviceStates(page, ["revoked"]);
    await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-02"]);
    await held!.fallback();
    await page.clock.runFor(100);
    await expectDeviceStates(page, ["revoked"]);
    await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-02"]);
  });
}
