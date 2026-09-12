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
  await expect(page.getByText("Connection: active")).toBeVisible();
  await expect(page.getByText("Gateway: converged")).toBeVisible();
  await expect(page.getByText("Binding: converged")).toBeVisible();
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

    await page.getByLabel("Device").selectOption(secondDevice.device_id);
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
    await page.getByLabel("Device").selectOption(firstDevice.device_id);
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
    await page.getByLabel("Device").selectOption(secondDevice.device_id);
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
    await page.getByLabel("Device").selectOption(firstDevice.device_id);
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
