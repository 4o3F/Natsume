import { readFileSync } from "node:fs";
import { expect, test, type Route } from "@playwright/test";

import {
  operator,
  bindingContext,
  device,
  fulfillJson,
  filterDeviceRows,
  selectDeviceStates,
  expectDeviceStates,
  selectConnectionStates,
  selectBindingStates,
  mockTargets,
  targetFleet,
} from "./fixtures/devices";
import {
  closeDetails,
  deviceRow,
  openDeviceDetails,
  openDeviceMenu,
  refreshLabel,
  seatCells,
} from "./fixtures/ui";

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
    page.getByRole("img", { name: /^Binding: bound,/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("columnheader", { name: "Convergence", exact: true }),
  ).toBeVisible();
  for (const name of ["Gateway", "Binding", "Runtime", "Session", "Home"]) {
    await expect(
      page.getByRole("img", { name: /Gateway: converged/ }),
    ).toHaveAccessibleName(new RegExp(`${name}: converged`));
  }
  await openDeviceDetails(page, "machine-01");
  await expect(page.getByText(/^Latest state /)).toBeVisible();
  await expect(
    page.getByText("converged", { exact: true }).first(),
  ).toBeVisible();

  await page.getByRole("button", { name: "Disable", exact: true }).click();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("button", { name: "Enable", exact: true }),
  ).toBeEnabled();
  await closeDetails(page);
  await expect(
    page.getByRole("cell", { name: "Disabled", exact: true }),
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
  const boundRow = deviceRow(page, bound.machine_hardware_id);
  await expect(
    boundRow.getByRole("cell", { name: "A-01", exact: true }),
  ).toBeVisible();
  await expect(
    table.getByText(bound.machine_hardware_id, { exact: true }),
  ).toHaveCount(0);
  await expect(boundRow.getByTitle(bound.machine_hardware_id)).toHaveText(
    `${bound.machine_hardware_id.slice(0, 12)}…`,
  );
  await expect(
    table.getByRole("columnheader", { name: "Created" }),
  ).toHaveCount(0);
  await expect(
    table.getByRole("columnheader", { name: "Convergence", exact: true }),
  ).toBeVisible();
  for (const name of [
    "Binding: bound",
    "Binding: converged",
    "Gateway: failed",
    "Runtime: drifted",
    "Session: reconciling",
    "Home: awaiting actual",
  ]) {
    const status =
      name === "Binding: bound"
        ? boundRow.getByRole("img", { name: /^Binding: bound,/ })
        : boundRow.getByRole("img", { name: /Gateway:/ });
    await expect(status).toHaveAccessibleName(new RegExp(name));
  }
  const unboundRow = deviceRow(page, unbound.machine_hardware_id);
  await expect(
    unboundRow.getByRole("img", { name: /^Binding: unbound,/ }),
  ).toBeVisible();
  await expect(
    unboundRow.getByRole("img", { name: /Binding: converged/ }),
  ).toBeVisible();
  await expect(unboundRow.getByRole("cell").nth(1)).toHaveText("—");
  const offlineRow = deviceRow(page, offline.machine_hardware_id);
  await expect(
    offlineRow.getByRole("img", { name: "Connection: Offline" }),
  ).toBeVisible();
  await expect(
    offlineRow.getByRole("img", { name: /^Binding: bound,/ }),
  ).toBeVisible();
  await expect(offlineRow.getByRole("cell").nth(1)).toHaveText("A-01");
  const disabledRow = deviceRow(page, disabled.machine_hardware_id);
  await expect(
    disabledRow.getByRole("img", { name: /^Binding: unknown,/ }),
  ).toBeVisible();
  await expect(
    disabledRow.getByRole("cell", { name: "Disabled", exact: true }),
  ).toBeVisible();
  await expect(disabledRow.getByRole("cell").nth(1)).toHaveText("—");
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
  expect(Math.max(...colors[0]) - Math.min(...colors[0])).toBeLessThanOrEqual(
    1,
  ); // Online rows use a neutral surface.
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
  const seats = await seatCells(page);
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
  const details = await openDeviceDetails(page, "machine-01");
  await expect(
    details.getByRole("definition").filter({ hasText: /^A-1$/ }),
  ).toBeVisible();
  await expect(
    details.getByText(bindingContext.domjudge_username, { exact: true }),
  ).toBeVisible();
});

test("/devices combines binding, connection, lifecycle, and search filters and resets empty results", async ({
  page,
  context,
}, testInfo) => {
  const fleet = targetFleet(6);
  fleet[0].machine_hardware_id = "f0e1d2c3-b4a5-5678-9012-abcdefabcdef";
  fleet[1].convergence.connection_state = "offline";
  fleet[2].convergence.connection_state = "awaiting_fresh_state";
  fleet[3].state = "disabled";
  fleet[3].convergence.connection_state = "offline";
  fleet[4].state = "revoked";
  fleet[4].convergence.connection_state = "offline";
  fleet[5].convergence.binding.target = {
    state: "unbound",
    negotiation_id: bindingContext.binding_id,
  };
  const api = await mockTargets(context, fleet);
  await page.clock.install({ time: new Date("2030-01-01T00:00:00Z") });
  await page.goto("/devices");
  const seats = await seatCells(page);
  const connection = page.getByRole("button", { name: /^Connection: / });
  const binding = page.getByRole("button", { name: /^Binding: / });
  const search = page.getByRole("searchbox", { name: "Search devices" });
  const reset = page.getByRole("button", { name: "Reset", exact: true });
  await expect(seats).toHaveText(["A-01", "A-02", "A-03", "A-04", "—"]);
  await expect(page.getByRole("status")).toHaveText("5 of 5 devices");
  await expect(reset).toHaveCount(0);
  await expect(connection).toHaveAccessibleName("Connection: All");
  await expect(binding).toHaveAccessibleName("Binding: All");
  await expectDeviceStates(page, ["enabled", "disabled"]);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.screenshot({
    path: testInfo.outputPath("devices-filters-all-desktop.png"),
    fullPage: true,
  });

  await selectConnectionStates(page, ["offline"]);
  await expect(seats).toHaveText(["A-02", "A-04"]);
  await expect(page.getByRole("status")).toHaveText("2 of 5 devices");
  await page.screenshot({
    path: testInfo.outputPath("devices-filters-offline-desktop.png"),
    fullPage: true,
  });
  for (const width of [768, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    await page.screenshot({
      path: testInfo.outputPath(`devices-filters-offline-${width}.png`),
      fullPage: true,
    });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  await selectConnectionStates(page, ["active"]);
  await expect(seats).toHaveText(["A-01", "—"]);
  await selectConnectionStates(page, ["awaiting_fresh_state"]);
  await expect(seats).toHaveText(["A-03"]);
  await selectConnectionStates(page, ["active", "offline"]);
  await expect(seats).toHaveText(["A-01", "A-02", "A-04", "—"]);
  await selectBindingStates(page, ["bound"]);
  await expect(seats).toHaveText(["A-01", "A-02", "A-04"]);
  await selectBindingStates(page, ["unbound"]);
  await expect(seats).toHaveText(["—"]);
  await selectBindingStates(page, ["bound", "unbound"]);
  await expect(seats).toHaveText(["A-01", "A-02", "A-04", "—"]);
  await selectBindingStates(page, ["bound", "unbound", "unknown"]);
  // Local filters do not cause extra requests or alter the lifecycle cache key.
  expect(api.reads).toEqual(["enabled,disabled"]);
  await selectDeviceStates(page, ["disabled"]);
  await expect(seats).toHaveText(["A-04"]);
  await selectDeviceStates(page, ["revoked"]);
  await expect(seats).toHaveText(["A-05"]);
  await reset.click();
  await expect(seats).toHaveText(["A-01", "A-02", "A-03", "A-04", "—"]);
  await expect(reset).toHaveCount(0);

  for (const [query, expected] of [
    ["  a-02  ", "A-02"],
    // Match hidden portions of a full hardware ID, not just its truncated label.
    ["D2C3-B4A5", "A-01"],
    ["MACHINE-03", "A-03"],
    [fleet[3].device_id, "A-04"],
    // The previous client assignment is not the current seat after unbinding.
    ["A-06", null],
  ] as const) {
    await search.fill(query);
    if (expected) await expect(seats).toHaveText([expected]);
    else
      await expect(
        page.getByText("No devices match", { exact: true }),
      ).toBeVisible();
  }
  await search.fill("machine-02");
  await selectConnectionStates(page, ["active"]);
  await expect(
    page.getByText("No devices match", { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("devices-filters-empty-desktop.png"),
    fullPage: true,
  });
  await expect(connection).toBeVisible();
  await reset.click();
  await expect(search).toHaveValue("");
  await expect(seats).toHaveCount(5);
  await selectConnectionStates(page, []);
  await expect(
    page.getByText("No devices match", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("status")).toHaveText("0 of 5 devices");
  await expect(connection).toHaveAccessibleName("Connection: None");
  // Controls remain keyboard-operable even when no states are selected.
  await connection.press("ArrowDown");
  const menu = page.getByRole("menu", { name: "Connection", exact: true });
  await menu
    .getByRole("menuitemcheckbox", { name: "Offline", exact: true })
    .focus();
  await page.keyboard.press("Space");
  await expect(menu).toBeVisible();
  await expect(seats).toHaveText(["A-02", "A-04"]);
  await page.keyboard.press("Escape");
  await expect(connection).toBeFocused();
  await expect(connection).toHaveAccessibleName("Connection: Offline");
});

test("/devices intersects all facets and search without treating unknown or stale Actual as unbound", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(7);
  for (const row of fleet) row.convergence.connection_state = "offline";
  for (const index of [1, 6]) {
    fleet[index].convergence.binding.target = {
      state: "unbound",
      negotiation_id: bindingContext.binding_id,
    };
  }
  fleet[2].convergence.binding.target = null;
  fleet[3].state = "disabled";
  fleet[4].convergence.connection_state = "active";
  fleet[5].state = "revoked";
  fleet[6].state = "disabled";
  const api = await mockTargets(context, fleet);
  await page.clock.install();
  await page.goto("/devices");
  const seats = await seatCells(page);
  const search = page.getByRole("searchbox", { name: "Search devices" });
  await expect(seats).toHaveText(["A-01", "—", "—", "A-04", "A-05", "—"]);
  await selectBindingStates(page, ["unknown"]);
  await expect(seats).toHaveText(["—"]);
  await expect(
    page.getByRole("row").filter({ hasText: "machine-03" }),
  ).toBeVisible();
  await search.fill("A-03");
  await expect(page.getByRole("status")).toHaveText("0 of 6 devices");
  await search.clear();
  await selectBindingStates(page, ["unbound"]);
  await expect(seats).toHaveText(["—", "—"]);
  await selectBindingStates(page, ["bound", "unknown"]);
  await expect(seats).toHaveText(["A-01", "—", "A-04", "A-05"]);
  await selectConnectionStates(page, ["offline"]);
  await expect(seats).toHaveText(["A-01", "—", "A-04"]);
  expect(api.reads).toEqual(["enabled,disabled"]);
  await selectDeviceStates(page, ["disabled"]);
  await expect(seats).toHaveText(["A-04"]);
  const readsAfterLifecycle = [...api.reads];
  await selectBindingStates(page, ["bound", "unbound"]);
  await expect(seats).toHaveText(["A-04", "—"]);
  await search.fill(" MACHINE-07 ");
  await expect(seats).toHaveText(["—"]);
  await expect(page.getByRole("status")).toHaveText("1 of 2 devices");
  await selectBindingStates(page, ["bound"]);
  await expect(page.getByRole("status")).toHaveText("0 of 2 devices");
  await selectBindingStates(page, []);
  await expect(
    page.getByRole("button", { name: /^Binding: / }),
  ).toHaveAccessibleName("Binding: None");
  expect(api.reads).toEqual(readsAfterLifecycle);
  await page.getByRole("button", { name: "Reset", exact: true }).click();
  await expect(search).toHaveValue("");
  await expect(page.getByRole("status")).toHaveText("6 of 6 devices");
  await expect(
    page.getByRole("button", { name: /^Connection: / }),
  ).toHaveAccessibleName("Connection: All");
  await expect(
    page.getByRole("button", { name: /^Binding: / }),
  ).toHaveAccessibleName("Binding: All");
  await expectDeviceStates(page, ["enabled", "disabled"]);
  await expect(
    page.getByRole("button", { name: "Reset", exact: true }),
  ).toHaveCount(0);
});

test("device facet menus support keyboard multi-selection and fit a mobile viewport", async ({
  page,
  context,
}, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockTargets(context, targetFleet(1));
  await page.goto("/devices");
  for (const [name, labels] of [
    ["Lifecycle", ["Enabled", "Disabled", "Revoked"]],
    ["Connection", ["Online", "Connected, awaiting fresh state", "Offline"]],
    ["Binding", ["Bound", "Unbound", "Unknown"]],
  ] as const) {
    const trigger = page.getByRole("button", {
      name: new RegExp(`^${name}: `),
    });
    await trigger.focus();
    await trigger.press("ArrowDown");
    const menu = page.getByRole("menu", { name, exact: true });
    await expect(menu.getByRole("menuitemcheckbox")).toHaveText([...labels]);
    const first = menu.getByRole("menuitemcheckbox", {
      name: labels[0],
      exact: true,
    });
    await expect(first).toBeFocused();
    await expect(first).toBeChecked();
    await page.keyboard.press("Space");
    await expect(first).not.toBeChecked();
    await expect(menu).toBeVisible();
    await page.keyboard.press("Enter");
    await expect(first).toBeChecked();
    await expect(menu).toBeVisible();
    await page.keyboard.press("ArrowDown");
    await expect(
      menu.getByRole("menuitemcheckbox", { name: labels[1], exact: true }),
    ).toBeFocused();
    const bounds = await menu.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(390);
    await page.screenshot({
      path: testInfo.outputPath(`filter-${name.toLowerCase()}.png`),
    });
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    await expect(trigger).toBeFocused();
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
  }
});

test("/devices reapplies local filters to polled connection and binding state without resetting search or details", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(2);
  fleet[0].convergence.connection_state = "offline";
  fleet[0].convergence.binding.target = {
    state: "unbound",
    negotiation_id: bindingContext.binding_id,
  };
  fleet[1].convergence.connection_state = "offline";
  await mockTargets(context, fleet);
  await page.clock.install();
  await page.goto("/devices");
  await selectConnectionStates(page, ["offline"]);
  await selectBindingStates(page, ["bound"]);
  const search = page.getByRole("searchbox", { name: "Search devices" });
  await search.fill("machine-");
  await expect(await seatCells(page)).toHaveText(["A-02"]);
  await openDeviceDetails(page, "machine-02");
  await expect(page.getByRole("dialog")).toContainText(fleet[1].device_id);
  await page.clock.runFor(10_000);
  await expect(page.getByRole("dialog")).toContainText(fleet[1].device_id);
  fleet[0].convergence.binding.target = {
    state: "bound",
    context: { ...bindingContext, seat_code: "A-01" },
  };
  fleet[1].convergence.binding.target = {
    state: "unbound",
    negotiation_id: bindingContext.binding_id,
  };
  await page.clock.runFor(10_000);
  await expect(page.getByRole("dialog")).toContainText(fleet[1].device_id);
  await expect(
    page
      .getByRole("dialog")
      .getByRole("heading", { name: "Unbound device", exact: true }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("definition")
      .filter({ hasText: /^unbound$/ }),
  ).toBeVisible();
  await closeDetails(page);
  await expect(await seatCells(page)).toHaveText(["A-01"]);
  fleet[0].convergence.connection_state = "active";
  await page.clock.runFor(10_000);
  await expect(page.getByRole("status")).toHaveText("0 of 2 devices");
  await expect(search).toHaveValue("machine-");
  await expect(
    page.getByRole("button", { name: /^Connection: / }),
  ).toHaveAccessibleName("Connection: Offline");
  await expect(
    page.getByRole("button", { name: /^Binding: / }),
  ).toHaveAccessibleName("Binding: Bound");
  await expectDeviceStates(page, ["enabled", "disabled"]);
});

test("/devices shows IP differences inline and exports the complete emergency address list", async ({
  page,
  context,
}, testInfo) => {
  const fleet = targetFleet(6);
  const network = (server: string, client: string | null = server) => ({
    server_observed_ip: server,
    client_ip: client,
    observed_at_unix_ms: 1_700_000_100_000,
  });
  fleet[0].network = network("192.0.2.10");
  fleet[1].network = network("192.0.2.2", "198.51.100.2");
  fleet[1].state = "disabled";
  fleet[1].convergence.connection_state = "offline";
  fleet[3].network = network("192.0.2.10", null);
  fleet[4].network = network("192.0.2.99");
  fleet[4].state = "revoked";
  fleet[5].network = network("2001:db8::5");
  await mockTargets(context, fleet);
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/devices");
  const emergency = page.getByRole("region", { name: "Emergency IP export" });
  await expect(emergency).not.toBeVisible();
  await expect(page.getByRole("columnheader", { name: /IP/i })).toHaveCount(0);
  const warning = page.getByRole("button", {
    name: "IP mismatch: view details for machine-02",
    exact: true,
  });
  await expect(page.getByRole("button", { name: /^IP mismatch:/ })).toHaveCount(
    1,
  );
  await warning.focus();
  await expect(page.getByRole("tooltip")).toBeVisible();
  await warning.press("Enter");
  const details = page.getByRole("region", { name: "Network addresses" });
  await expect(details).toContainText("Client and Server IP differ");
  await expect(details).toContainText(
    "Device offline; showing last recorded addresses.",
  );
  await expect(details.getByText("192.0.2.2", { exact: true })).toBeVisible();
  await expect(
    details.getByText("198.51.100.2", { exact: true }),
  ).toBeVisible();
  await details
    .getByRole("button", { name: "Copy Server IP", exact: true })
    .click();
  await expect(
    page.locator("[data-sonner-toast]").filter({ hasText: "Server IP copied" }),
  ).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "192.0.2.2",
  );
  await details
    .getByRole("button", { name: "Copy Client IP", exact: true })
    .click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "198.51.100.2",
  );
  await closeDetails(page);
  await page.getByRole("button", { name: "Emergency", exact: true }).click();
  for (const [label, count] of [
    ["Unique IPs", 3],
    ["Offline", 1],
    ["Mismatch", 1],
    ["Missing", 1],
  ] as const) {
    await expect(
      emergency
        .getByRole("term")
        .filter({ hasText: new RegExp(`^${label}$`) })
        .locator("..")
        .getByRole("definition"),
    ).toHaveText(String(count));
  }
  for (const width of [1440, 768, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    const bounds = await page
      .getByRole("menu", { name: "Emergency tools", exact: true })
      .boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    await page.screenshot({
      path: testInfo.outputPath(`network-emergency-${width}.png`),
      fullPage: true,
    });
  }
  await selectDeviceStates(page, ["enabled"]);
  await selectConnectionStates(page, ["active"]);
  await selectBindingStates(page, []);
  await page
    .getByRole("searchbox", { name: "Search devices" })
    .fill("no-match");
  await expect(
    page.getByText("No devices match", { exact: true }),
  ).toBeVisible();
  fleet[0].network = network("192.0.2.11");
  fleet[3].network = network("192.0.2.11", null);
  await expect(emergency).not.toBeVisible();
  await page.getByRole("button", { name: "Emergency", exact: true }).click();
  const downloaded = page.waitForEvent("download");
  await emergency
    .getByRole("menuitem", { name: "Export all device IPs" })
    .click();
  const file = await downloaded;
  expect(file.suggestedFilename()).toMatch(/^natsume-device-ips-.*\.txt$/);
  expect(readFileSync((await file.path())!, "utf8")).toBe(
    "192.0.2.2\n192.0.2.11\n2001:db8::5\n",
  );
  await expect(emergency.getByRole("status")).toContainText(
    "Exported 3 unique IPs",
  );
});

for (const width of [1440, 390]) {
  test(`Emergency is a keyboard-accessible header tool without shifting the table at ${width}px`, async ({
    page,
    context,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await mockTargets(context, targetFleet(1));
    await page.goto("/devices");
    const trigger = page.getByRole("button", {
      name: "Emergency",
      exact: true,
    });
    const menu = page.getByRole("menu", {
      name: "Emergency tools",
      exact: true,
    });
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
    await expect(menu).toHaveCount(0);
    const triggerBounds = await trigger.boundingBox();
    const filterBounds = await page
      .getByRole("search", { name: "Device filters", exact: true })
      .boundingBox();
    expect(triggerBounds!.y + triggerBounds!.height).toBeLessThanOrEqual(
      filterBounds!.y,
    );
    const tableBefore = await page.getByRole("table").boundingBox();
    await trigger.focus();
    await trigger.press("ArrowDown");
    await expect(menu).toBeVisible();
    await expect(
      menu.getByRole("menuitem", { name: "Export all device IPs" }),
    ).toBeFocused();
    const menuBounds = await menu.boundingBox();
    expect(menuBounds!.x).toBeGreaterThanOrEqual(0);
    expect(menuBounds!.x + menuBounds!.width).toBeLessThanOrEqual(width);
    expect(menuBounds!.y + menuBounds!.height).toBeLessThanOrEqual(900);
    expect((await page.getByRole("table").boundingBox())!.y).toBe(
      tableBefore!.y,
    );
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    await page.screenshot({
      path: testInfo.outputPath("emergency-header-open.png"),
    });
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    await expect(trigger).toBeFocused();
    await trigger.click();
    await expect(menu).toBeVisible();
    await page.getByRole("heading", { name: "Devices", exact: true }).click();
    await expect(menu).toHaveCount(0);
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
    await page.screenshot({
      path: testInfo.outputPath("emergency-header-closed.png"),
    });
  });
}

test("/devices reports failed and empty IP exports without downloading a partial file", async ({
  page,
  context,
}) => {
  const fleet = targetFleet(1);
  await mockTargets(context, fleet);
  await page.goto("/devices");
  await page.getByRole("button", { name: "Emergency", exact: true }).click();
  const emergency = page.getByRole("region", { name: "Emergency IP export" });
  await expect(
    emergency
      .getByRole("term")
      .filter({ hasText: /^Unique IPs$/ })
      .locator("..")
      .getByRole("definition"),
  ).toHaveText("0");
  const downloads: string[] = [];
  page.on("download", (file) => downloads.push(file.suggestedFilename()));
  await context.route("**/api/v2/devices?**", (route) =>
    fulfillJson(route, 500, {}),
  );
  await emergency
    .getByRole("menuitem", { name: "Export all device IPs" })
    .click();
  await expect(
    emergency.getByRole("alert").filter({ hasText: "IP export failed" }),
  ).toBeVisible();
  expect(downloads).toHaveLength(0);
  await context.unroute("**/api/v2/devices?**");
  await emergency
    .getByRole("menuitem", { name: "Export all device IPs" })
    .click();
  await expect(emergency.getByRole("status")).toHaveText(
    "No IP addresses available to export.",
  );
  expect(downloads).toHaveLength(0);
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
          return (await timer.count())
            ? refreshLabel(timer).textContent()
            : null;
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

    const details = await openDeviceDetails(page, "machine-150");
    await page.clock.runFor(350);
    expect(await noPageOverflow()).toBe(true);
    await expect(details).toContainText("a".repeat(64));
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
  const details = await openDeviceDetails(page, "machine-01");
  await expect(details).toContainText(fleet[0].device_id);
  await details.getByRole("button", { name: "Revoke", exact: true }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Revoke device", exact: true })
    .click();
  await expect(
    page.getByText("No devices match", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await selectDeviceStates(page, ["revoked"]);
  await expect(deviceRow(page, "machine-01")).toHaveCount(1);
  await expect(
    page.getByRole("cell", { name: "Revoked", exact: true }),
  ).toBeVisible();
  const actions = await openDeviceMenu(page, "machine-01");
  await expect(actions.getByRole("menuitem")).toHaveText([
    "Enable",
    "Unbind seat",
    "Revoke…",
  ]);
  await expect(
    actions.getByRole("menuitem", { name: "Enable", exact: true }),
  ).toBeDisabled();
  await expect(
    actions.getByRole("menuitem", { name: "Revoke…", exact: true }),
  ).toBeDisabled();
});
