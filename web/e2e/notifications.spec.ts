import { expect, test, type Page } from "@playwright/test";
import { openAccountMenu } from "./fixtures/ui";

interface ClipboardFixtureWindow extends Window {
  rejectPendingClipboard?: () => void;
}

async function mockNotifications(page: Page, deferredClipboard: boolean) {
  await page.addInitScript((deferred) => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: () => {
          if (!deferred) return Promise.reject(new Error("Clipboard denied"));
          return new Promise<void>((_, reject) => {
            (window as ClipboardFixtureWindow).rejectPendingClipboard = () =>
              reject(new Error("Clipboard denied"));
          });
        },
      },
    });
  }, deferredClipboard);
  let authenticated = true;
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/v2/session") {
      if (request.method() === "DELETE") {
        authenticated = false;
        return route.fulfill({ status: 204 });
      }
      if (request.method() === "POST") authenticated = true;
      return route.fulfill({
        status: authenticated ? 200 : 401,
        json: authenticated
          ? { operator_id: "operator", username: "operator", role: "admin" }
          : {
              title: "No valid session",
              status: 401,
              code: "AUTHENTICATION_FAILED",
            },
      });
    }
    if (path === "/api/v2/print/printers" && request.method() === "GET")
      return route.fulfill({
        json: [
          {
            printer_id: "printer",
            name: "Test printer",
            client_id: null,
            client_confirmed: false,
            pairing_expires_at_unix_ms: null,
          },
        ],
      });
    if (
      path === "/api/v2/print/printers/printer/pairing-code" &&
      request.method() === "POST"
    )
      return route.fulfill({
        json: {
          printer_id: "printer",
          code: "DEMO2345",
          expires_at_unix_ms: Date.now() + 600_000,
        },
      });
    expect(request.method()).toBe("GET");
    return route.fulfill({ json: [] });
  });
  await page.goto("/print/printers");
  await page
    .getByRole("button", { name: "Issue pairing code", exact: true })
    .click();
  await expect(page.getByText("DEMO2345", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Copy code", exact: true }).click();
  if (deferredClipboard)
    await expect
      .poll(() =>
        page.evaluate(() =>
          Boolean((window as ClipboardFixtureWindow).rejectPendingClipboard),
        ),
      )
      .toBe(true);
  else
    await expect(
      page.locator("[data-sonner-toast]").filter({ hasText: "Copy failed" }),
    ).toBeVisible();
}

async function rejectClipboard(page: Page) {
  await page.evaluate(async () => {
    const reject = (window as ClipboardFixtureWindow).rejectPendingClipboard;
    if (!reject) throw new Error("No pending clipboard operation");
    reject();
    // Allow the rejected callback and its React updates to finish before checking absence.
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => resolve()),
    );
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => resolve()),
    );
  });
}

async function signOutAndBackIn(page: Page) {
  const menu = await openAccountMenu(page, "operator");
  await menu.getByRole("menuitem", { name: "Log out", exact: true }).click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByText("DEMO2345", { exact: true })).toHaveCount(0);
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(0);
  await page.getByLabel("Login name", { exact: true }).fill("operator");
  await page.getByLabel("Password", { exact: true }).fill("demo-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/seats$/);
  await expect(
    page.getByRole("heading", { name: "Seats", exact: true }),
  ).toBeVisible();
}

test("a delayed clipboard failure cannot notify another page", async ({
  page,
}) => {
  await mockNotifications(page, true);
  await page.getByRole("link", { name: "Seats", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Seats", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("DEMO2345", { exact: true })).toHaveCount(0);
  await rejectClipboard(page);
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(0);
});

test("logout clears visible notifications and one-time pairing credentials", async ({
  page,
}) => {
  await mockNotifications(page, false);
  await signOutAndBackIn(page);
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(0);
  await expect(page.getByText("DEMO2345", { exact: true })).toHaveCount(0);
});

test("a delayed clipboard failure cannot notify the next authenticated session", async ({
  page,
}) => {
  await mockNotifications(page, true);
  await signOutAndBackIn(page);
  await rejectClipboard(page);
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(0);
  await expect(page.getByText("DEMO2345", { exact: true })).toHaveCount(0);
});
