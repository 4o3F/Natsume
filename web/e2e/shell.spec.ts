import { expect, test, type Route } from "@playwright/test";

const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const correlationId = "01923456-789a-7bcd-8ef0-123456789abc";

function fulfillJson(route: Route, status: number, body: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

const unauthorized = {
  title: "No valid session",
  status: 401,
  code: "AUTHENTICATION_FAILED",
  correlation_id: correlationId,
};

test("unauthenticated visits redirect to the login form", async ({ page }) => {
  await page.route("**/api/v2/**", (route) =>
    fulfillJson(route, 401, unauthorized),
  );

  await page.goto("/");

  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByLabel("Login name")).toBeVisible();
  await expect(page.getByLabel("Password")).toBeVisible();
  await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();
});

test("successful login reaches the authenticated shell", async ({ page }) => {
  let authenticated = false;

  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session" && request.method() === "POST") {
      authenticated = true;
      return fulfillJson(route, 200, operator);
    }

    if (pathname === "/api/v2/session" && request.method() === "GET") {
      return authenticated
        ? fulfillJson(route, 200, operator)
        : fulfillJson(route, 401, unauthorized);
    }

    return fulfillJson(route, 200, []);
  });

  await page.goto("/");
  await page.getByLabel("Login name").fill("operator");
  await page.getByLabel("Password").fill("correct password");
  await page.getByRole("button", { name: "Sign in" }).click();

  await expect(page.getByRole("link", { name: "Preparation" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Seats" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Accounts" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Bindings" })).toBeVisible();
  await expect(page.getByText("ADMIN", { exact: true })).toBeVisible();
});

test("failed login surfaces the coded error", async ({ page }) => {
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();

    if (request.method() === "POST") {
      return fulfillJson(route, 401, {
        title: "Authentication failed",
        status: 401,
        code: "AUTHENTICATION_FAILED",
        correlation_id: correlationId,
      });
    }

    return fulfillJson(route, 401, unauthorized);
  });

  await page.goto("/login");
  await page.getByLabel("Login name").fill("operator");
  await page.getByLabel("Password").fill("wrong password");
  await page.getByRole("button", { name: "Sign in" }).click();

  const alert = page.getByRole("alert");
  await expect(alert).toContainText("Login failed: Authentication failed");
  await expect(alert).toContainText(`Correlation ID: ${correlationId}`);
});

test("seats page renders rows", async ({ page }) => {
  await page.route("**/api/v2/**", (route) => {
    const pathname = new URL(route.request().url()).pathname;

    if (pathname === "/api/v2/session") {
      return fulfillJson(route, 200, operator);
    }

    if (pathname === "/api/v2/seats") {
      return fulfillJson(route, 200, [
        { seat_id: "seat-001", seat_code: "A-01" },
        { seat_id: "seat-002", seat_code: "A-02" },
      ]);
    }

    return fulfillJson(route, 200, []);
  });

  await page.goto("/seats");

  await expect(page.getByText("A-01", { exact: true })).toBeVisible();
  await expect(page.getByText("A-02", { exact: true })).toBeVisible();
});

test("expired session redirects to login during polling", async ({ page }) => {
  let expired = false;

  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (expired && request.method() === "GET") {
      return fulfillJson(route, 401, unauthorized);
    }

    if (pathname === "/api/v2/session") {
      return fulfillJson(route, 200, operator);
    }

    if (pathname === "/api/v2/seats") {
      return fulfillJson(route, 200, [
        { seat_id: "seat-001", seat_code: "A-01" },
      ]);
    }

    return fulfillJson(route, 200, []);
  });

  await page.goto("/seats");
  await expect(page.getByText("A-01", { exact: true })).toBeVisible();

  expired = true;

  await expect(page).toHaveURL(/\/login$/, { timeout: 15_000 });
});
