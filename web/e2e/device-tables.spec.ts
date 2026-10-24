import { expect, test, type Route } from "@playwright/test";
import {
  selectDeviceStates,
  expectDeviceStates,
  mockTargets,
  targetFleet,
} from "./fixtures/devices";
import {
  closeDetails,
  deviceRow,
  openDeviceDetails,
  seatCells,
} from "./fixtures/ui";

for (const path of ["/devices", "/targets"]) {
  test(`${path} opens keyboard-accessible details without inserting table rows`, async ({
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
    const first = deviceRow(page, "machine-01");
    await first.focus();
    await first.press("Enter");
    const drawer = page.getByRole("dialog");
    await expect(drawer).toContainText(fleet[0].device_id);
    if (path === "/targets") {
      await expect(
        drawer.getByRole("region", { name: "Network addresses", exact: true }),
      ).toHaveCount(0);
      await expect(
        drawer.getByRole("button", { name: /^IP mismatch:/ }),
      ).toHaveCount(0);
      await expect(
        drawer.getByRole("button", { name: "Emergency", exact: true }),
      ).toHaveCount(0);
    }
    await page.screenshot({ path: testInfo.outputPath("device-details.png") });
    await closeDetails(page);
    await expect(first).toBeFocused();
    await expect(page.locator("tbody > tr")).toHaveCount(3);
    await page.getByRole("button", { name: "Seat", exact: true }).click();
    await page.getByRole("button", { name: "Seat", exact: true }).click();
    await expect(deviceRow(page, "machine-03")).toBeVisible();
    await expect((await seatCells(page)).first()).toHaveText("A-03");
    await openDeviceDetails(page, "machine-02");
    await expect(drawer).toContainText(fleet[1].device_id);
    await expect(drawer).not.toContainText(fleet[0].device_id);
    await closeDetails(page);
    const second = deviceRow(page, "machine-02");
    await second.focus();
    await second.press("Space");
    await expect(drawer).toContainText(fleet[1].device_id);
    await closeDetails(page);
    const selection = second.getByRole("checkbox", {
      name: `Select row ${fleet[1].device_id}`,
      exact: true,
    });
    await selection.focus();
    await selection.press("Space");
    await expect(selection).toBeChecked();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await page
      .getByRole("searchbox", { name: "Search devices", exact: true })
      .fill("machine-02");
    await expect(page.locator("tbody tr")).toHaveCount(1);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    for (const width of [768, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth),
      ).toBeLessThanOrEqual(width);
    }
  });

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
        ? page.getByRole("button", { name: /^Lifecycle:/ })
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
      await expect(await seatCells(page)).toHaveText([...seats]);
      if (states.length === 1 && states[0] === "revoked")
        await expect(page.locator("tbody tr")).toHaveClass(
          /text-muted-foreground/,
        );
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
    const before = api.reads.length;
    await selectDeviceStates(page, []);
    await expect(
      page.getByText(
        path === "/devices" ? "No devices match" : "No devices found",
        { exact: true },
      ),
    ).toBeVisible();
    const after = api.reads.length;
    expect(api.reads.slice(before)).not.toContain("");
    await page.clock.runFor(10_000);
    if (path === "/devices") {
      expect(api.reads).toHaveLength(after);
      await expect(filter).toHaveAccessibleName("Lifecycle: None");
    } else expect(api.reads.slice(after)).toEqual(["enabled"]);
    await expect(filter).toBeVisible();
    fleet[2].state = "disabled";
    await selectDeviceStates(page, ["revoked"]);
    await expect(
      page.getByText(
        path === "/devices" ? "No devices match" : "No devices found",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(filter).toBeVisible();
    await selectDeviceStates(page, ["enabled"]);
    await expect(await seatCells(page)).toHaveText(["A-01"]);
  });

  test(`${path} ignores a late response from a previous lifecycle filter`, async ({
    page,
    context,
  }) => {
    const fleet = targetFleet(2);
    fleet[1].state = "revoked";
    await mockTargets(context, fleet);
    await page.clock.install();
    await page.goto(path);
    await expect(await seatCells(page)).toHaveText(["A-01"]);
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
    await selectDeviceStates(page, ["enabled"]);
    if (path === "/targets") await page.clock.runFor(10_000);
    await expect.poll(() => Boolean(held)).toBe(true);
    await selectDeviceStates(page, ["revoked"]);
    await expect(await seatCells(page)).toHaveText(["A-02"]);
    await held!.fallback();
    await page.clock.runFor(100);
    await expectDeviceStates(page, ["revoked"]);
    await expect(await seatCells(page)).toHaveText(["A-02"]);
  });
}
