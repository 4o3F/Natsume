import {
  expect,
  type BrowserContext,
  type Page,
  type Route,
} from "@playwright/test";

import type {
  BindingContextResponse,
  DeviceConvergenceResponse_Serialize as DeviceConvergenceResponse,
  DeviceListState,
  DeviceResponse_Serialize as DeviceResponse,
  TargetSubmissionBody,
  TargetSubmissionResponse,
} from "../../src/api/generated/types";

export const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
  username: "operator",
};
export const bindingContext: BindingContextResponse = {
  binding_id: "01912345-6789-7abc-8def-0123456789ac",
  account_id: "01912345-6789-7abc-8def-0123456789ad",
  credential_revision: 1,
  domjudge_username: "team1",
  seat_code: "A-01",
};
const convergence: DeviceConvergenceResponse = {
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
export const device: DeviceResponse = {
  device_id: "01923456-789a-7bcd-8ef0-123456789abc",
  machine_hardware_id: "machine-01",
  evidence_quality: "strong",
  state: "enabled",
  created_at_unix_ms: 1_700_000_000_000,
  network: null,
  convergence,
};

export function fulfillJson(route: Route, status: number, body?: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

type TargetBody = TargetSubmissionBody;
type TargetReply = TargetSubmissionResponse;

export function filterDeviceRows(rows: (typeof device)[], url: string) {
  const stateParams = new URL(url).searchParams.getAll("state");
  expect(stateParams).toHaveLength(1);
  const states = stateParams[0].split(",");
  for (const state of states)
    expect(["enabled", "disabled", "revoked"]).toContain(state);
  return rows.filter((device) => states.includes(device.state));
}

export async function selectDeviceStates(
  page: Page,
  states: DeviceListState[],
) {
  if (new URL(page.url()).pathname === "/devices") {
    await selectDeviceFacet(page, "Lifecycle", [
      ["Enabled", states.includes("enabled")],
      ["Disabled", states.includes("disabled")],
      ["Revoked", states.includes("revoked")],
    ]);
    return;
  }
  const filter = page.getByRole("group", { name: "Device state", exact: true });
  for (const state of ["enabled", "disabled", "revoked"] as const) {
    await filter
      .getByRole("checkbox", { name: new RegExp(`^${state}$`, "i") })
      .setChecked(states.includes(state));
  }
}

export async function expectDeviceStates(
  page: Page,
  states: DeviceListState[],
) {
  const dropdown = new URL(page.url()).pathname === "/devices";
  const filter = dropdown
    ? page.getByRole("menu", { name: "Lifecycle", exact: true })
    : page.getByRole("group", { name: "Device state", exact: true });
  const trigger = page.getByRole("button", { name: /^Lifecycle: / });
  if (dropdown) await trigger.click();
  for (const state of ["enabled", "disabled", "revoked"] as const) {
    await expect(
      filter.getByRole(dropdown ? "menuitemcheckbox" : "checkbox", {
        name: new RegExp(`^${state}$`, "i"),
      }),
    ).toBeChecked({ checked: states.includes(state) });
  }
  if (dropdown) {
    await page.keyboard.press("Escape");
    await expect(trigger).toBeFocused();
    const summary =
      states.length === 3
        ? "All"
        : states.length === 0
          ? "None"
          : states
              .map((state) => state[0].toUpperCase() + state.slice(1))
              .join(", ");
    await expect(trigger).toHaveAccessibleName(`Lifecycle: ${summary}`);
  }
}

async function selectDeviceFacet(
  page: Page,
  name: "Lifecycle" | "Connection" | "Binding",
  choices: [string, boolean][],
) {
  const trigger = page.getByRole("button", { name: new RegExp(`^${name}: `) });
  await trigger.click();
  const menu = page.getByRole("menu", { name, exact: true });
  for (const [label, selected] of choices) {
    const item = menu.getByRole("menuitemcheckbox", {
      name: label,
      exact: true,
    });
    if ((await item.getAttribute("aria-checked")) !== String(selected)) {
      await item.click();
      // Multi-selection must not dismiss the dropdown after each choice.
      await expect(menu).toBeVisible();
    }
    await expect(item).toBeChecked({ checked: selected });
  }
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(trigger).toBeFocused();
  const selected = choices
    .filter(([, checked]) => checked)
    .map(([label]) => label);
  const summary =
    selected.length === choices.length
      ? "All"
      : selected.length === 0
        ? "None"
        : selected.join(", ");
  await expect(trigger).toHaveAccessibleName(`${name}: ${summary}`);
}

export async function selectConnectionStates(
  page: Page,
  states: DeviceConvergenceResponse["connection_state"][],
) {
  await selectDeviceFacet(page, "Connection", [
    ["Online", states.includes("active")],
    [
      "Connected, awaiting fresh state",
      states.includes("awaiting_fresh_state"),
    ],
    ["Offline", states.includes("offline")],
  ]);
}

export async function selectBindingStates(
  page: Page,
  states: ("bound" | "unbound" | "unknown")[],
) {
  await selectDeviceFacet(page, "Binding", [
    ["Bound", states.includes("bound")],
    ["Unbound", states.includes("unbound")],
    ["Unknown", states.includes("unknown")],
  ]);
}

export async function mockTargets(
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

export async function selectTarget(page: Page, deviceId: string) {
  await page
    .getByRole("row")
    .filter({ has: page.getByTitle(deviceId, { exact: true }) })
    .getByRole("button", { name: "Manage", exact: true })
    .click();
}
export async function openTargets(page: Page) {
  await page.goto("/targets");
  await selectTarget(page, device.device_id);
}
export async function confirmTarget(page: Page, name: string) {
  await page.getByRole("button", { name, exact: true }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name, exact: true })
    .click();
}
export async function confirmAll(page: Page, name: string) {
  await page
    .getByRole("button", { name: `${name} (all)`, exact: true })
    .click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Apply to all enabled devices" })
    .click();
}
export function targetFleet(count = 5) {
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
