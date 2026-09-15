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
      return fulfillJson(
        route,
        200,
        filterDeviceRows([{ ...device, state: deviceState }], request.url()),
      );
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

type TargetBody = components["schemas"]["TargetSubmissionBody"];
type TargetReply = components["schemas"]["TargetSubmissionResponse"];

function filterDeviceRows(rows: (typeof device)[], url: string) {
  const stateParams = new URL(url).searchParams.getAll("state");
  expect(stateParams).toHaveLength(1);
  const states = stateParams[0].split(",");
  for (const state of states)
    expect(["enabled", "disabled", "revoked"]).toContain(state);
  return rows.filter((device) => states.includes(device.state));
}

async function selectDeviceStates(
  page: Page,
  states: components["schemas"]["DeviceListState"][],
) {
  const filter = page.getByRole("group", { name: "Device state", exact: true });
  for (const state of ["enabled", "disabled", "revoked"] as const) {
    await filter
      .getByRole("checkbox", { name: new RegExp(`^${state}$`, "i") })
      .setChecked(states.includes(state));
  }
}

async function mockTargets(
  context: BrowserContext,
  currentDevices: typeof device | (typeof device)[],
  operatorId = operator.operator_id,
) {
  const devices = Array.isArray(currentDevices)
    ? currentDevices
    : [currentDevices];
  const writes: TargetBody[] = [];
  const reads: string[] = [];
  const rejections = new Map<string, string>();
  const receipts = new Map<
    string,
    { fingerprint: string; reply: TargetReply }
  >();
  function apply(body: TargetBody): TargetReply {
    writes.push(structuredClone(body));
    const fingerprint = JSON.stringify([body.scope, body.action]);
    const old = receipts.get(body.operation_id);
    if (old) {
      expect(fingerprint).toBe(old.fingerprint);
      return structuredClone(old.reply);
    }
    const ids =
      body.scope.kind === "devices"
        ? [...new Set(body.scope.device_ids)]
        : devices
            .filter(
              (device) =>
                device.state === "enabled" &&
                (body.scope.kind === "all_enabled" ||
                  device.convergence.connection_state === "active"),
            )
            .map((device) => device.device_id);
    const results: TargetReply["results"] = ids.map((id) => {
      const current = devices.find((device) => device.device_id === id);
      if (!current)
        return {
          device_id: id,
          status: "rejected",
          code: "device_not_found",
          message: "Device not found",
        };
      if (current.state !== "enabled")
        return {
          device_id: id,
          status: "rejected",
          code: "device_not_enabled",
          message: "Device is not enabled",
        };
      const rejection = rejections.get(id);
      if (rejection)
        return {
          device_id: id,
          status: "rejected",
          code: "epoch_exhausted",
          message: rejection,
        };
      const session = current.convergence.session_control;
      const home = current.convergence.home;
      switch (body.action.kind) {
        case "set_foreground":
          session.target = {
            foreground_target: body.action.foreground_target,
            terminate_epoch: session.target?.terminate_epoch ?? null,
          };
          session.status = session.actual ? "drifted" : "awaiting_actual";
          break;
        case "terminate_session":
          session.target = {
            foreground_target: session.target?.foreground_target ?? "waiting",
            terminate_epoch: (session.target?.terminate_epoch ?? 0) + 1,
          };
          session.status = session.actual ? "reconciling" : "awaiting_actual";
          break;
        case "reset_home":
          home.target_reset_epoch = (home.target_reset_epoch ?? 0) + 1;
          home.status = home.actual ? "drifted" : "awaiting_actual";
          break;
      }
      return { device_id: id, status: "submitted" };
    });
    const reply = { operation_id: body.operation_id, results };
    receipts.set(body.operation_id, {
      fingerprint,
      reply: structuredClone(reply),
    });
    return reply;
  }
  await context.route("**/api/v2/**", (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/v2/session" && request.method() === "GET")
      return fulfillJson(route, 200, { ...operator, operator_id: operatorId });
    if (path === "/api/v2/devices" && request.method() === "GET") {
      reads.push(new URL(request.url()).searchParams.get("state") ?? "all");
      return fulfillJson(route, 200, filterDeviceRows(devices, request.url()));
    }
    if (path === "/api/v2/target-submissions" && request.method() === "POST")
      return fulfillJson(route, 200, apply(request.postDataJSON()));
    return fulfillJson(route, 404, {});
  });
  return { writes, apply, rejections, reads };
}

async function selectTarget(page: Page, deviceId: string) {
  await page
    .getByRole("row")
    .filter({ has: page.getByTitle(deviceId, { exact: true }) })
    .getByRole("button", { name: "Manage", exact: true })
    .click();
}
async function openTargets(page: Page) {
  await page.goto("/targets");
  await selectTarget(page, device.device_id);
}
async function confirmTarget(page: Page, name: string) {
  await page.getByRole("button", { name, exact: true }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name, exact: true })
    .click();
}
async function confirmAll(page: Page, name: string) {
  await page
    .getByRole("button", { name: `${name} (all)`, exact: true })
    .click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to all enabled devices" })
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

for (const path of ["/devices", "/targets"]) {
  test(`${path} requests lifecycle filters and keeps filter controls on empty results`, async ({
    page,
    context,
  }) => {
    const fleet = targetFleet(3);
    fleet[1].state = "disabled";
    fleet[2].state = "revoked";
    const api = await mockTargets(context, fleet);
    await page.goto(path);
    const filter = page.getByRole("group", {
      name: "Device state",
      exact: true,
    });
    await expect(
      filter.getByRole("checkbox", { name: /^enabled$/i }),
    ).toBeChecked();
    await expect(
      filter.getByRole("checkbox", { name: /^disabled$/i }),
    ).toBeChecked();
    await expect(
      filter.getByRole("checkbox", { name: /^revoked$/i }),
    ).not.toBeChecked();
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
    await selectDeviceStates(page, []);
    await expect(
      page.getByText("No devices found.", { exact: true }),
    ).toBeVisible();
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
    const filter = page.getByRole("group", {
      name: "Device state",
      exact: true,
    });
    await selectDeviceStates(page, ["enabled"]);
    await expect.poll(() => Boolean(held)).toBe(true);
    await selectDeviceStates(page, ["revoked"]);
    await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-02"]);
    await held!.fallback();
    await page.clock.runFor(100);
    await expect(
      filter.getByRole("checkbox", { name: /^enabled$/i }),
    ).not.toBeChecked();
    await expect(
      filter.getByRole("checkbox", { name: /^disabled$/i }),
    ).not.toBeChecked();
    await expect(
      filter.getByRole("checkbox", { name: /^revoked$/i }),
    ).toBeChecked();
    await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-02"]);
  });
}

test("revoking a selected device removes it from the default view but retains the revoked view", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(1);
  await mockTargets(context, fleet);
  await context.route(`**/api/v2/devices/${fleet[0].device_id}`, (route) => {
    expect(route.request().method()).toBe("PATCH");
    expect(route.request().postDataJSON()).toEqual({ state: "revoked" });
    fleet[0].state = "revoked";
    return fulfillJson(route, 204);
  });
  await page.goto("/devices");
  await page.getByRole("button", { name: "View", exact: true }).click();
  await expect(
    page.getByText(fleet[0].device_id, { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Revoke", exact: true }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Revoke", exact: true })
    .click();
  await expect(
    page.getByText("No devices found.", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText(fleet[0].device_id, { exact: true })).toHaveCount(
    0,
  );
  await selectDeviceStates(page, ["revoked"]);
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expect(
    page.getByRole("img", { name: "Lifecycle: revoked", exact: true }),
  ).toBeVisible();
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
