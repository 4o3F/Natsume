import { expect, test, type Route } from "@playwright/test";

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

for (const [name, action] of [
  [
    "Show waiting screen",
    { kind: "set_foreground", foreground_target: "waiting" },
  ],
  [
    "Show contest desktop",
    { kind: "set_foreground", foreground_target: "contest" },
  ],
  ["Terminate", { kind: "terminate_session" }],
  ["Reset home", { kind: "reset_home" }],
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
      /bg-destructive\/10/,
    );
    await expect(page.locator("tbody tr").nth(3)).toHaveClass(/bg-muted\/60/);
    await page.getByRole("searchbox", { name: "Search devices" }).fill("A-01");
    await expect(page.locator("tbody tr")).toHaveCount(1);
    await page
      .getByRole("button", { name: `${name} (all)`, exact: true })
      .click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toHaveAccessibleName(
      `${name} on all enabled devices?`,
    );
    await expect(dialog).toContainText("Estimated scope: 3 devices");
    await dialog.getByRole("button", { name: "Cancel" }).click();
    expect(api.writes).toHaveLength(0);
    await confirmAll(page, name);
    await expect(
      page.getByRole("region", { name: "Target submission", exact: true }),
    ).toContainText("3 submitted, 0 rejected");
    expect(api.writes).toHaveLength(1);
    expect(api.writes[0]).toMatchObject({
      scope: { kind: "all_enabled" },
      action,
    });
    expect(api.writes[0].operation_id).toMatch(/^[0-9a-f-]{36}$/);
    await page.getByRole("searchbox", { name: "Search devices" }).clear();
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
  await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-05"]);
  const trigger = page.getByRole("button", {
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
  await expect(
    page.getByRole("region", { name: "Target submission", exact: true }),
  ).toContainText("1 submitted, 0 rejected");
  expect(api.writes).toHaveLength(1);
  expect(api.writes[0]).toMatchObject({
    scope: { kind: "all_online_enabled" },
    action: { kind: "power_off" },
  });
  await expect(
    page.getByRole("region", { name: "Target submission", exact: true }),
  ).toContainText("All online enabled devices");
  api.rejections.set(fleet[0].device_id, "Target epoch is exhausted");
  await trigger.click();
  await input.fill("POWER OFF 1");
  await submit.click();
  await expect(
    page.getByRole("region", { name: "Target submission", exact: true }),
  ).toContainText("0 submitted, 1 rejected");
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
  await page
    .getByRole("button", { name: "Reset home (all)", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "Estimated scope: 2 devices",
  );
  fleet[2].state = "enabled";
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to all enabled devices" })
    .click();
  await expect(
    page.getByRole("region", { name: "Target submission", exact: true }),
  ).toContainText("3 submitted, 0 rejected");
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
  const summary = page.getByRole("region", {
    name: "Target submission",
    exact: true,
  });
  await expect(summary).toContainText("2 submitted, 1 rejected");
  await expect(
    page
      .getByRole("row")
      .filter({ has: page.getByRole("cell", { name: "A-02", exact: true }) })
      .getByRole("img", { name: "Rejected: Target epoch is exhausted" }),
  ).toBeVisible();
  api.rejections.clear();
  await confirmTarget(page, "Retry failed devices");
  await expect(summary).toContainText("1 submitted, 0 rejected");
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
  const summary = page.getByRole("region", {
    name: "Target submission",
    exact: true,
  });
  await expect(summary).toContainText("6 submitted, 2 rejected");
  await expect(
    summary.getByText("Partially submitted", { exact: true }),
  ).toBeVisible();
  await expect(summary.getByRole("table")).not.toBeVisible();
  await expect(
    summary.getByText(
      "Server acceptance does not confirm completion. Check device convergence.",
    ),
  ).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("targets-summary.png"),
    fullPage: true,
  });

  await summary.locator("summary").click();
  await expect(summary.getByRole("table").locator("tbody tr")).toHaveCount(8);
  const submittedStatus = summary
    .getByRole("table")
    .getByText("Submitted", { exact: true })
    .first();
  const rejectedStatus = summary
    .getByRole("table")
    .getByText("Rejected", { exact: true })
    .first();
  for (const status of [submittedStatus, rejectedStatus]) {
    await expect(status).toHaveCSS("border-top-width", "0px");
    await expect(status).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  }
  const submittedBounds = await submittedStatus.boundingBox();
  const rejectedBounds = await rejectedStatus.boundingBox();
  expect(submittedBounds).not.toBeNull();
  expect(rejectedBounds).not.toBeNull();
  expect(rejectedBounds!.height).toBe(submittedBounds!.height);
  expect(rejectedBounds!.x).toBe(submittedBounds!.x);
  expect((await rejectedStatus.locator("svg").boundingBox())!.x).toBe(
    (await submittedStatus.locator("svg").boundingBox())!.x,
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
  await page.screenshot({
    path: testInfo.outputPath("targets-rejected.png"),
    fullPage: true,
  });

  // Receipt scope and device labels survive filtering the current device list.
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
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(width);
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
  await page
    .getByRole("button", { name: "Manage", exact: true })
    .first()
    .click();
  const deviceDetails = page.locator('[data-slot="card"]').filter({
    has: page.getByRole("region", { name: "Session Control", exact: true }),
  });
  await expect(
    deviceDetails.getByText("Target foreground: contest", { exact: true }),
  ).toBeVisible();
  await deviceDetails.screenshot({
    path: testInfo.outputPath("targets-device-details.png"),
  });
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
  const summary = page.getByRole("region", {
    name: "Target submission",
    exact: true,
  });
  await expect(
    summary.getByText("Submission rejected", { exact: true }),
  ).toBeVisible();
  await expect(summary).toContainText("0 submitted, 2 rejected");
  await page
    .getByRole("button", { name: "Reset home (all)", exact: true })
    .click();
  for (const device of fleet) device.state = "disabled";
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to all enabled devices" })
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
    page.getByRole("button", { name: "Terminate (all)", exact: true }),
  ).toBeDisabled();
  await page.clock.runFor(10_000);
  expect(api.writes).toHaveLength(1);
  await page.reload();
  await expect(page.getByRole("alert")).toContainText(
    "Submission result not confirmed",
  );
  await selectTarget(page, fleet[1].device_id);
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Retry original request" }).click();
  await expect(
    page.getByRole("region", { name: "Target submission", exact: true }),
  ).toContainText("2 submitted, 0 rejected");
  expect(api.writes).toHaveLength(2);
  expect(api.writes[1]).toEqual(api.writes[0]);
  expect(
    fleet.map((device) => device.convergence.home.target_reset_epoch),
  ).toEqual([1, 1]);
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toBeEnabled();
  await confirmTarget(page, "Reset home");
  await expect(page.getByText("Reset epoch: 2", { exact: true })).toBeVisible();
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
  await expect(
    page.getByText("Target foreground: contest", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Reset home (all)", exact: true }),
  ).toBeDisabled();
  await held!.fallback();
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByText("Target foreground: contest", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("region", { name: "Target submission", exact: true }),
  ).toContainText("A-01");
  expect(api.writes).toHaveLength(1);
  expect(api.writes[0].scope).toEqual({
    kind: "devices",
    device_ids: [fleet[0].device_id],
  });
  await selectTarget(page, fleet[0].device_id);
  await expect(
    page.getByText("Target foreground: waiting", { exact: true }),
  ).toBeVisible();
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
    await expect(page.locator("tbody tr")).toHaveCount(200);
    await page.getByRole("button", { name: "Seat", exact: true }).click();
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    await page.screenshot({ path: testInfo.outputPath("targets.png") });
    await page.locator("tbody tr").nth(150).scrollIntoViewIfNeeded();
    const before = await page.evaluate(() => window.scrollY);
    expect(before).toBeGreaterThan(0);
    fleet[150].convergence.session_control.target!.foreground_target =
      "waiting";
    await page.clock.runFor(10_000);
    await expect(
      page
        .locator("tbody tr")
        .nth(150)
        .getByRole("cell", { name: "waiting", exact: true }),
    ).toBeVisible();
    expect(
      Math.abs((await page.evaluate(() => window.scrollY)) - before),
    ).toBeLessThan(2);
    await expect(
      page.getByRole("columnheader", { name: "Seat", exact: true }),
    ).toHaveAttribute("aria-sort", "ascending");
  });
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
  await expect(
    page.getByRole("region", { name: "All device actions" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "View", exact: true }).first().click();
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
    await expect(
      page.getByText(`Target foreground: ${foreground}`, { exact: true }),
    ).toBeVisible();
  }
  await confirmTarget(page, "Terminate");
  await expect(
    page.getByText("Terminate epoch: 1", { exact: true }),
  ).toBeVisible();
  await confirmTarget(page, "Reset home");
  await expect(page.getByText("Reset epoch: 1", { exact: true })).toBeVisible();
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
  const currentDevice = structuredClone(device);
  currentDevice.convergence.binding = {
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
  currentDevice.convergence.session_control.status = "reconciling";
  currentDevice.convergence.session_control.actual!.foreground = "waiting";
  await mockTargets(context, currentDevice);
  await openTargets(page);

  const sessionRegion = page.getByRole("region", { name: "Session Control" });
  for (const text of [
    "Target foreground: contest",
    "Actual foreground: waiting",
    "Actual session: running",
    "Waiting display ready: true",
    "Contest desktop ready: true",
    "Waiting for binding before showing the contest desktop.",
    "Convergence: reconciling",
  ]) {
    await expect(sessionRegion.getByText(text, { exact: true })).toBeVisible();
  }
  await expect(
    sessionRegion.getByText("Convergence: converged", { exact: true }),
  ).toHaveCount(0);
});

test("target submission stays distinct from reported progress, failure and completion", async ({
  page,
  context,
}) => {
  const currentDevice = structuredClone(device);
  await mockTargets(context, currentDevice);
  await page.clock.install();
  await openTargets(page);
  await confirmTarget(page, "Terminate");
  await expect(
    page.getByText("Terminate epoch: 1", { exact: true }),
  ).toBeVisible();
  await confirmTarget(page, "Reset home");
  await expect(page.getByText("Reset epoch: 1", { exact: true })).toBeVisible();

  const sessionRegion = page.getByRole("region", { name: "Session Control" });
  const homeRegion = page.getByRole("region", { name: "Home", exact: true });
  await expect(
    page.getByRole("status").filter({ hasText: "submitted," }),
  ).toHaveText(
    "1 submitted, 0 rejected. Check device convergence for completion.",
  );
  await expect(
    sessionRegion.getByText("Convergence: reconciling", { exact: true }),
  ).toBeVisible();
  await expect(
    sessionRegion.getByText("Completed terminate epoch: none"),
  ).toBeVisible();
  await expect(
    homeRegion.getByText("Convergence: drifted", { exact: true }),
  ).toBeVisible();
  await expect(
    homeRegion.getByText("Completed reset epoch: none"),
  ).toBeVisible();
  await expect(
    page.getByText("Convergence: converged", { exact: true }),
  ).toHaveCount(0);

  const { session_control: session, home } = currentDevice.convergence;
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
  await expect(
    sessionRegion.getByText("Actual session: terminating"),
  ).toBeVisible();
  await expect(homeRegion.getByText("Actual home: resetting")).toBeVisible();
  await expect(
    homeRegion.getByText("Convergence: reconciling", { exact: true }),
  ).toBeVisible();

  session.actual.session_state = "error";
  session.status = "failed";
  home.actual.state = "recovery_required";
  home.status = "failed";
  await page.clock.runFor(10_000);
  await expect(
    sessionRegion.getByText("Convergence: failed", { exact: true }),
  ).toBeVisible();
  await expect(
    sessionRegion.getByText("The device reported a session error."),
  ).toBeVisible();
  await expect(
    homeRegion.getByText("Convergence: failed", { exact: true }),
  ).toBeVisible();
  await expect(
    homeRegion.getByText("The device requires Home recovery."),
  ).toBeVisible();
  await expect(
    page.getByText("Convergence: converged", { exact: true }),
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
  currentDevice.convergence.received_at_unix_ms = 1_700_000_200_000;
  await page.clock.runFor(10_000);
  await expect(
    sessionRegion.getByText("Completed terminate epoch: 1"),
  ).toBeVisible();
  await expect(homeRegion.getByText("Completed reset epoch: 1")).toBeVisible();
  await expect(
    page.getByText("Convergence: converged", { exact: true }),
  ).toHaveCount(2);
  const reportTime = await page.evaluate(
    (timestamp) => new Date(timestamp).toLocaleString(),
    currentDevice.convergence.received_at_unix_ms,
  );
  await expect(
    page.getByText(`Last device report: ${reportTime}`, { exact: true }),
  ).toBeVisible();
});

test("offline and reconnecting devices wait for fresh Actual after submission", async ({
  page,
  context,
}) => {
  const currentDevice = structuredClone(device);
  const state = currentDevice.convergence;
  state.connection_state = "offline";
  state.received_at_unix_ms = null;
  state.session_control = {
    target: null,
    actual: null,
    status: "awaiting_actual",
  };
  state.home.actual = null;
  state.home.status = "awaiting_actual";
  await mockTargets(context, currentDevice);
  await page.clock.install();
  await openTargets(page);
  await expect(
    page.getByText("Target foreground: not initialized"),
  ).toBeVisible();
  await confirmTarget(page, "Reset home");
  await expect(page.getByText("Reset epoch: 1", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("status").filter({ hasText: "submitted," }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "Device offline. Waiting for a connection and a fresh state report.",
    ),
  ).toBeVisible();
  await expect(
    page.getByText("Last device report: not received"),
  ).toBeVisible();
  await expect(
    page.getByText("Convergence: awaiting actual", { exact: true }),
  ).toHaveCount(2);
  await expect(
    page.getByText("Convergence: converged", { exact: true }),
  ).toHaveCount(0);

  state.connection_state = "awaiting_fresh_state";
  await page.clock.runFor(10_000);
  await expect(
    page.getByText("Device connected. Waiting for a fresh state report."),
  ).toBeVisible();
  await expect(page.getByText("Actual home: not received")).toBeVisible();
  await expect(page.getByText("Completed reset epoch: none")).toBeVisible();
  await expect(
    page.getByText("Convergence: awaiting actual", { exact: true }),
  ).toHaveCount(2);
});

test("refresh failures mark cached completion as old and retry reads the submitted target", async ({
  page,
  context,
}) => {
  const currentDevice = structuredClone(device);
  await mockTargets(context, currentDevice);
  await page.clock.install();
  await openTargets(page);
  const lastRefresh = await page
    .getByText("Last successful refresh:")
    .textContent();
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
  await expect(page.getByText("Last successful refresh:")).toHaveText(
    lastRefresh!,
  );
  await expect(
    page.getByText("Last known convergence: converged", { exact: true }),
  ).toHaveCount(2);

  await confirmTarget(page, "Reset home");
  await expect(
    page.getByRole("status").filter({ hasText: "submitted," }),
  ).toBeVisible();
  await expect(
    page.getByText("Reset epoch: none", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Convergence: converged", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("Refresh failed");

  failRefresh = false;
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByText("Reset epoch: 1", { exact: true })).toBeVisible();
  await expect(page.getByText("Completed reset epoch: none")).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "Home", exact: true })
      .getByText("Convergence: drifted", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Last successful refresh:")).not.toHaveText(
    lastRefresh!,
  );
});

test("a delayed pre-mutation poll cannot replace the refreshed target", async ({
  page,
  context,
}) => {
  const currentDevice = structuredClone(device);
  await mockTargets(context, currentDevice);
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
    page.getByText("Refreshing. Showing previous result."),
  ).toBeVisible();
  await confirmTarget(page, "Reset home");
  await expect.poll(() => heldReads.length).toBe(2);
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Terminate", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByText("Last known convergence: converged", { exact: true }),
  ).toHaveCount(2);

  await fulfillJson(heldReads[1], 200, [currentDevice]);
  await expect(page.getByText("Reset epoch: 1", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toBeEnabled();
  await fulfillJson(heldReads[0], 200, [device]);
  await page.clock.runFor(100);
  await expect(page.getByText("Reset epoch: 1", { exact: true })).toBeVisible();
  await expect(page.getByText("Completed reset epoch: none")).toBeVisible();
});

test("foreground targets follow another operator within one polling interval", async ({
  page,
  context,
  browser,
}) => {
  const currentDevice = structuredClone(device);
  await mockTargets(context, currentDevice);
  await page.clock.install();
  await openTargets(page);
  const otherContext = await browser.newContext();
  try {
    await mockTargets(
      otherContext,
      currentDevice,
      "01912345-6789-7abc-8def-0123456789ac",
    );
    const otherPage = await otherContext.newPage();
    await otherPage.clock.install();
    await openTargets(otherPage);
    await confirmTarget(otherPage, "Terminate");
    await expect(
      otherPage.getByText("Terminate epoch: 1", { exact: true }),
    ).toBeVisible();
    await confirmTarget(otherPage, "Reset home");
    await expect(
      otherPage.getByText("Reset epoch: 1", { exact: true }),
    ).toBeVisible();

    await page.bringToFront();
    await page.clock.runFor(10_000);
    await expect(
      page.getByText("Terminate epoch: 1", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Reset epoch: 1", { exact: true }),
    ).toBeVisible();

    await confirmTarget(page, "Terminate");
    await expect(
      page.getByText("Terminate epoch: 2", { exact: true }),
    ).toBeVisible();
    await otherPage.bringToFront();
    await otherPage.clock.runFor(10_000);
    await expect(
      otherPage.getByText("Terminate epoch: 2", { exact: true }),
    ).toBeVisible();
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
    page.getByText("No devices found.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "All enabled devices (2)", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reset home (all)", exact: true }),
  ).toBeEnabled();
  await confirmAll(page, "Reset home");
  await expect(
    page.getByRole("region", { name: "Target submission", exact: true }),
  ).toContainText("2 submitted, 0 rejected");
  expect(api.writes).toHaveLength(1);
  expect(api.writes[0].scope).toEqual({ kind: "all_enabled" });
  expect(api.reads).toContain("enabled");
  await expect(
    page.getByText("No devices found.", { exact: true }),
  ).toBeVisible();
});
