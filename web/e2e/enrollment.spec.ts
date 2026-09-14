import { expect, test, type Page, type Route } from "@playwright/test";

import type { components } from "../src/api/generated/schema";

type WindowState = components["schemas"]["ProvisioningWindowResponse"]["state"];

function fulfillJson(route: Route, status: number, body: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

async function mockEnrollment(
  page: Page,
  handleWindow: (route: Route) => Promise<void>,
  role: "admin" | "viewer" = "admin",
) {
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    if (pathname === "/api/v2/session" && request.method() === "GET") {
      return fulfillJson(route, 200, {
        operator_id: "01912345-6789-7abc-8def-0123456789ab",
        role,
      });
    }
    if (
      pathname === "/api/v2/enrollment-reviews" &&
      request.method() === "GET"
    ) {
      return fulfillJson(route, 200, []);
    }
    if (
      pathname === "/api/v2/provisioning-window" &&
      ["GET", "PUT"].includes(request.method())
    ) {
      return handleWindow(route);
    }
    return fulfillJson(route, 404, {});
  });
}

test("an administrator can open and close the window without duplicate submissions", async ({
  page,
}) => {
  let state: WindowState = "closed";
  const updates: unknown[] = [];
  let releaseRead!: () => void;
  const readReady = new Promise<void>((resolve) => (releaseRead = resolve));
  let releaseUpdate!: () => void;
  const updateReady = new Promise<void>((resolve) => (releaseUpdate = resolve));
  await mockEnrollment(page, async (route) => {
    const request = route.request();
    if (request.method() === "GET") {
      await readReady;
      return fulfillJson(route, 200, { state });
    }
    const body = request.postDataJSON();
    updates.push(body);
    await updateReady;
    state = body.state;
    return fulfillJson(route, 200, { state });
  });

  await page.goto("/enrollment");
  const window = page.getByRole("region", { name: "Enrollment window" });
  await expect(window.getByRole("status")).toHaveText("Loading...");
  await expect(
    window.getByRole("button", { name: "Open window" }),
  ).toBeDisabled();
  releaseRead();
  await expect(window.getByRole("status")).toHaveText("Closed");
  await window.getByRole("button", { name: "Open window" }).click();
  await expect(
    window.getByRole("button", { name: "Updating..." }),
  ).toBeDisabled();
  await expect(window.getByRole("status")).toHaveText("Closed");
  await expect.poll(() => updates).toEqual([{ state: "open" }]);
  releaseUpdate();
  await expect(window.getByRole("status")).toHaveText("Open");
  await window.getByRole("button", { name: "Close window" }).click();
  await expect(window.getByRole("status")).toHaveText("Closed");
  expect(updates).toEqual([{ state: "open" }, { state: "closed" }]);
});

test("a viewer sees window changes from the server without a write control", async ({
  page,
}) => {
  await page.clock.install();
  let state: WindowState = "open";
  const methods: string[] = [];
  await mockEnrollment(
    page,
    (route) => {
      methods.push(route.request().method());
      return fulfillJson(route, 200, { state });
    },
    "viewer",
  );

  await page.goto("/enrollment");
  const window = page.getByRole("region", { name: "Enrollment window" });
  await expect(window.getByRole("status")).toHaveText("Open");
  await expect(window.getByRole("button")).toHaveCount(0);
  state = "closed";
  await page.clock.fastForward(10_000);
  await expect(window.getByRole("status")).toHaveText("Closed");
  await expect(window.getByRole("button")).toHaveCount(0);
  expect(methods.length).toBeGreaterThanOrEqual(2);
  expect(methods.every((method) => method === "GET")).toBe(true);
});

test("an administrator can approve a pending request while the window is closed", async ({
  page,
}) => {
  const reviewId = "01912345-6789-7abc-8def-0123456789abc";
  const review = {
    review_id: reviewId,
    machine_hardware_id: "a9aa9d04-3ece-5567-8260-910930ff5e03",
    candidate_public_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    evidence_quality: "strong",
    daemon_version: "2.3.0",
    agent_version: "2.3.0",
  };
  let approved = false;
  await mockEnrollment(page, (route) => {
    expect(route.request().method()).toBe("GET");
    return fulfillJson(route, 200, { state: "closed" });
  });
  await page.route("**/api/v2/enrollment-reviews", (route) =>
    fulfillJson(route, 200, approved ? [] : [review]),
  );
  await page.route(
    `**/api/v2/enrollment-reviews/${reviewId}/actions/approve`,
    (route) => {
      expect(route.request().method()).toBe("POST");
      approved = true;
      return route.fulfill({ status: 204 });
    },
  );

  await page.goto("/enrollment");
  const window = page.getByRole("region", { name: "Enrollment window" });
  await expect(window.getByRole("status")).toHaveText("Closed");
  await expect(window).toContainText(
    "Closed keeps requests here for administrator approval.",
  );
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Approve", exact: true })
    .click();
  await expect(
    page.getByText("No enrollment reviews are pending."),
  ).toBeVisible();
  expect(approved).toBe(true);
  await expect(window.getByRole("status")).toHaveText("Closed");
});

test("a failed window read is unavailable until a successful retry", async ({
  page,
}) => {
  let failRead = true;
  await mockEnrollment(page, (route) => {
    expect(route.request().method()).toBe("GET");
    return failRead
      ? fulfillJson(route, 503, {
          code: "UNAVAILABLE",
          status: 503,
          title: "Window temporarily unavailable",
        })
      : fulfillJson(route, 200, { state: "open" });
  });

  await page.goto("/enrollment");
  const window = page.getByRole("region", { name: "Enrollment window" });
  await expect(window.getByRole("status")).toHaveText("Unavailable");
  await expect(window.getByRole("alert")).toContainText(
    "Unable to load enrollment window: Window temporarily unavailable",
  );
  await expect(
    window.getByRole("button", { name: "Open window" }),
  ).toBeDisabled();
  failRead = false;
  await window.getByRole("button", { name: "Retry" }).click();
  await expect(window.getByRole("status")).toHaveText("Open");
  await expect(
    window.getByRole("button", { name: "Close window" }),
  ).toBeEnabled();
  await expect(window.getByRole("alert")).toHaveCount(0);
});

test("a rejected update keeps the server state and can be retried", async ({
  page,
}) => {
  let state: WindowState = "closed";
  let failUpdate = true;
  await mockEnrollment(page, (route) => {
    if (route.request().method() === "GET") {
      return fulfillJson(route, 200, { state });
    }
    expect(route.request().postDataJSON()).toEqual({ state: "open" });
    if (failUpdate) {
      return fulfillJson(route, 403, {
        code: "FORBIDDEN",
        status: 403,
        title: "Administrator role required",
      });
    }
    state = "open";
    return fulfillJson(route, 200, { state });
  });

  await page.goto("/enrollment");
  const window = page.getByRole("region", { name: "Enrollment window" });
  await window.getByRole("button", { name: "Open window" }).click();
  await expect(window.getByRole("alert")).toHaveText(
    "Unable to update enrollment window: Administrator role required",
  );
  await expect(window.getByRole("status")).toHaveText("Closed");
  await expect(
    window.getByRole("button", { name: "Open window" }),
  ).toBeEnabled();
  failUpdate = false;
  await window.getByRole("button", { name: "Open window" }).click();
  await expect(window.getByRole("status")).toHaveText("Open");
  await expect(window.getByRole("alert")).toHaveCount(0);
});
