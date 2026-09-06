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
  binding: { status: "awaiting_actual", target: null, actual: null },
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
    target: { lock_state: "unlocked", terminate_epoch: null },
    actual: { session_state: "none", completed_terminate_epoch: null },
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
  await expect(page.getByText("Connection: active")).toBeVisible();
  await expect(page.getByText("Gateway: converged")).toBeVisible();
  await expect(page.getByText("Binding: awaiting actual")).toBeVisible();
  await expect(page.getByText("Runtime: converged")).toBeVisible();
  await expect(page.getByText("Session: converged")).toBeVisible();
  await expect(page.getByText("Home: converged")).toBeVisible();
  await page.getByRole("button", { name: "View" }).click();
  await expect(page.getByText("Latest state:")).toBeVisible();
  await expect(
    page.getByText("converged", { exact: true }).first(),
  ).toBeVisible();

  await page.getByRole("button", { name: "Disable" }).click();
  await expect(page.getByText("disabled", { exact: true })).toBeVisible();
});

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

    if (pathname === "/api/v2/session") {
      return fulfillJson(route, 200, operator);
    }
    if (pathname === "/api/v2/enrollment-reviews") {
      return fulfillJson(route, 200, reviews);
    }
    if (pathname.endsWith("/actions/approve")) {
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
  currentDevice: typeof device,
  operatorId = operator.operator_id,
) {
  await context.route("**/api/v2/**", async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session") {
      return fulfillJson(route, 200, { ...operator, operator_id: operatorId });
    }
    if (pathname === "/api/v2/devices") {
      return fulfillJson(route, 200, [currentDevice]);
    }
    const session = currentDevice.convergence.session_control;
    const home = currentDevice.convergence.home;
    if (pathname.endsWith("/session-control") && request.method() === "PUT") {
      session.target = {
        terminate_epoch: session.target?.terminate_epoch ?? null,
        lock_state: request.postDataJSON().lock_state,
      };
      session.status = "drifted";
      return fulfillJson(route, 200, { target: session.target });
    }
    if (pathname.endsWith("/session-control/actions/terminate")) {
      session.target = {
        lock_state: session.target?.lock_state ?? "unlocked",
        terminate_epoch: (session.target?.terminate_epoch ?? 0) + 1,
      };
      session.status = session.actual ? "reconciling" : "awaiting_actual";
      return fulfillJson(route, 200, { target: session.target });
    }
    if (pathname.endsWith("/home/actions/reset")) {
      home.target_reset_epoch = (home.target_reset_epoch ?? 0) + 1;
      home.status = home.actual ? "drifted" : "awaiting_actual";
      return fulfillJson(route, 200, { reset_epoch: home.target_reset_epoch });
    }
    return fulfillJson(route, 404, {});
  });
}

async function openTargets(page: Page) {
  await page.goto("/targets");
  await page.getByLabel("Device").selectOption(device.device_id);
}

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
  for (const [name, lockState] of [
    ["Unlock", "unlocked"],
    ["Lock", "locked"],
  ]) {
    const request = page.waitForRequest(
      (request) =>
        request.method() === "PUT" &&
        request.url().endsWith("/session-control"),
    );
    await page.getByRole("button", { name, exact: true }).click();
    expect((await request).postDataJSON()).toEqual({ lock_state: lockState });
    await expect(
      page.getByText(`Target lock: ${lockState}`, { exact: true }),
    ).toBeVisible();
  }

  await confirmTarget(page, "Terminate");
  await expect(page.getByText("Terminate epoch: 1")).toBeVisible();

  await confirmTarget(page, "Reset home");
  await expect(page.getByText("Reset epoch: 1")).toBeVisible();
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

  session.actual = { session_state: "none", completed_terminate_epoch: 1 };
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
  await expect(page.getByText("Target lock: not initialized")).toBeVisible();
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
    page.getByRole("button", { name: "Lock", exact: true }),
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

test("switching devices isolates an in-flight target submission", async ({
  page,
  context,
}) => {
  const firstDevice = structuredClone(device);
  const secondDevice = {
    ...structuredClone(device),
    device_id: "01923456-789a-7bcd-8ef0-123456789abd",
    machine_hardware_id: "machine-02",
  };
  await mockTargets(context, firstDevice);
  await context.route("**/api/v2/devices", (route) =>
    fulfillJson(route, 200, [firstDevice, secondDevice]),
  );
  const heldWrites: Route[] = [];
  await context.route("**/session-control", (route) => {
    heldWrites.push(route);
  });
  await openTargets(page);
  await page.getByRole("button", { name: "Lock", exact: true }).click();
  await expect.poll(() => heldWrites.length).toBe(1);
  await expect(
    page.getByRole("button", { name: "Lock", exact: true }),
  ).toBeDisabled();

  await page.getByLabel("Device").selectOption(secondDevice.device_id);
  await expect(
    page.getByRole("button", { name: "Lock", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByText("Target lock: unlocked", { exact: true }),
  ).toBeVisible();
  const refreshed = page.waitForResponse((response) =>
    response.url().endsWith("/api/v2/devices"),
  );
  await heldWrites[0].fallback();
  await refreshed;
  await expect(
    page.getByText("Target lock: unlocked", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Target submitted.", { exact: false }),
  ).toHaveCount(0);
  await page.getByLabel("Device").selectOption(firstDevice.device_id);
  await expect(
    page.getByText("Target lock: locked", { exact: true }),
  ).toBeVisible();
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
