import { expect, test, type Route } from "@playwright/test";
import {
  closeNavigation,
  openAccountMenu,
  openNavigation,
} from "./fixtures/ui";

const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
  username: "operator",
};
const unauthorized = {
  title: "No valid session",
  status: 401,
  code: "AUTHENTICATION_FAILED",
};
function fulfillJson(route: Route, status: number, body: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

test("unauthenticated visits redirect to the login form", async ({ page }) => {
  await page.route("**/api/v2/**", (route) =>
    fulfillJson(route, 401, unauthorized),
  );
  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByLabel("Login name")).toBeVisible();
  await expect(page.getByLabel("Password")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Sign in", exact: true }),
  ).toBeVisible();
});

test("successful login reaches the authenticated shell", async ({ page }) => {
  let authenticated = false;
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    if (new URL(request.url()).pathname === "/api/v2/session") {
      if (request.method() === "POST") authenticated = true;
      return fulfillJson(
        route,
        authenticated ? 200 : 401,
        authenticated ? operator : unauthorized,
      );
    }
    return fulfillJson(route, 200, []);
  });
  await page.goto("/");
  await page.getByLabel("Login name").fill("operator");
  await page.getByLabel("Password").fill("correct password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  const navigation = await openNavigation(page);
  for (const name of [
    "Preparation",
    "Seats",
    "Accounts",
    "Devices",
    "Enrollment",
    "Targets",
  ])
    await expect(
      navigation.getByRole("link", { name, exact: true }),
    ).toBeVisible();
  await expect(
    navigation.getByRole("link", { name: "Bindings", exact: true }),
  ).toHaveCount(0);
  const menu = await openAccountMenu(page, "operator");
  await expect(menu.getByText("admin", { exact: true })).toBeVisible();
});

for (const role of ["admin", "viewer"])
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
            logoutCount++;
            authenticated = false;
            return route.fulfill({ status: 204 });
          }
          return fulfillJson(
            route,
            authenticated ? 200 : 401,
            authenticated ? { ...operator, role } : unauthorized,
          );
        }
        return fulfillJson(
          route,
          200,
          path === "/api/v2/seats"
            ? [
                { seat_id: "seat-001", seat_code: "A-01" },
                { seat_id: "seat-002", seat_code: "A-02" },
              ]
            : [],
        );
      });
      await page.goto("/seats");
      await expect(page.getByText("A-01", { exact: true })).toBeVisible();
      await openNavigation(page);
      const trigger = page.getByRole("button", {
        name: "Account menu for operator",
        exact: true,
      });
      await expect(trigger).toContainText("operator");
      await expect(trigger).toContainText(role);
      await expect(trigger).toHaveAttribute("aria-expanded", "false");
      const menu = await openAccountMenu(page, "operator");
      await expect(trigger).toHaveAttribute("aria-expanded", "true");
      await expect(menu.getByText(role, { exact: true })).toBeVisible();
      await expect(
        menu.getByRole("menuitem", { name: "Change password", exact: true }),
      ).toBeVisible();
      await expect(
        menu.getByRole("menuitem", { name: "Log out", exact: true }),
      ).toBeVisible();
      const bounds = (await menu.boundingBox())!;
      expect(bounds.x).toBeGreaterThanOrEqual(0);
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
      await page.screenshot({ path: testInfo.outputPath("account-open.png") });
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);
      await expect(trigger).toBeFocused();
      await trigger.press("ArrowDown");
      await expect(
        menu.getByRole("menuitem", { name: "Change password", exact: true }),
      ).toBeFocused();
      await page.keyboard.press("ArrowDown");
      await expect(
        menu.getByRole("menuitem", { name: "Log out", exact: true }),
      ).toBeFocused();
      await page.keyboard.press("Escape");
      await trigger.click();
      await (
        width < 768
          ? page.getByRole("dialog", { name: "Navigation", exact: true })
          : page.locator("aside")
      )
        .getByText("Natsume", { exact: true })
        .click();
      await expect(menu).toHaveCount(0);
      await trigger.click();
      await menu
        .getByRole("menuitem", { name: "Change password", exact: true })
        .click();
      await expect(page).toHaveURL(/\/change-password$/);
      await expect(
        page.getByRole("dialog", { name: "Navigation", exact: true }),
      ).toHaveCount(0);
      await expect(page.getByLabel("Current password")).toBeVisible();
      const reopened = await openAccountMenu(page, "operator");
      await reopened
        .getByRole("menuitem", { name: "Log out", exact: true })
        .click();
      await expect(page).toHaveURL(/\/login$/);
      expect(logoutCount).toBe(1);
    });
  }

test("navbar highlights only the current page across navigation and reloads", async ({
  page,
}) => {
  await page.route("**/api/v2/**", (route) => {
    expect(route.request().method()).toBe("GET");
    const path = new URL(route.request().url()).pathname;
    return fulfillJson(
      route,
      200,
      path === "/api/v2/session"
        ? operator
        : path === "/api/v2/imports"
          ? { pending: null }
          : path === "/api/v2/provisioning-window"
            ? { state: "closed" }
            : [],
    );
  });
  await page.goto("/preparation");
  const navigation = await openNavigation(page);
  await expect(
    navigation.getByRole("link", { name: "Bindings", exact: true }),
  ).toHaveCount(0);
  for (const name of [
    "Preparation",
    "Seats",
    "Accounts",
    "Devices",
    "Enrollment",
    "Targets",
    "Print queue",
  ]) {
    const link = navigation.getByRole("link", { name, exact: true });
    await link.click();
    await expect(link).toHaveAttribute("aria-current", "page");
    await expect(navigation.locator('a[aria-current="page"]')).toHaveCount(1);
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
    await expect(
      page.getByRole("navigation", { name: "Breadcrumb", exact: true }),
    ).toContainText(name);
  }
  await page.goBack();
  await expect(
    navigation.getByRole("link", { name: "Targets", exact: true }),
  ).toHaveAttribute("aria-current", "page");
  await page.reload();
  await expect(navigation.locator('a[aria-current="page"]')).toHaveText(
    "Targets",
  );
  await expect(navigation.locator('a[aria-current="page"]')).toHaveCount(1);
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
  await openNavigation(page);
  const trigger = page.getByRole("button", {
    name: `Account menu for ${username}`,
    exact: true,
  });
  await expect(trigger).toHaveAccessibleName(`Account menu for ${username}`);
  const menu = await openAccountMenu(page, username);
  await expect(menu.getByText(username, { exact: true })).toBeVisible();
  for (const element of [trigger, menu]) {
    const bounds = (await element.boundingBox())!;
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(390);
  }
  await page.keyboard.press("Escape");
  await closeNavigation(page);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(
    390,
  );
});

for (const width of [1440, 390]) {
  test(`pending logout cannot be submitted twice and failures remain visible at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    let heldLogout: Route | undefined;
    let logoutCount = 0;
    await page.route("**/api/v2/**", (route) => {
      if (route.request().method() === "DELETE") {
        logoutCount++;
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
    await (
      await openAccountMenu(page, "operator")
    )
      .getByRole("menuitem", { name: "Log out", exact: true })
      .click();
    await expect.poll(() => logoutCount).toBe(1);
    const menu = await openAccountMenu(page, "operator");
    await expect(
      menu.getByRole("menuitem", { name: "Log out", exact: true }),
    ).toHaveAttribute("aria-disabled", "true");
    await fulfillJson(heldLogout!, 503, {
      title: "Unable to log out",
      status: 503,
      code: "SERVICE_UNAVAILABLE",
    });
    await expect(
      menu.getByRole("menuitem", { name: "Log out", exact: true }),
    ).not.toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    await closeNavigation(page);
    await expect(page.getByRole("alert")).toContainText("Unable to log out");
    expect(logoutCount).toBe(1);
  });
}

test("failed login surfaces the coded error", async ({ page }) => {
  await page.route("**/api/v2/**", (route) =>
    fulfillJson(
      route,
      401,
      route.request().method() === "POST"
        ? {
            title: "Authentication failed",
            status: 401,
            code: "AUTHENTICATION_FAILED",
          }
        : unauthorized,
    ),
  );
  await page.goto("/login");
  await page.getByLabel("Login name").fill("operator");
  await page.getByLabel("Password").fill("wrong password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Login failed: Authentication failed",
  );
});

test("seats page renders rows", async ({ page }) => {
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    return fulfillJson(
      route,
      200,
      path === "/api/v2/session"
        ? operator
        : path === "/api/v2/seats"
          ? [
              { seat_id: "seat-001", seat_code: "A-01" },
              { seat_id: "seat-002", seat_code: "A-02" },
            ]
          : [],
    );
  });
  await page.goto("/seats");
  await expect(page.getByText("A-01", { exact: true })).toBeVisible();
  await expect(page.getByText("A-02", { exact: true })).toBeVisible();
  await page.getByRole("radio", { name: "List view", exact: true }).click();
  await expect(
    page.getByRole("table", { name: "Seats", exact: true }).locator("tbody tr"),
  ).toHaveCount(2);
});

test("expired session redirects to login during polling", async ({ page }) => {
  let expired = false;
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (expired && route.request().method() === "GET")
      return fulfillJson(route, 401, unauthorized);
    return fulfillJson(
      route,
      200,
      path === "/api/v2/session"
        ? operator
        : path === "/api/v2/seats"
          ? [{ seat_id: "seat-001", seat_code: "A-01" }]
          : [],
    );
  });
  await page.goto("/seats");
  await expect(page.getByText("A-01", { exact: true })).toBeVisible();
  expired = true;
  await expect(page).toHaveURL(/\/login$/, { timeout: 15_000 });
});
