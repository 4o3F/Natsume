import {
  expect,
  test,
  type BrowserContext,
  type Page,
  type Route,
} from "@playwright/test";

import type { components } from "../src/api/generated/schema";

const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const bindingContext: components["schemas"]["BindingContextResponse"] = {
  binding_id: "01912345-6789-7abc-8def-0123456789ac",
  account_id: "01912345-6789-7abc-8def-0123456789ad",
  credential_revision: 1,
  domjudge_username: "team1",
  seat_code: "A-01",
};
const convergence: components["schemas"]["DeviceConvergenceResponse"] = {
  connection_state: "active",
  received_at_unix_ms: 1_700_000_100_000,
  gateway: {
    status: "converged",
    target: {
      credential_id: "credential-1",
      gateway_leaf_sha256: "leaf-1",
    },
    actual: {
      credential_id: "credential-1",
      state: "ready",
      gateway_leaf_sha256: "leaf-1",
    },
  },
  binding: {
    status: "converged",
    target: { state: "bound", context: bindingContext },
    actual: {
      assignment_state: "applied",
      credential_state: "applied",
      context: bindingContext,
    },
  },
  runtime_config: {
    status: "converged",
    target_domjudge_origin: "https://domjudge.example",
    actual: {
      state: "applied",
      applied_domjudge_origin: "https://domjudge.example",
    },
  },
  session_control: {
    status: "converged",
    target: { foreground_target: "contest", terminate_epoch: null },
    actual: {
      session_state: "running",
      completed_terminate_epoch: null,
      foreground: "contest",
      waiting_ready: true,
      contest_ready: true,
    },
  },
  home: {
    status: "converged",
    target_reset_epoch: null,
    actual: { state: "steady", completed_reset_epoch: null },
  },
};
const device: components["schemas"]["DeviceResponse"] = {
  device_id: "01923456-789a-7bcd-8ef0-123456789abc",
  machine_hardware_id: "machine-01",
  evidence_quality: "strong",
  state: "enabled",
  created_at_unix_ms: 1_700_000_000_000,
  convergence,
};

function fulfillJson(route: Route, status: number, body?: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

test("device lifecycle and convergence use the operator API", async ({
  page,
}) => {
  let deviceState = device.state;

  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session") {
      return fulfillJson(route, 200, operator);
    }
    if (pathname === "/api/v2/devices" && request.method() === "GET") {
      return fulfillJson(route, 200, [{ ...device, state: deviceState }]);
    }
    if (
      pathname === `/api/v2/devices/${device.device_id}` &&
      request.method() === "PATCH"
    ) {
      deviceState = request.postDataJSON().state;
      return fulfillJson(route, 204);
    }
    return fulfillJson(route, 404, {});
  });

  await page.goto("/devices");
  await expect(page.getByText("machine-01", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("img", { name: "Connection: Online" }),
  ).toBeVisible();
  await expect(
    page.getByRole("img", { name: "Binding: bound", exact: true }),
  ).toBeVisible();
  for (const name of ["Gateway", "Binding", "Runtime", "Session", "Home"]) {
    await expect(
      page.getByRole("columnheader", { name, exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("img", { name: `${name}: converged`, exact: true }),
    ).toBeVisible();
  }
  await page.getByRole("button", { name: "View" }).click();
  await expect(page.getByText("Latest state:")).toBeVisible();
  await expect(
    page.getByText("converged", { exact: true }).first(),
  ).toBeVisible();

  await page.getByRole("button", { name: "Disable" }).click();
  await expect(
    page.getByRole("img", { name: "Lifecycle: disabled" }),
  ).toBeVisible();
});

test("device rows distinguish binding, convergence, and offline alerts", async ({
  page,
  context,
}) => {
  const bound = structuredClone(device);
  bound.machine_hardware_id = "11111111-1111-5111-8111-111111111111";
  bound.convergence.gateway.status = "failed";
  bound.convergence.gateway.actual!.state = "upstream_unhealthy";
  bound.convergence.runtime_config.status = "drifted";
  bound.convergence.session_control.status = "reconciling";
  bound.convergence.home.status = "awaiting_actual";
  bound.convergence.home.actual = null;
  const unbound = structuredClone(device);
  unbound.device_id = "01923456-789a-7bcd-8ef0-123456789abd";
  unbound.machine_hardware_id = "22222222-2222-5222-8222-222222222222";
  unbound.convergence.binding.target = {
    state: "unbound",
    negotiation_id: bindingContext.binding_id,
  };
  unbound.convergence.binding.actual = {
    assignment_state: "absent",
    credential_state: "absent",
    context: null,
  };
  const offline = structuredClone(device);
  offline.device_id = "01923456-789a-7bcd-8ef0-123456789abe";
  offline.machine_hardware_id = "33333333-3333-5333-8333-333333333333";
  offline.convergence.connection_state = "offline";
  offline.convergence.binding.status = "awaiting_actual";
  offline.convergence.binding.actual = null;
  const disabled = structuredClone(offline);
  disabled.device_id = "01923456-789a-7bcd-8ef0-123456789abf";
  disabled.machine_hardware_id = "44444444-4444-5444-8444-444444444444";
  disabled.state = "disabled";
  disabled.convergence.binding.target = null;
  const reconnecting = structuredClone(disabled);
  reconnecting.device_id = "01923456-789a-7bcd-8ef0-123456789ac0";
  reconnecting.machine_hardware_id = "55555555-5555-5555-8555-555555555555";
  reconnecting.state = "enabled";
  reconnecting.convergence.connection_state = "awaiting_fresh_state";

  await mockTargets(context, [bound, unbound, offline, disabled, reconnecting]);
  await page.goto("/devices");
  const table = page.getByRole("table");
  const boundRow = table.getByRole("row").filter({ hasText: "11111111…" });
  await expect(
    boundRow.getByRole("cell", { name: "A-01", exact: true }),
  ).toBeVisible();
  await expect(
    table.getByText(bound.machine_hardware_id, { exact: true }),
  ).toHaveCount(0);
  await expect(boundRow.getByTitle(bound.machine_hardware_id)).toHaveText(
    "11111111…",
  );
  await expect(
    table.getByRole("columnheader", { name: "Created" }),
  ).toHaveCount(0);
  await expect(
    table.getByRole("columnheader", { name: "Convergence", exact: true }),
  ).toHaveCount(0);
  for (const name of [
    "Binding: bound",
    "Binding: converged",
    "Gateway: failed",
    "Runtime: drifted",
    "Session: reconciling",
    "Home: awaiting actual",
  ]) {
    await expect(
      boundRow.getByRole("img", { name, exact: true }),
    ).toHaveAttribute("title", name);
  }
  const unboundRow = table.getByRole("row").filter({ hasText: "22222222…" });
  await expect(
    unboundRow.getByRole("img", { name: "Binding: unbound", exact: true }),
  ).toBeVisible();
  await expect(
    unboundRow.getByRole("img", { name: "Binding: converged", exact: true }),
  ).toBeVisible();
  await expect(unboundRow.getByRole("cell").first()).toHaveText("—");
  const offlineRow = table.getByRole("row").filter({ hasText: "33333333…" });
  await expect(
    offlineRow.getByRole("img", { name: "Connection: Offline" }),
  ).toBeVisible();
  await expect(
    offlineRow.getByRole("img", { name: "Binding: bound", exact: true }),
  ).toBeVisible();
  await expect(offlineRow.getByRole("cell").first()).toHaveText("A-01");
  const disabledRow = table.getByRole("row").filter({ hasText: "44444444…" });
  await expect(
    disabledRow.getByRole("img", { name: "Binding: unknown" }),
  ).toBeVisible();
  await expect(
    disabledRow.getByRole("img", { name: "Lifecycle: disabled" }),
  ).toBeVisible();
  await expect(disabledRow.getByRole("cell").first()).toHaveText("—");
  await expect(
    table.getByRole("img", {
      name: "Connection: Connected, awaiting fresh state",
    }),
  ).toBeVisible();
  const colors = await table.locator("tbody tr").evaluateAll((rows) =>
    rows.map((row) => {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 1;
      const paint = canvas.getContext("2d")!;
      paint.fillStyle = "white";
      paint.fillRect(0, 0, 1, 1);
      paint.fillStyle = getComputedStyle(row).backgroundColor;
      paint.fillRect(0, 0, 1, 1);
      return Array.from(paint.getImageData(0, 0, 1, 1).data).slice(0, 3);
    }),
  );
  expect(colors[0][1]).toBeGreaterThan(colors[0][0]); // Online is green.
  expect(colors[2][0]).toBeGreaterThan(colors[2][1]); // Offline is an alert.
  expect(Math.max(...colors[3]) - Math.min(...colors[3])).toBeLessThanOrEqual(
    1,
  ); // Disabled is gray.
  expect(colors[4][0]).toBeGreaterThan(colors[4][2]); // Waiting for state is amber.
  for (const viewport of [
    { name: "tablet-portrait", width: 768, height: 1024 },
    { name: "tablet-landscape", width: 1024, height: 768 },
    { name: "desktop", width: 1440, height: 900 },
  ]) {
    await page.setViewportSize(viewport);
    await page.screenshot({
      path: test.info().outputPath(`devices-${viewport.name}.png`),
      fullPage: true,
    });
  }
});

test("seat sorting uses current binding and survives device polling", async ({
  page,
  context,
}) => {
  const first = structuredClone(device);
  first.convergence.binding.target = {
    state: "bound",
    context: { ...bindingContext, seat_code: "A-10" },
  };
  const second = structuredClone(device);
  second.device_id = "01923456-789a-7bcd-8ef0-123456789abd";
  second.machine_hardware_id = "machine-02";
  second.convergence.binding.target = {
    state: "bound",
    context: { ...bindingContext, seat_code: "A-2" },
  };
  const unbound = structuredClone(device);
  unbound.device_id = "01923456-789a-7bcd-8ef0-123456789abe";
  unbound.machine_hardware_id = "machine-03";
  // A previous client assignment must not become the current seat after unbinding.
  unbound.convergence.binding.target = {
    state: "unbound",
    negotiation_id: bindingContext.binding_id,
  };
  unbound.convergence.binding.status = "drifted";
  await mockTargets(context, [first, unbound, second]);
  await page.clock.install();
  await page.goto("/devices");
  const seats = page.getByRole("table").locator("tbody tr td:first-child");
  await expect(seats).toHaveText(["A-10", "—", "A-2"]);
  const header = page.getByRole("columnheader", { name: "Seat", exact: true });
  await header.getByRole("button", { name: "Seat", exact: true }).click();
  await expect(header).toHaveAttribute("aria-sort", "ascending");
  await expect(seats).toHaveText(["A-2", "A-10", "—"]);
  await header
    .getByRole("button", { name: "Seat", exact: true })
    .press("Enter");
  await expect(header).toHaveAttribute("aria-sort", "descending");
  await expect(seats).toHaveText(["A-10", "A-2", "—"]);

  first.convergence.binding.target = {
    state: "bound",
    context: { ...bindingContext, seat_code: "A-1" },
  };
  first.convergence.connection_state = "offline";
  await page.clock.runFor(10_000);
  await expect(seats).toHaveText(["A-2", "A-1", "—"]);
  await expect(header).toHaveAttribute("aria-sort", "descending");
  const firstRow = page.getByRole("row").filter({ hasText: "machine-01" });
  await expect(
    firstRow.getByRole("img", { name: "Connection: Offline" }),
  ).toBeVisible();
  await firstRow.getByRole("button", { name: "View", exact: true }).click();
  await expect(
    page.getByText(`bound to A-1 (${bindingContext.domjudge_username})`, {
      exact: false,
    }),
  ).toBeVisible();
});

for (const width of [768, 1024, 1440]) {
  test(`device refresh preserves a 200-row viewport at width ${width}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.clock.install({ time: new Date("2030-01-01T00:00:00Z") });
    await page.clock.pauseAt(new Date("2030-01-01T00:01:00Z"));
    const devices = Array.from({ length: 200 }, (_, index) => {
      const current = structuredClone(device);
      current.device_id = `01923456-789a-7bcd-8ef0-${String(index).padStart(12, "0")}`;
      current.machine_hardware_id = `machine-${index + 1}`;
      const context = { ...bindingContext, seat_code: `A-${index + 1}` };
      current.convergence.binding.target = { state: "bound", context };
      current.convergence.binding.actual!.context = context;
      current.convergence.gateway.target!.gateway_leaf_sha256 = "a".repeat(64);
      return current;
    });
    const watchedDevice = devices[149];
    let holdNextRead = false;
    let failReads = false;
    let heldRead: Route | undefined;
    let readCount = 0;
    await page.route("**/api/v2/**", (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      if (path === "/api/v2/session" && request.method() === "GET") {
        return fulfillJson(route, 200, operator);
      }
      if (path === "/api/v2/devices" && request.method() === "GET") {
        readCount += 1;
        if (holdNextRead) {
          heldRead = route;
          holdNextRead = false;
          return;
        }
        return failReads
          ? fulfillJson(route, 503, {
              code: "unavailable",
              title: "Temporarily unavailable",
              status: 503,
            })
          : fulfillJson(route, 200, devices);
      }
      return fulfillJson(route, 404, {});
    });

    await page.goto("/devices");
    const timer = page.getByRole("timer", { name: "Next device refresh" });
    const expectTimer = async (text: string) => {
      // Flush React Query's batched notifications while wall time is paused.
      await expect
        .poll(async () => {
          await page.clock.runFor(20);
          return (await timer.count()) ? timer.textContent() : null;
        })
        .toBe(text);
    };
    await expectTimer("Refresh in 10s");
    await expect(timer).toBeInViewport();
    const table = page.getByRole("table");
    const header = table.getByRole("columnheader", {
      name: "Seat",
      exact: true,
    });
    await header.getByRole("button").click();
    await expect(header).toHaveAttribute("aria-sort", "ascending");
    await expect(table.locator("tbody tr")).toHaveCount(200);
    const noPageOverflow = () =>
      page.evaluate(
        () =>
          document.documentElement.scrollWidth <=
          document.documentElement.clientWidth,
      );
    expect(await noPageOverflow()).toBe(true);

    const watchedRow = table
      .getByRole("row")
      .filter({ hasText: "machine-150" });
    await watchedRow
      .getByText("machine-150", { exact: true })
      .scrollIntoViewIfNeeded();
    const tableNode = await table.elementHandle();
    const rowNode = await watchedRow.elementHandle();
    const scrollContainer = page.locator('[data-slot="table-container"]');
    await scrollContainer.evaluate((element) => {
      element.scrollLeft = element.scrollWidth - element.clientWidth;
    });
    const initialTop = await page.evaluate(() => window.scrollY);
    const initialLeft = await scrollContainer.evaluate(
      (element) => element.scrollLeft,
    );
    expect(initialTop).toBeGreaterThan(0);
    if (width === 768) expect(initialLeft).toBeGreaterThan(0);
    const unchangedViewport = async () => {
      await expect(table.locator("tbody tr")).toHaveCount(200);
      expect(
        await table.evaluate(
          (element, original) => element === original,
          tableNode,
        ),
      ).toBe(true);
      expect(
        await watchedRow.evaluate(
          (element, original) => element === original,
          rowNode,
        ),
      ).toBe(true);
      expect(
        Math.abs((await page.evaluate(() => window.scrollY)) - initialTop),
      ).toBeLessThanOrEqual(1);
      expect(
        Math.abs(
          (await scrollContainer.evaluate((element) => element.scrollLeft)) -
            initialLeft,
        ),
      ).toBeLessThanOrEqual(1);
      await expect(header).toHaveAttribute("aria-sort", "ascending");
    };

    await page.clock.runFor(7_000);
    await expectTimer("Refresh in 3s");
    expect(readCount).toBe(1);
    await unchangedViewport();
    holdNextRead = true;
    await page.clock.runFor(3_000);
    await expect.poll(() => heldRead !== undefined).toBe(true);
    await expectTimer("Refreshing…");
    await unchangedViewport();
    await page.clock.runFor(5_000);
    await expectTimer("Refreshing…");
    expect(readCount).toBe(2);
    devices.reverse();
    watchedDevice.convergence.connection_state = "offline";
    await fulfillJson(heldRead!, 200, devices);
    await expectTimer("Refresh in 10s");
    await expect(
      watchedRow.getByRole("img", { name: "Connection: Offline" }),
    ).toBeVisible();
    await unchangedViewport();

    failReads = true;
    await page.clock.runFor(10_000);
    await expectTimer("Retry in 10s");
    await expect(timer).toHaveAttribute("title", /last received device states/);
    await unchangedViewport();
    failReads = false;
    await page.clock.runFor(10_000);
    await expectTimer("Refresh in 10s");
    expect(readCount).toBe(4);
    await unchangedViewport();

    await watchedRow.getByRole("button", { name: "View", exact: true }).click();
    const details = page.locator('[data-slot="card"]');
    await details.scrollIntoViewIfNeeded();
    expect(await noPageOverflow()).toBe(true);
    await expect(details).toContainText("a".repeat(64));
  });
}

test("an administrator can approve an enrollment review", async ({ page }) => {
  let reviews = [
    {
      review_id: "01934567-89ab-7cde-8f01-23456789abcd",
      machine_hardware_id: "machine-02",
      candidate_public_key: "candidate-public-key",
      evidence_quality: "medium",
      daemon_version: "2.0.0",
      agent_version: "2.0.0",
    },
  ];

  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session" && request.method() === "GET") {
      return fulfillJson(route, 200, operator);
    }
    if (
      pathname === "/api/v2/provisioning-window" &&
      request.method() === "GET"
    ) {
      return fulfillJson(route, 200, { state: "open" });
    }
    if (
      pathname === "/api/v2/enrollment-reviews" &&
      request.method() === "GET"
    ) {
      return fulfillJson(route, 200, reviews);
    }
    if (
      pathname ===
        "/api/v2/enrollment-reviews/01934567-89ab-7cde-8f01-23456789abcd/actions/approve" &&
      request.method() === "POST"
    ) {
      reviews = [];
      return fulfillJson(route, 204);
    }
    return fulfillJson(route, 404, {});
  });

  await page.goto("/enrollment");
  await expect(page.getByText("machine-02", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Approve" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Approve" })
    .click();
  await expect(
    page.getByText("No enrollment reviews are pending."),
  ).toBeVisible();
});

async function mockTargets(
  context: BrowserContext,
  currentDevices: typeof device | (typeof device)[],
  operatorId = operator.operator_id,
) {
  const devices = Array.isArray(currentDevices)
    ? currentDevices
    : [currentDevices];
  await context.route("**/api/v2/**", async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session") {
      return fulfillJson(route, 200, { ...operator, operator_id: operatorId });
    }
    if (pathname === "/api/v2/devices") {
      return fulfillJson(route, 200, devices);
    }
    const currentDevice = devices.find((entry) =>
      pathname.startsWith(`/api/v2/devices/${entry.device_id}/`),
    );
    if (!currentDevice) return fulfillJson(route, 404, {});
    const devicePath = `/api/v2/devices/${currentDevice.device_id}`;
    const session = currentDevice.convergence.session_control;
    const home = currentDevice.convergence.home;
    if (
      pathname === `${devicePath}/session-control` &&
      request.method() === "PUT"
    ) {
      session.target = {
        terminate_epoch: session.target?.terminate_epoch ?? null,
        foreground_target: request.postDataJSON().foreground_target,
      };
      session.status = "drifted";
      return fulfillJson(route, 200, { target: session.target });
    }
    if (
      pathname === `${devicePath}/session-control/actions/terminate` &&
      request.method() === "POST"
    ) {
      session.target = {
        foreground_target: session.target?.foreground_target ?? "contest",
        terminate_epoch: (session.target?.terminate_epoch ?? 0) + 1,
      };
      session.status = session.actual ? "reconciling" : "awaiting_actual";
      return fulfillJson(route, 200, { target: session.target });
    }
    if (
      pathname === `${devicePath}/home/actions/reset` &&
      request.method() === "POST"
    ) {
      home.target_reset_epoch = (home.target_reset_epoch ?? 0) + 1;
      home.status = home.actual ? "drifted" : "awaiting_actual";
      return fulfillJson(route, 200, { reset_epoch: home.target_reset_epoch });
    }
    return fulfillJson(route, 404, {});
  });
}

async function openTargets(page: Page) {
  await page.goto("/targets");
  await selectTarget(page, device.device_id);
}

async function selectTarget(page: Page, deviceId: string) {
  await page
    .getByRole("row")
    .filter({ has: page.getByTitle(deviceId, { exact: true }) })
    .getByRole("button", { name: "Manage", exact: true })
    .click();
}

function targetFleet(count = 5) {
  return Array.from({ length: count }, (_, index) => {
    const current = structuredClone(device);
    current.device_id = `01923456-789a-7bcd-8ef0-${index.toString(16).padStart(12, "0")}`;
    current.machine_hardware_id = `machine-${String(index + 1).padStart(2, "0")}`;
    current.convergence.binding.target = {
      state: "bound",
      context: {
        ...bindingContext,
        seat_code: `A-${String(index + 1).padStart(2, "0")}`,
      },
    };
    return current;
  });
}

for (const [operation, method, path] of [
  ["Show waiting screen", "PUT", "session-control"],
  ["Show contest desktop", "PUT", "session-control"],
  ["Terminate", "POST", "session-control/actions/terminate"],
  ["Reset home", "POST", "home/actions/reset"],
]) {
  test(`bulk ${operation} includes offline enabled devices and ignores the search filter`, async ({
    page,
    context,
  }) => {
    const fleet = targetFleet();
    fleet[1].convergence.connection_state = "offline";
    fleet[3].state = "disabled";
    fleet[4].state = "revoked";
    await mockTargets(context, fleet);
    const writes: { path: string; method: string; body: unknown }[] = [];
    await context.route("**/api/v2/devices/**", (route) => {
      const request = route.request();
      if (["PUT", "POST"].includes(request.method())) {
        writes.push({
          path: new URL(request.url()).pathname,
          method: request.method(),
          body: request.postData() ? request.postDataJSON() : null,
        });
      }
      return route.fallback();
    });
    await page.goto("/targets");
    await expect(page.locator("tbody tr")).toHaveCount(5);
    await expect(page.locator("tbody tr").nth(1)).toHaveClass(
      /bg-destructive\/10/,
    );
    await expect(page.locator("tbody tr").nth(3)).toHaveClass(/bg-muted\/60/);
    await page.getByRole("searchbox", { name: "Search devices" }).fill("A-01");
    await expect(page.locator("tbody tr")).toHaveCount(1);
    const all = page.getByRole("button", {
      name: `${operation} (all)`,
      exact: true,
    });
    await all.click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toHaveAccessibleName(`${operation} on 3 devices?`);
    await expect(dialog).toContainText("1 offline devices");
    await expect(dialog).toContainText(
      "2 disabled or revoked devices are skipped",
    );
    await dialog.getByRole("button", { name: "Cancel" }).click();
    expect(writes).toEqual([]);
    await all.click();
    await dialog.getByRole("button", { name: "Apply to 3 devices" }).click();
    await expect(
      page.getByRole("status").filter({ hasText: `${operation}:` }),
    ).toContainText("3 submitted, 0 not confirmed, 0 remaining");
    await expect(all).toBeEnabled();
    expect(writes.map((write) => write.path).sort()).toEqual(
      fleet
        .slice(0, 3)
        .map((entry) => `/api/v2/devices/${entry.device_id}/${path}`)
        .sort(),
    );
    expect(writes.map((write) => write.method)).toEqual([
      method,
      method,
      method,
    ]);
    if (method === "PUT") {
      expect(writes.map((write) => write.body)).toEqual(
        Array.from({ length: 3 }, () => ({
          foreground_target:
            operation === "Show waiting screen" ? "waiting" : "contest",
        })),
      );
    }
    await page.getByRole("searchbox", { name: "Search devices" }).clear();
    await expect(
      page.getByRole("img", {
        name: "Target submitted; check convergence for completion",
        exact: true,
      }),
    ).toHaveCount(3);
  });
}

test("bulk failures remain per device without retrying accepted resets", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(3);
  await mockTargets(context, fleet);
  let writes = 0;
  await context.route("**/api/v2/devices/**", (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    writes++;
    if (
      new URL(route.request().url()).pathname ===
      `/api/v2/devices/${fleet[1].device_id}/home/actions/reset`
    ) {
      return fulfillJson(route, 409, {
        code: "CONFLICT",
        status: 409,
        title: "Reset rejected for A-02",
      });
    }
    return route.fallback();
  });
  await page.clock.install();
  await page.goto("/targets");
  await page
    .getByRole("button", { name: "Reset home (all)", exact: true })
    .click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to 3 devices" })
    .click();
  await expect(
    page.getByRole("status").filter({ hasText: "Reset home:" }),
  ).toContainText("2 submitted, 1 not confirmed, 0 remaining");
  const failed = page
    .getByRole("row")
    .filter({ has: page.getByRole("cell", { name: "A-02", exact: true }) });
  await expect(
    failed.getByRole("img", { name: "Not confirmed: Reset rejected for A-02" }),
  ).toBeVisible();
  await expect(
    page.getByRole("img", {
      name: "Target submitted; check convergence for completion",
      exact: true,
    }),
  ).toHaveCount(2);
  await page.clock.runFor(10_000);
  expect(writes).toBe(3);
  expect(
    fleet.map((entry) => entry.convergence.home.target_reset_epoch),
  ).toEqual([1, null, 1]);
});

test("bulk confirmation freezes its device set and bounds parallel requests", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(20);
  await mockTargets(context, fleet);
  const held: Route[] = [];
  await context.route("**/api/v2/devices/**", (route) => {
    if (route.request().method() === "PUT") {
      held.push(route);
      return;
    }
    return route.fallback();
  });
  await page.clock.install();
  await page.goto("/targets");
  await page
    .getByRole("button", { name: "Show waiting screen (all)", exact: true })
    .click();
  const added = targetFleet(21)[20];
  fleet.push(added);
  await page.clock.runFor(10_000);
  await expect(page.locator("tbody tr")).toHaveCount(21);
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to 20 devices" })
    .click();
  await expect.poll(() => held.length).toBe(6);
  await expect(
    page.getByRole("img", { name: "Session: converged", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Reset home (all)", exact: true }),
  ).toBeDisabled();
  await selectTarget(page, fleet[0].device_id);
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeDisabled();
  await page.clock.runFor(100);
  expect(held).toHaveLength(6);
  for (let index = 0; index < 20; index++) {
    await expect.poll(() => held.length).toBeGreaterThan(index);
    expect(held[index].request().method()).toBe("PUT");
    await held[index].fallback();
  }
  await expect(
    page.getByRole("status").filter({ hasText: "Show waiting screen:" }),
  ).toContainText("20 submitted, 0 not confirmed, 0 remaining");
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeEnabled();
  expect(
    held.map((route) => new URL(route.request().url()).pathname).sort(),
  ).toEqual(
    fleet
      .slice(0, 20)
      .map((entry) => `/api/v2/devices/${entry.device_id}/session-control`)
      .sort(),
  );
  expect(added.convergence.session_control.target?.foreground_target).toBe(
    "contest",
  );
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
    await expect(
      page.getByRole("columnheader", { name: "Seat", exact: true }),
    ).toHaveAttribute("aria-sort", "ascending");
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
    await expect(
      page.getByRole("timer", { name: "Next device refresh" }),
    ).toContainText("Refresh in");
  });
}

test("viewers see all targets without bulk or single-device mutation controls", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(2);
  await mockTargets(context, fleet);
  await context.route("**/api/v2/session", (route) =>
    fulfillJson(route, 200, { ...operator, role: "viewer" }),
  );
  await page.goto("/targets");
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await expect(
    page.getByRole("region", { name: "All device actions" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "View", exact: true }).first().click();
  await expect(
    page.getByRole("region", { name: "Session Control" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Reset home", exact: true }),
  ).toHaveCount(0);
});

async function confirmTarget(page: Page, name: string) {
  await page.getByRole("button", { name, exact: true }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name, exact: true })
    .click();
}

test("target controls call their generated API operations", async ({
  page,
  context,
}) => {
  await mockTargets(context, structuredClone(device));
  await openTargets(page);
  for (const [name, foregroundTarget] of [
    ["Show contest desktop", "contest"],
    ["Show waiting screen", "waiting"],
  ]) {
    const request = page.waitForRequest(
      (request) =>
        request.method() === "PUT" &&
        request.url().endsWith("/session-control"),
    );
    await page.getByRole("button", { name, exact: true }).click();
    const sent = await request;
    expect(new URL(sent.url()).pathname).toBe(
      `/api/v2/devices/${device.device_id}/session-control`,
    );
    expect(sent.postDataJSON()).toEqual({
      foreground_target: foregroundTarget,
    });
    await expect(
      page.getByText(`Target foreground: ${foregroundTarget}`, { exact: true }),
    ).toBeVisible();
  }

  for (const [name, path, target] of [
    ["Terminate", "session-control/actions/terminate", "Terminate epoch: 1"],
    ["Reset home", "home/actions/reset", "Reset epoch: 1"],
  ]) {
    const request = page.waitForRequest(
      (request) =>
        request.method() === "POST" && request.url().endsWith(`/${path}`),
    );
    await confirmTarget(page, name);
    expect(new URL((await request).url()).pathname).toBe(
      `/api/v2/devices/${device.device_id}/${path}`,
    );
    await expect(page.getByText(target, { exact: true })).toBeVisible();
  }
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
    page.getByRole("status").filter({ hasText: "Target submitted." }),
  ).toHaveText(
    "Target submitted. Check the convergence state below for device completion.",
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
    page.getByRole("status").filter({ hasText: "Target submitted." }),
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
  await context.route("**/api/v2/devices", (route) =>
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
    page.getByRole("status").filter({ hasText: "Target submitted." }),
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
  await context.route("**/api/v2/devices", (route) => {
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
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Terminate", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Show waiting screen", exact: true }),
  ).toBeDisabled();
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

for (const outcome of ["success", "failure"]) {
  test(`switching devices isolates an in-flight target ${outcome}`, async ({
    page,
    context,
  }) => {
    const firstDevice = structuredClone(device);
    const secondDevice = {
      ...structuredClone(device),
      device_id: "01923456-789a-7bcd-8ef0-123456789abd",
      machine_hardware_id: "machine-02",
    };
    await mockTargets(context, [firstDevice, secondDevice]);
    const heldWrites: Route[] = [];
    await context.route("**/session-control", (route) => {
      heldWrites.push(route);
    });
    await page.clock.install();
    await openTargets(page);
    await page
      .getByRole("button", { name: "Show waiting screen", exact: true })
      .click();
    await expect.poll(() => heldWrites.length).toBe(1);
    const request = heldWrites[0].request();
    expect(new URL(request.url()).pathname).toBe(
      `/api/v2/devices/${firstDevice.device_id}/session-control`,
    );
    expect(request.method()).toBe("PUT");
    expect(request.postDataJSON()).toEqual({ foreground_target: "waiting" });
    await expect(
      page.getByRole("button", { name: "Show waiting screen", exact: true }),
    ).toBeDisabled();

    await expect(
      page.getByRole("button", { name: "Reset home (all)", exact: true }),
    ).toBeDisabled();
    await selectTarget(page, secondDevice.device_id);
    await selectTarget(page, firstDevice.device_id);
    await expect(
      page.getByRole("button", { name: "Show waiting screen", exact: true }),
    ).toBeDisabled();
    await selectTarget(page, secondDevice.device_id);
    await expect(
      page.getByRole("button", { name: "Show waiting screen", exact: true }),
    ).toBeEnabled();
    await expect(
      page.getByText("Target foreground: contest", { exact: true }),
    ).toBeVisible();
    if (outcome === "success") {
      const refreshed = page.waitForResponse((response) =>
        response.url().endsWith("/api/v2/devices"),
      );
      await heldWrites[0].fallback();
      await (await refreshed).finished();
    } else {
      const failed = page.waitForResponse(request.url());
      await fulfillJson(heldWrites[0], 409, {
        title: "Device A target rejected",
        status: 409,
        code: "CONFLICT",
      });
      await (await failed).finished();
    }
    // Flush mutation notifications before checking that B stayed untouched.
    await page.clock.runFor(100);
    await expect(page.getByRole("alert")).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Show waiting screen", exact: true }),
    ).toBeEnabled();
    await expect(
      page.getByText("Target foreground: contest", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Target submitted.", { exact: false }),
    ).toHaveCount(0);
    await selectTarget(page, firstDevice.device_id);
    await expect(
      page.getByText(
        `Target foreground: ${outcome === "success" ? "waiting" : "contest"}`,
        { exact: true },
      ),
    ).toBeVisible();
  });
}

for (const order of ["A then B", "B then A"]) {
  test(`device target submissions stay isolated when responses arrive ${order}`, async ({
    page,
    context,
  }) => {
    const firstDevice = structuredClone(device);
    const secondDevice = {
      ...structuredClone(device),
      device_id: "01923456-789a-7bcd-8ef0-123456789abd",
      machine_hardware_id: "machine-02",
    };
    await mockTargets(context, [firstDevice, secondDevice]);
    const heldWrites: Route[] = [];
    await context.route("**/actions/*", (route) => {
      heldWrites.push(route);
    });
    await page.clock.install();
    await openTargets(page);
    await confirmTarget(page, "Terminate");
    await expect.poll(() => heldWrites.length).toBe(1);
    await selectTarget(page, secondDevice.device_id);
    await confirmTarget(page, "Reset home");
    await expect.poll(() => heldWrites.length).toBe(2);
    expect(
      heldWrites.map((route) => [
        route.request().method(),
        new URL(route.request().url()).pathname,
      ]),
    ).toEqual([
      [
        "POST",
        `/api/v2/devices/${firstDevice.device_id}/session-control/actions/terminate`,
      ],
      ["POST", `/api/v2/devices/${secondDevice.device_id}/home/actions/reset`],
    ]);
    const reset = page.getByRole("button", { name: "Reset home", exact: true });
    await expect(reset).toBeDisabled();

    let secondCompleted = false;
    for (const index of order === "A then B" ? [0, 1] : [1, 0]) {
      const refreshed = page.waitForResponse((response) =>
        response.url().endsWith("/api/v2/devices"),
      );
      await heldWrites[index].fallback();
      await (await refreshed).finished();
      await page.clock.runFor(100);
      secondCompleted ||= index === 1;
      await expect(reset).toBeEnabled({ enabled: secondCompleted });
      await expect(page.getByRole("alert")).toHaveCount(0);
      await expect(
        page.getByText("Target submitted.", { exact: false }),
      ).toHaveCount(secondCompleted ? 1 : 0);
      await expect(
        page.getByText("Submitting target and refreshing status.", {
          exact: false,
        }),
      ).toHaveCount(secondCompleted ? 0 : 1);
      await expect(
        page.getByText("Terminate epoch: none", { exact: true }),
      ).toBeVisible();
      await expect(
        page.getByText(`Reset epoch: ${secondCompleted ? "1" : "none"}`, {
          exact: true,
        }),
      ).toBeVisible();
    }
    await selectTarget(page, firstDevice.device_id);
    await expect(
      page.getByText("Terminate epoch: 1", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Reset epoch: none", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Target submitted.", { exact: false }),
    ).toHaveCount(0);
  });
}

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
