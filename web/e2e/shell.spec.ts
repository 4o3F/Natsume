import { expect, test, type Route } from "@playwright/test";

const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
  username: "operator",
};

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
  await expect(page.getByRole("link", { name: "Devices" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Enrollment" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Targets" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Account menu for operator" }),
  ).toBeVisible();
  await expect(page.getByText("ADMIN", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Account menu for operator" }).click();
  await expect(page.getByText("ADMIN", { exact: true })).toBeVisible();
});

for (const role of ["admin", "viewer"]) {
  for (const width of [1440, 390]) {
    test(`${role} account dropdown groups actions and role at ${width}px`, async ({
      page,
    }, testInfo) => {
      await page.setViewportSize({ width, height: 900 });
      let authenticated = true;
      let logoutCount = 0;
      await page.route("**/api/v2/**", (route) => {
        const request = route.request();
        const path = new URL(request.url()).pathname;
        if (path === "/api/v2/session") {
          if (request.method() === "DELETE") {
            logoutCount += 1;
            authenticated = false;
            return route.fulfill({ status: 204 });
          }
          return authenticated
            ? fulfillJson(route, 200, { ...operator, role })
            : fulfillJson(route, 401, unauthorized);
        }
        if (path === "/api/v2/seats") {
          return fulfillJson(route, 200, [
            { seat_id: "seat-001", seat_code: "A-01" },
            { seat_id: "seat-002", seat_code: "A-02" },
          ]);
        }
        return fulfillJson(route, 200, []);
      });
      await page.goto("/seats");
      await expect(page.getByText("A-01", { exact: true })).toBeVisible();
      const trigger = page.getByRole("button", {
        name: "Account menu for operator",
      });
      const menu = page.getByRole("menu");
      await expect(trigger).toHaveText("operator");
      await expect(trigger).toHaveAttribute("aria-expanded", "false");
      await expect(menu).toHaveCount(0);
      await expect(
        page.getByText(role.toUpperCase(), { exact: true }),
      ).toHaveCount(0);
      if (role === "admin") {
        await page.screenshot({
          path: testInfo.outputPath("account-closed.png"),
        });
      }
      await trigger.click();
      await expect(trigger).toHaveAttribute("aria-expanded", "true");
      await expect(
        menu.getByText(role.toUpperCase(), { exact: true }),
      ).toBeVisible();
      await expect(
        menu.getByRole("menuitem", { name: "Change password" }),
      ).toBeVisible();
      await expect(
        menu.getByRole("menuitem", { name: "Logout" }),
      ).toBeVisible();
      const bounds = await menu.boundingBox();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
      if (role === "admin") {
        await page.screenshot({
          path: testInfo.outputPath("account-open.png"),
        });
      }
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);
      await expect(trigger).toBeFocused();
      await trigger.press("ArrowDown");
      await expect(
        menu.getByRole("menuitem", { name: "Change password" }),
      ).toBeFocused();
      await page.keyboard.press("ArrowDown");
      await expect(
        menu.getByRole("menuitem", { name: "Logout" }),
      ).toBeFocused();
      await page.keyboard.press("Escape");
      await trigger.click();
      await page.getByText("Natsume", { exact: true }).click();
      await expect(menu).toHaveCount(0);
      await trigger.click();
      await menu.getByRole("menuitem", { name: "Change password" }).click();
      await expect(page).toHaveURL(/\/change-password$/);
      await expect(page.getByLabel("Current password")).toBeVisible();
      await expect(menu).toHaveCount(0);
      await trigger.click();
      await menu.getByRole("menuitem", { name: "Logout" }).click();
      await expect(page).toHaveURL(/\/login$/);
      expect(logoutCount).toBe(1);
    });
  }
}

test("navbar highlights only the current page across navigation and reloads", async ({
  page,
}) => {
  await page.route("**/api/v2/**", (route) => {
    expect(route.request().method()).toBe("GET");
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session") return fulfillJson(route, 200, operator);
    if (path === "/api/v2/imports")
      return fulfillJson(route, 200, { pending: null });
    if (path === "/api/v2/provisioning-window")
      return fulfillJson(route, 200, { state: "closed" });
    if (
      [
        "/api/v2/seats",
        "/api/v2/accounts",
        "/api/v2/bindings",
        "/api/v2/devices",
        "/api/v2/organizations",
        "/api/v2/enrollment-reviews",
      ].includes(path)
    )
      return fulfillJson(route, 200, []);
    return fulfillJson(route, 404, {});
  });
  await page.goto("/preparation");
  const navigation = page.getByRole("navigation", {
    name: "Primary navigation",
  });
  for (const name of [
    "Preparation",
    "Seats",
    "Accounts",
    "Bindings",
    "Devices",
    "Enrollment",
    "Targets",
  ]) {
    const link = navigation.getByRole("link", { name, exact: true });
    await link.click();
    await expect(link).toHaveAttribute("aria-current", "page");
    await expect(navigation.locator('a[aria-current="page"]')).toHaveCount(1);
    await expect(navigation.locator("a.bg-primary")).toHaveCount(1);
    await expect(link).toHaveClass(/bg-primary/);
    const inactive = navigation
      .getByRole("link")
      .filter({ hasNotText: name })
      .first();
    expect(
      await link.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      ),
    ).not.toBe(
      await inactive.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      ),
    );
  }
  await page.goBack();
  await expect(
    navigation.getByRole("link", { name: "Enrollment", exact: true }),
  ).toHaveAttribute("aria-current", "page");
  await page.reload();
  await expect(navigation.locator('a[aria-current="page"]')).toHaveText(
    "Enrollment",
  );
  await expect(navigation.locator("a.bg-primary")).toHaveCount(1);
});

test("long usernames stay within the mobile header and menu", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const username = "用户名".repeat(14);
  await page.route("**/api/v2/**", (route) =>
    fulfillJson(
      route,
      200,
      new URL(route.request().url()).pathname === "/api/v2/session"
        ? { ...operator, username }
        : [],
    ),
  );
  await page.goto("/seats");
  const trigger = page.getByRole("button", {
    name: `Account menu for ${username}`,
  });
  await expect(trigger).toHaveAttribute("title", username);
  await trigger.click();
  await expect(
    page.getByRole("menu").getByText(username, { exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(
    390,
  );
});

test("pending logout cannot be submitted twice and failures remain visible", async ({
  page,
}) => {
  let heldLogout: Route | undefined;
  let logoutCount = 0;
  await page.route("**/api/v2/**", (route) => {
    if (route.request().method() === "DELETE") {
      logoutCount += 1;
      heldLogout = route;
      return;
    }
    return fulfillJson(
      route,
      200,
      new URL(route.request().url()).pathname === "/api/v2/session"
        ? operator
        : [],
    );
  });
  await page.goto("/seats");
  const trigger = page.getByRole("button", {
    name: "Account menu for operator",
  });
  await trigger.click();
  await page.getByRole("menuitem", { name: "Logout" }).click();
  await expect.poll(() => logoutCount).toBe(1);
  await trigger.click();
  await expect(page.getByRole("menuitem", { name: "Logout" })).toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await fulfillJson(heldLogout!, 503, {
    title: "Unable to log out",
    status: 503,
    code: "SERVICE_UNAVAILABLE",
  });
  await expect(page.getByRole("alert")).toContainText("Unable to log out");
  await expect(
    page.getByRole("menuitem", { name: "Logout" }),
  ).not.toHaveAttribute("aria-disabled", "true");
  expect(logoutCount).toBe(1);
});

test("failed login surfaces the coded error", async ({ page }) => {
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();

    if (request.method() === "POST") {
      return fulfillJson(route, 401, {
        title: "Authentication failed",
        status: 401,
        code: "AUTHENTICATION_FAILED",
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
