import { expect, test, type Page } from "@playwright/test";
import {
  closeDetails,
  closeNavigation,
  deviceRow,
  openDeviceDetails,
  openDeviceMenu,
  openNavigation,
  seatCells,
} from "./fixtures/ui";

import type {
  BindingContextResponse,
  DeviceResponse_Serialize as DeviceResponse,
} from "../src/api/generated/types";

type Device = DeviceResponse;

const devices: Device[] = ["A-01", "A-2", "A-10", null, null].map(
  (seatCode, index) => {
    const context: BindingContextResponse = {
      binding_id: `01923456-789a-7bcd-8ef0-123456789b0${index}`,
      account_id: "01923456-789a-7bcd-8ef0-123456789a01",
      credential_revision: 1,
      domjudge_username: `team${index + 1}`,
      seat_code: seatCode ?? "OLD-SEAT",
    };
    return {
      device_id: `01923456-789a-7bcd-8ef0-123456789c0${index}`,
      machine_hardware_id: `machine-0${index + 1}`,
      evidence_quality: "strong",
      state: index === 2 ? "disabled" : "enabled",
      created_at_unix_ms: 1_700_000_000_000,
      network: null,
      convergence: {
        connection_state: index === 1 || index === 2 ? "offline" : "active",
        received_at_unix_ms: 1_700_000_100_000,
        binding: {
          status: "awaiting_actual",
          target: seatCode
            ? { state: "bound", context }
            : index === 3
              ? { state: "unbound", negotiation_id: context.binding_id }
              : null,
          // Stale Actual must not supply a seat or an Unbind action.
          actual: {
            assignment_state: "applied",
            credential_state: "applied",
            context,
          },
        },
        gateway: { status: "awaiting_actual", target: null, actual: null },
        runtime_config: {
          status: "awaiting_actual",
          target_domjudge_origin: null,
          actual: null,
        },
        session_control: {
          status: "awaiting_actual",
          target: null,
          actual: null,
        },
        home: {
          status: "awaiting_actual",
          target_reset_epoch: null,
          actual: null,
        },
      },
    };
  },
);

async function mockDevices(
  page: Page,
  { role = "admin", devicesFail = false, unbindFail = false } = {},
) {
  const currentDevices = structuredClone(devices);
  const api = { deleted: [] as string[], deviceReads: 0 };
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    let body: unknown;
    if (request.method() === "GET") {
      if (path === "/api/v2/session") {
        body = {
          operator_id: "01912345-6789-7abc-8def-0123456789ab",
          role,
          username: "operator",
        };
      } else if (path === "/api/v2/devices") {
        api.deviceReads += 1;
        if (devicesFail) {
          return route.fulfill({
            status: 500,
            contentType: "application/problem+json",
            body: JSON.stringify({
              code: "INTERNAL_ERROR",
              title: "Devices unavailable",
              status: 500,
            }),
          });
        }
        const states = url.searchParams.get("state")?.split(",") ?? [];
        body = currentDevices.filter((device) => states.includes(device.state));
      } else if (path === "/api/v2/seats" || path === "/api/v2/bindings") {
        body = [];
      }
    } else if (request.method() === "DELETE") {
      const device = currentDevices.find(
        (device) => path === `/api/v2/devices/${device.device_id}/binding`,
      );
      if (device) {
        api.deleted.push(device.device_id);
        if (unbindFail) {
          return route.fulfill({
            status: 409,
            contentType: "application/problem+json",
            body: JSON.stringify({
              code: "CONFLICT",
              title: "Unbind rejected",
              status: 409,
            }),
          });
        }
        device.convergence.binding.target = {
          state: "unbound",
          negotiation_id: "01923456-789a-7bcd-8ef0-123456789b00",
        };
        return route.fulfill({ status: 204 });
      }
    }
    return route.fulfill({
      status: body === undefined ? 404 : 200,
      contentType: "application/json",
      body: JSON.stringify(body ?? {}),
    });
  });
  return api;
}

for (const width of [390, 1024, 1440]) {
  test(`merged devices identify seats and unbind the selected device at ${width}`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    const api = await mockDevices(page);
    await page.clock.install();
    await page.goto("/bindings");
    await expect(page).toHaveURL(/\/devices$/);
    const navigation = await openNavigation(page);
    await expect(
      navigation.getByRole("link", { name: "Bindings", exact: true }),
    ).toHaveCount(0);
    await expect(
      navigation.getByRole("link", { name: "Seats", exact: true }),
    ).toBeVisible();
    await expect(
      navigation.getByRole("link", { name: "Devices", exact: true }),
    ).toHaveAttribute("aria-current", "page");
    await closeNavigation(page);
    const rows = page.locator("tbody tr");
    await expect(await seatCells(page)).toHaveText([
      "A-01",
      "A-2",
      "A-10",
      "—",
      "—",
    ]);
    for (const name of ["Seat ID", "Binding ID"]) {
      await expect(
        page.getByRole("columnheader", { name, exact: true }),
      ).toHaveCount(0);
    }
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    const actionPositions = await rows
      .locator("td:last-child")
      .evaluateAll((cells) =>
        cells.map((cell) =>
          Array.from(cell.querySelectorAll("button"), (button) => {
            const bounds = button.getBoundingClientRect();
            return { x: bounds.x, width: bounds.width };
          }),
        ),
      );
    expect(actionPositions[0]).toHaveLength(1);
    expect(actionPositions[0][0].width).toBeGreaterThanOrEqual(32);
    for (const positions of actionPositions) {
      expect(positions).toEqual(actionPositions[0]);
    }
    await page.screenshot({ path: testInfo.outputPath("merged-devices.png") });

    const search = page.getByRole("searchbox", { name: "Search devices" });
    await search.fill(" a-01 ");
    await expect(rows).toHaveCount(1);
    await (
      await openDeviceMenu(page, "machine-01")
    )
      .getByRole("menuitem", { name: "Unbind seat", exact: true })
      .click();
    const dialog = page.getByRole("alertdialog", { name: "Unbind seat A-01?" });
    await expect(dialog).toBeVisible();
    await expect(
      dialog.getByText(devices[0].machine_hardware_id, { exact: true }),
    ).toBeVisible();
    await expect(
      dialog.getByText(devices[0].device_id, { exact: true }),
    ).toBeVisible();
    await page.screenshot({
      path: testInfo.outputPath("unbind-confirmation.png"),
    });
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(api.deleted).toEqual([]);
    await expect(
      rows.getByRole("cell", { name: "A-01", exact: true }),
    ).toBeVisible();
    const readsBeforeUnbind = api.deviceReads;
    await (
      await openDeviceMenu(page, "machine-01")
    )
      .getByRole("menuitem", { name: "Unbind seat", exact: true })
      .click();
    await dialog.getByRole("button", { name: "Unbind", exact: true }).click();
    await expect(
      page.getByText("No devices match", { exact: true }),
    ).toBeVisible();
    expect(api.deleted).toEqual([devices[0].device_id]);
    expect(api.deviceReads).toBeGreaterThan(readsBeforeUnbind);
    await search.clear();
    // Unbinding changes the durable target; it does not delete the device.
    await expect(rows).toHaveCount(5);
    const unboundRow = deviceRow(page, "machine-01");
    await expect(unboundRow.getByRole("cell").nth(1)).toHaveText("—");
    await expect(
      unboundRow.getByRole("img", { name: /^Binding: unbound,/ }),
    ).toBeVisible();
    await expect(
      (await openDeviceMenu(page, "machine-01")).getByRole("menuitem", {
        name: "Unbind seat",
        exact: true,
      }),
    ).toBeDisabled();
  });
}

test("legacy bindings redirect replaces history rather than adding a navigation entry", async ({
  page,
}) => {
  await mockDevices(page);
  await page.goto("/seats");
  await expect(
    page.getByRole("heading", { name: "Seats", exact: true }),
  ).toBeVisible();
  await page.goto("/bindings");
  await expect(page).toHaveURL(/\/devices$/);
  await page.goBack();
  await expect(page).toHaveURL(/\/seats$/);
});

test("viewers can find bound devices without unbind controls", async ({
  page,
}) => {
  await mockDevices(page, { role: "viewer" });
  await page.goto("/devices");
  await expect(
    page.getByRole("cell", { name: "A-01", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: /^Actions for / })).toHaveCount(
    0,
  );
  const details = await openDeviceDetails(page, "machine-01");
  await expect(
    details.getByRole("button", { name: /^(Unbind|Disable|Revoke)$/ }),
  ).toHaveCount(0);
  await closeDetails(page);
  await page.getByRole("searchbox", { name: "Search devices" }).fill("A-10");
  await expect(page.locator("tbody tr")).toHaveCount(1);
});

test("offline and disabled bound devices can Unbind while unknown and unbound targets keep the action disabled", async ({
  page,
}) => {
  await mockDevices(page);
  await page.goto("/devices");
  for (const [hardware, seat] of [
    ["machine-02", "A-2"],
    ["machine-03", "A-10"],
  ]) {
    const row = page.getByRole("row").filter({ hasText: hardware });
    await expect(
      row.getByRole("img", { name: "Connection: Offline", exact: true }),
    ).toBeVisible();
    const menu = await openDeviceMenu(page, hardware);
    await menu
      .getByRole("menuitem", { name: "Unbind seat", exact: true })
      .focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("alertdialog", {
      name: `Unbind seat ${seat}?`,
    });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(
      row.getByRole("button", { name: /^Actions for / }),
    ).toBeFocused();
  }
  await expect(
    page
      .getByRole("row")
      .filter({ hasText: "machine-03" })
      .getByRole("cell", { name: "Disabled", exact: true }),
  ).toBeVisible();
  for (const [hardware, state] of [
    ["machine-04", "unbound"],
    ["machine-05", "unknown"],
  ]) {
    const row = page.getByRole("row").filter({ hasText: hardware });
    await expect(row.getByRole("cell").nth(1)).toHaveText("—");
    await expect(
      row.getByRole("img", { name: new RegExp(`^Binding: ${state},`) }),
    ).toBeVisible();
    await expect(
      (await openDeviceMenu(page, hardware)).getByRole("menuitem", {
        name: "Unbind seat",
        exact: true,
      }),
    ).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
  }
  await page
    .getByRole("searchbox", { name: "Search devices" })
    .fill("OLD-SEAT");
  await expect(
    page.getByText("No devices match", { exact: true }),
  ).toBeVisible();
});

test("unbind failure is visible and leaves the device bound", async ({
  page,
}) => {
  const api = await mockDevices(page, { unbindFail: true });
  await page.goto("/devices");
  const row = deviceRow(page, "machine-01");
  await (
    await openDeviceMenu(page, "machine-01")
  )
    .getByRole("menuitem", { name: "Unbind seat", exact: true })
    .click();
  await page
    .getByRole("alertdialog", { name: "Unbind seat A-01?" })
    .getByRole("button", { name: "Unbind", exact: true })
    .click();
  await expect(
    page.locator("[data-sonner-toast]").filter({ hasText: "Unbind rejected" }),
  ).toBeVisible();
  expect(api.deleted).toEqual([devices[0].device_id]);
  await expect(
    row.getByRole("cell", { name: "A-01", exact: true }),
  ).toBeVisible();
  await expect(
    (await openDeviceMenu(page, "machine-01")).getByRole("menuitem", {
      name: "Unbind seat",
      exact: true,
    }),
  ).toBeEnabled();
});

test("device list failure is visible before offering unbind controls", async ({
  page,
}) => {
  await mockDevices(page, { devicesFail: true });
  await page.goto("/bindings");
  await expect(page).toHaveURL(/\/devices$/);
  await expect(
    page.getByText("Devices unavailable", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Unbind", exact: true }),
  ).toHaveCount(0);
});
