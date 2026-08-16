import { expect, test, type Page, type Route } from "@playwright/test";

const admin = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const viewer = {
  ...admin,
  operator_id: "01912345-6789-7abc-8def-0123456789ac",
  role: "viewer",
};
const correlationId = "01945678-9abc-7def-8012-3456789abcde";
const pendingRequestId = "01934567-89ab-7cde-8f01-23456789abcd";
const approvedRequestId = "01934567-89ab-7cde-8f01-23456789abce";
const pendingHardwareId = "dba321cf-4683-5e48-b50d-4b010e167b8a";
const approvedHardwareId = "a1da35c2-5043-5713-b753-aed875892dc3";

const initialRequests = [
  {
    enrollment_request_id: pendingRequestId,
    machine_hardware_id: pendingHardwareId,
    hardware_identity_quality: "strong",
    gateway_spki_sha256: "a".repeat(64),
    client_version: "2.0.0-device",
    protocol_version: 1,
    state: "pending",
    resolution: "replace_device_credentials",
    resolved_device_id: "01956789-abcd-7ef0-8123-456789abcdef",
    created_at: "2026-08-16T09:00:00.000Z",
    source_ip: "192.0.2.41",
  },
  {
    enrollment_request_id: approvedRequestId,
    machine_hardware_id: approvedHardwareId,
    hardware_identity_quality: "medium",
    gateway_spki_sha256: "b".repeat(64),
    client_version: "2.0.0-retry",
    protocol_version: 1,
    state: "approved",
    resolution: "replace_device_credentials",
    resolved_device_id: "01956789-abcd-7ef0-8123-456789abcdee",
    created_at: "2026-08-16T09:01:00.000Z",
    source_ip: "192.0.2.42",
  },
] as const;

function fulfillJson(route: Route, status: number, body: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    headers: { "X-Correlation-Id": correlationId },
    body: JSON.stringify(body),
  });
}

async function mockEnrollmentApi(
  page: Page,
  role: "admin" | "viewer" = "admin",
) {
  let requests = initialRequests.map((request) => ({ ...request }));
  let window = { state: "open", revision: 7 };
  const calls: string[] = [];

  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session" && request.method() === "GET") {
      return fulfillJson(route, 200, role === "admin" ? admin : viewer);
    }
    if (
      pathname === "/api/v2/enrollment-requests" &&
      request.method() === "GET"
    ) {
      return fulfillJson(route, 200, requests);
    }
    if (
      pathname === "/api/v2/provisioning-window" &&
      request.method() === "GET"
    ) {
      return fulfillJson(route, 200, window);
    }
    if (
      pathname ===
        `/api/v2/enrollment-requests/${pendingRequestId}/actions/approve` &&
      request.method() === "POST"
    ) {
      calls.push(pathname);
      requests = requests.map((item) =>
        item.enrollment_request_id === pendingRequestId
          ? { ...item, state: "approved" }
          : item,
      );
      return fulfillJson(route, 200, {
        enrollment_request_id: pendingRequestId,
        state: "approved",
      });
    }
    if (
      pathname ===
        `/api/v2/enrollment-requests/${pendingRequestId}/actions/reject` &&
      request.method() === "POST"
    ) {
      calls.push(pathname);
      requests = requests.filter(
        (item) => item.enrollment_request_id !== pendingRequestId,
      );
      return fulfillJson(route, 200, {
        enrollment_request_id: pendingRequestId,
        state: "rejected",
      });
    }
    if (
      pathname === "/api/v2/provisioning-window/actions/close" &&
      request.method() === "POST"
    ) {
      calls.push(pathname);
      window = { state: "closed", revision: window.revision + 1 };
      requests = [];
      return fulfillJson(route, 200, window);
    }
    if (
      pathname === "/api/v2/provisioning-window/actions/open" &&
      request.method() === "POST"
    ) {
      calls.push(pathname);
      window = { state: "open", revision: window.revision + 1 };
      return fulfillJson(route, 200, window);
    }

    return fulfillJson(route, 200, []);
  });

  return calls;
}

test("Enrollment list renders redacted review rows", async ({ page }) => {
  await mockEnrollmentApi(page);
  await page.goto("/enrollment");

  await expect(page.getByRole("heading", { name: "Enrollment" })).toBeVisible();
  await expect(page.getByTitle(pendingHardwareId)).toBeVisible();
  await expect(page.getByTitle("a".repeat(64))).toHaveText("a".repeat(12));
  await expect(
    page.getByText("2.0.0-device / v1", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("strong", { exact: true })).toBeVisible();
  await expect(page.getByText("approved", { exact: true })).toBeVisible();
  const links = page.getByRole("navigation").getByRole("link");
  await expect(links.nth(0)).toHaveText("Preparation");
  await expect(links.nth(1)).toHaveText("Enrollment");
});

test("approve confirmation posts and refreshes the row state", async ({
  page,
}) => {
  const calls = await mockEnrollmentApi(page);
  await page.goto("/enrollment");

  const row = page.getByRole("row").filter({ hasText: pendingHardwareId });
  await row.getByRole("button", { name: "Approve" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText(
    "Approving lets this device claim credentials for that hardware id.",
  );
  await dialog.getByRole("button", { name: "Confirm approve" }).click();

  await expect(row.getByText("approved", { exact: true })).toBeVisible();
  await expect(row.getByRole("button", { name: "Approve" })).toHaveCount(0);
  await expect(
    page.getByRole("alert").filter({ hasText: "Enrollment approved" }),
  ).toBeVisible();
  expect(calls).toContain(
    `/api/v2/enrollment-requests/${pendingRequestId}/actions/approve`,
  );
});

test("reject confirmation posts and removes the terminal row after refetch", async ({
  page,
}) => {
  const calls = await mockEnrollmentApi(page);
  await page.goto("/enrollment");

  const row = page.getByRole("row").filter({ hasText: pendingHardwareId });
  await row.getByRole("button", { name: "Reject" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText(
    "Rejecting is terminal for this hardware id until the window closes.",
  );
  await dialog.getByRole("button", { name: "Confirm reject" }).click();

  await expect(row).toHaveCount(0);
  await expect(
    page.getByRole("alert").filter({ hasText: "Enrollment rejected" }),
  ).toBeVisible();
  expect(calls).toContain(
    `/api/v2/enrollment-requests/${pendingRequestId}/actions/reject`,
  );
});

test("window close confirmation expires requests and refreshes the badge", async ({
  page,
}) => {
  const calls = await mockEnrollmentApi(page);
  await page.goto("/enrollment");

  await page.getByRole("button", { name: "Close window" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText(
    "Closing expires every unclaimed enrollment request.",
  );
  await dialog.getByRole("button", { name: "Confirm close" }).click();

  await expect(page.getByText("closed", { exact: true })).toBeVisible();
  await expect(page.getByText("No live enrollment requests.")).toBeVisible();
  expect(calls).toContain("/api/v2/provisioning-window/actions/close");
});

test("viewer sees current facts without mutation controls", async ({
  page,
}) => {
  await mockEnrollmentApi(page, "viewer");
  await page.goto("/enrollment");

  await expect(page.getByTitle(pendingHardwareId)).toBeVisible();
  await expect(page.getByText("open", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Approve" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Reject" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open window" })).toHaveCount(
    0,
  );
  await expect(page.getByRole("button", { name: "Close window" })).toHaveCount(
    0,
  );
});
