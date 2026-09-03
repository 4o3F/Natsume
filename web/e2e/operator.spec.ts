import { expect, test, type Route } from "@playwright/test";

const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const convergence = {
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
const device = {
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

test("target controls call their generated API operations", async ({
  page,
}) => {
  let sessionTarget = {
    lock_state: "unlocked",
    terminate_epoch: null as number | null,
  };
  let resetEpoch: number | null = null;

  await page.route("**/api/v2/**", async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session") {
      return fulfillJson(route, 200, operator);
    }
    if (pathname === "/api/v2/devices") {
      return fulfillJson(route, 200, [device]);
    }
    if (pathname.endsWith("/session-control")) {
      if (request.method() === "PUT") {
        sessionTarget = {
          ...sessionTarget,
          lock_state: request.postDataJSON().lock_state,
        };
      }
      return fulfillJson(route, 200, { target: sessionTarget });
    }
    if (pathname.endsWith("/session-control/actions/terminate")) {
      sessionTarget = { ...sessionTarget, terminate_epoch: 1 };
      return fulfillJson(route, 200, { target: sessionTarget });
    }
    if (pathname.endsWith("/home")) {
      return fulfillJson(route, 200, { reset_epoch: resetEpoch });
    }
    if (pathname.endsWith("/home/actions/reset")) {
      resetEpoch = 1;
      return fulfillJson(route, 200, { reset_epoch: resetEpoch });
    }
    return fulfillJson(route, 404, {});
  });

  await page.goto("/targets");
  await page.getByLabel("Device").selectOption(device.device_id);
  await page.getByRole("button", { name: "Lock" }).click();
  await expect(page.getByRole("button", { name: "Unlock" })).toBeVisible();

  await page.getByRole("button", { name: "Terminate" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Terminate" })
    .click();
  await expect(page.getByText("Terminate epoch: 1")).toBeVisible();

  await page.getByRole("button", { name: "Reset home" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Reset home" })
    .click();
  await expect(page.getByText("Reset epoch: 1")).toBeVisible();
});
