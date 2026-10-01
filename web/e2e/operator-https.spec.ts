import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { expect, test, type Page, type BrowserContext } from "@playwright/test";

import type { components } from "../src/api/generated/schema";

type Account = components["schemas"]["OperatorAccountResponse"];
type Invitation = components["schemas"]["OperatorInvitationIssuedResponse"];
type Reset = components["schemas"]["OperatorPasswordResetIssuedResponse"];
const origin = process.env.NATSUME_OPERATOR_HTTPS_ORIGIN!;
const root = process.env.NATSUME_OPERATOR_HTTPS_ROOT!;
const evidence = process.env.NATSUME_OPERATOR_HTTPS_EVIDENCE!;
const rootPassword = process.env.NATSUME_OPERATOR_TEST_PASSWORD!;
const cookieName = "__Secure-natsume_session";
const unavailable =
  "This link is no longer available. Contact an administrator.";

// All requests use Chromium's real HTTPS connection and cookie jar. No routing
// mocks or APIRequestContext TLS bypass are used by this acceptance suite.
async function api<T = unknown>(
  page: Page,
  path: string,
  method = "GET",
  body?: unknown,
) {
  const result = await page.evaluate(
    async ({ path, method, body }) => {
      const response = await fetch(`/api/v2${path}`, {
        method,
        credentials: "same-origin",
        headers:
          body === undefined
            ? undefined
            : { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const text = await response.text();
      return {
        status: response.status,
        cache: response.headers.get("cache-control"),
        data: text ? JSON.parse(text) : null,
      };
    },
    { path, method, body },
  );
  return { ...result, data: result.data as T };
}

function userRow(page: Page, username: string) {
  return page
    .getByRole("region", { name: "User accounts", exact: true })
    .getByRole("row")
    .filter({ has: page.getByText(username, { exact: true }) });
}

async function signIn(page: Page, username: string, password: string) {
  await page.goto("/login");
  await page.getByLabel("Login name").fill(username);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Logout", exact: true }),
  ).toBeVisible();
}

async function setPasswords(page: Page, password: string) {
  await page.getByLabel("New password", { exact: true }).fill(password);
  await page.getByLabel("Confirm new password", { exact: true }).fill(password);
}

async function account(page: Page, username: string) {
  const result = await api<Account[]>(page, "/operator/accounts");
  expect(result.status).toBe(200);
  expect(result.cache).toBe("no-store");
  const found = result.data.find((value) => value.username === username);
  expect(Boolean(found)).toBe(true);
  expect(Object.keys(found!).sort()).toEqual([
    "operator_id",
    "role",
    "username",
  ]);
  return found!;
}

async function issueUI(
  page: Page,
  role: "admin" | "viewer",
  username?: string,
) {
  await page.goto("/users");
  const link = page.getByLabel("Link", { exact: true });
  if (username)
    await userRow(page, username)
      .getByRole("button", { name: "Reset password", exact: true })
      .click();
  else {
    await page.getByLabel("Invitation role").selectOption(role);
    await page
      .getByRole("button", { name: "Create invitation", exact: true })
      .click();
  }
  await expect(link).toBeVisible();
  const value = await link.inputValue();
  expect(new URL(value).origin).toBe(origin);
  expect(new URL(value).search).toBe("");
  expect(new URL(value).pathname).toBe(
    username ? "/reset-password" : "/register",
  );
  return value;
}

function token(url: string) {
  return new URLSearchParams(new URL(url).hash.slice(1)).get("token")!;
}

async function register(
  page: Page,
  link: string,
  username: string,
  password: string,
) {
  await page.goto(link);
  await expect(page.getByLabel("Username", { exact: true })).toBeVisible();
  expect(new URL(page.url()).hash).toBe("");
  await page.getByLabel("Username", { exact: true }).fill(username);
  await setPasswords(page, password);
  await page
    .getByRole("button", { name: "Create account", exact: true })
    .click();
  await expect(page).toHaveURL(`${origin}/login`);
  await expect(page.getByRole("status")).toContainText(
    `Account created for ${username}`,
  );
  expect(
    (await page.context().cookies(`${origin}/api/v2/session`)).some(
      (cookie) => cookie.name === cookieName,
    ),
  ).toBe(false);
}

async function screenshots(page: Page, name: string) {
  const directory = join(evidence, "screenshots");
  mkdirSync(directory, { recursive: true });
  for (const [size, width, height] of [
    ["desktop", 1440, 1000],
    ["mobile", 390, 844],
  ] as const) {
    await page.setViewportSize({ width, height });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    for (const alert of await page.locator('[data-slot="alert-title"]').all()) {
      expect(
        await alert.evaluate(
          (element) => getComputedStyle(element).webkitLineClamp,
        ),
      ).toBe("none");
    }
    for (const username of await page
      .locator('section[aria-label="User accounts"] td:first-child > div')
      .all()) {
      expect(
        await username.evaluate(
          (element) =>
            element.getBoundingClientRect().right <=
            element.parentElement!.getBoundingClientRect().right,
        ),
      ).toBe(true);
    }
    await page.screenshot({
      path: join(directory, `${name}-${size}.png`),
      fullPage: true,
      mask: [
        page.getByLabel("Link", { exact: true }),
        page.locator('input[type="password"]'),
      ],
    });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
}

test("production HTTPS supports the complete Operator lifecycle and rejects revoked authorization", async ({
  browser,
  page,
  context,
}) => {
  const contexts: BrowserContext[] = [];
  const secrets = new Set([rootPassword, "incorrect-current-password"]);
  const consoleOutput: string[] = [];
  const requests: {
    method: string;
    path: string;
    status: number;
    cache: string | undefined;
  }[] = [];
  let secretURLs = 0;
  function observe(current: Page) {
    current.on("console", (message) => consoleOutput.push(message.text()));
    current.on("response", (response) => {
      const url = new URL(response.url());
      if (!url.pathname.startsWith("/api/v2/")) return;
      if (
        url.search.includes("invite_") ||
        url.search.includes("reset_") ||
        url.pathname.includes("invite_") ||
        url.pathname.includes("reset_")
      )
        secretURLs++;
      requests.push({
        method: response.request().method(),
        path: url.pathname,
        status: response.status(),
        cache: response.headers()["cache-control"],
      });
    });
  }
  observe(page);
  async function openPage() {
    const current = await browser.newContext({ baseURL: origin });
    contexts.push(current);
    const next = await current.newPage();
    observe(next);
    await next.goto("/login");
    return next;
  }
  const adminPassword = `${rootPassword}@2`;
  const viewerPassword = `${rootPassword}@3`;
  const changedPassword = `${rootPassword}@4`;
  const resetPassword = "1".repeat(15) + "@";
  for (const value of [
    adminPassword,
    viewerPassword,
    changedPassword,
    resetPassword,
  ])
    secrets.add(value);
  try {
    const navigation = await page.goto("/login");
    const security = await navigation!.securityDetails();
    expect(security?.protocol).toBe("TLS 1.3");
    expect(security?.subjectName).toBe("Natsume Operator Acceptance Server");
    await signIn(page, "acceptance-admin", rootPassword);
    const cookie = (await context.cookies(`${origin}/api/v2/session`)).find(
      (value) => value.name === cookieName,
    )!;
    expect(Boolean(cookie)).toBe(true);
    expect(cookie.secure).toBe(true);
    expect(cookie.httpOnly).toBe(true);
    expect(cookie.sameSite).toBe("Strict");
    expect(cookie.path).toBe("/api/v2");
    expect(/^[0-9a-f]{64}$/.test(cookie.value)).toBe(true);
    secrets.add(cookie.value);
    const initial = await account(page, "acceptance-admin");

    const adminPage = await openPage();
    const viewerPage = await openPage();
    const otherViewer = await openPage();
    const anonymous = await openPage();

    await test.step("last admin, fixed invitation role, conflict retry and secure cookies", async () => {
      await page.goto("/users");
      await expect(page.getByLabel("Role for acceptance-admin")).toBeDisabled();
      await expect(
        userRow(page, "acceptance-admin").getByRole("button", {
          name: "Delete",
          exact: true,
        }),
      ).toBeDisabled();
      const link = await issueUI(page, "admin");
      secrets.add(token(link));
      await context.grantPermissions(["clipboard-read", "clipboard-write"], {
        origin,
      });
      await page
        .getByRole("button", { name: "Copy link", exact: true })
        .click();
      await expect(
        page.getByRole("button", { name: "Copied", exact: true }),
      ).toBeVisible();
      expect(
        (await page.evaluate(() => navigator.clipboard.readText())) === link,
      ).toBe(true);
      await screenshots(page, "users-invitation");
      const beforeRegistration = await api<Account[]>(
        page,
        "/operator/accounts",
      );
      expect(beforeRegistration.data.length).toBe(1);
      await adminPage.goto(link);
      await expect(adminPage.getByText("Your role:")).toContainText("ADMIN");
      expect(new URL(adminPage.url()).hash).toBe("");
      await screenshots(adminPage, "register");
      await adminPage
        .getByLabel("Username", { exact: true })
        .fill("acceptance-admin");
      await setPasswords(adminPage, adminPassword);
      await adminPage
        .getByRole("button", { name: "Create account", exact: true })
        .click();
      await expect(adminPage.getByRole("alert")).toContainText(
        "username is already in use",
      );
      expect(
        (await adminPage
          .getByLabel("New password", { exact: true })
          .inputValue()) === adminPassword,
      ).toBe(true);
      await screenshots(adminPage, "register-conflict");
      await adminPage
        .getByLabel("Username", { exact: true })
        .fill("second-admin");
      await adminPage
        .getByRole("button", { name: "Create account", exact: true })
        .click();
      await expect(adminPage).toHaveURL(`${origin}/login`);
      expect((await api(adminPage, "/session")).status).toBe(401);
      await signIn(adminPage, "second-admin", adminPassword);
      const reused = await api(
        anonymous,
        "/operator/register/inspect",
        "POST",
        { token: token(link) },
      );
      expect(reused.status).toBe(410);
      expect(reused.cache).toBe("no-store");
      const viewerLink = await issueUI(page, "viewer");
      secrets.add(token(viewerLink));
      await register(viewerPage, viewerLink, "reader", viewerPassword);
      await signIn(viewerPage, "reader", viewerPassword);
      await signIn(otherViewer, "reader", viewerPassword);
      await expect(
        viewerPage.getByRole("link", { name: "Users", exact: true }),
      ).toHaveCount(0);
      const denied = await api(viewerPage, "/operator/accounts");
      expect(denied.status).toBe(403);
      expect((denied.data as { code: string }).code).toBe(
        "AUTHORIZATION_DENIED",
      );
      const direct = await api(
        viewerPage,
        `/operator/accounts/${initial.operator_id}`,
        "DELETE",
      );
      expect(direct.status).toBe(403);
    });

    const reader = await account(page, "reader");
    await test.step("self password change retains wrong-password sessions and revokes every session and grant on success", async () => {
      const pendingReset = await issueUI(page, "viewer", "reader");
      secrets.add(token(pendingReset));
      expect((await api(otherViewer, "/session")).status).toBe(200);
      await viewerPage.goto("/change-password");
      await viewerPage
        .getByLabel("Current password")
        .fill("incorrect-current-password");
      await setPasswords(viewerPage, changedPassword);
      await viewerPage
        .getByRole("button", { name: "Change password", exact: true })
        .click();
      await expect(viewerPage.getByRole("alert")).toContainText(
        "current password is incorrect",
      );
      expect((await api(viewerPage, "/session")).status).toBe(200);
      await screenshots(viewerPage, "change-password-error");
      await viewerPage.getByLabel("Current password").fill(viewerPassword);
      await viewerPage
        .getByRole("button", { name: "Change password", exact: true })
        .click();
      await expect(viewerPage).toHaveURL(`${origin}/login`);
      await expect(viewerPage.getByRole("status")).toContainText(
        "Password changed",
      );
      expect((await api(otherViewer, "/session")).status).toBe(401);
      const removed = await api(
        anonymous,
        "/operator/password/reset/inspect",
        "POST",
        { token: token(pendingReset) },
      );
      expect(removed.status).toBe(410);
      await signIn(viewerPage, "reader", changedPassword);
      await signIn(otherViewer, "reader", changedPassword);
    });

    await test.step("reset replacement, logged-in gate, username binding and no automatic login", async () => {
      const oldLink = await issueUI(page, "viewer", "reader");
      const link = await issueUI(page, "viewer", "reader");
      secrets.add(token(oldLink));
      secrets.add(token(link));
      expect(token(oldLink) === token(link)).toBe(false);
      expect(
        (
          await api(anonymous, "/operator/password/reset/inspect", "POST", {
            token: token(oldLink),
          })
        ).status,
      ).toBe(410);
      expect((await api(viewerPage, "/session")).status).toBe(200);
      await viewerPage.goto(link);
      await expect(
        viewerPage.getByRole("button", {
          name: "Sign out and continue",
          exact: true,
        }),
      ).toBeVisible();
      const gated = await api(
        viewerPage,
        "/operator/password/reset/inspect",
        "POST",
        { token: token(link) },
      );
      expect(gated.status).toBe(409);
      expect((gated.data as { code: string }).code).toBe(
        "OPERATOR_LOGOUT_REQUIRED",
      );
      await viewerPage
        .getByRole("button", { name: "Sign out and continue", exact: true })
        .click();
      await expect(viewerPage.getByText("Reset password for")).toContainText(
        "reader",
      );
      expect(new URL(viewerPage.url()).hash).toBe("");
      await expect(
        viewerPage.getByLabel("Username", { exact: true }),
      ).toHaveCount(0);
      await screenshots(viewerPage, "reset-password");
      await setPasswords(viewerPage, resetPassword);
      await viewerPage
        .getByRole("button", { name: "Set new password", exact: true })
        .click();
      await expect(viewerPage).toHaveURL(`${origin}/login`);
      expect((await api(viewerPage, "/session")).status).toBe(401);
      expect((await api(otherViewer, "/session")).status).toBe(401);
      expect(
        (
          await api(anonymous, "/operator/password/reset/inspect", "POST", {
            token: token(link),
          })
        ).status,
      ).toBe(410);
      await signIn(viewerPage, "reader", resetPassword);
      expect((await api(page, "/session")).status).toBe(200);
    });

    const second = await account(page, "second-admin");
    await test.step("issuer demotion revokes both grant kinds and promotion cannot resurrect them", async () => {
      const invitation = await api<Invitation>(
        adminPage,
        "/operator/invitations",
        "POST",
        { role: "viewer" },
      );
      const reset = await api<Reset>(
        adminPage,
        `/operator/accounts/${reader.operator_id}/password-resets`,
        "POST",
      );
      expect(invitation.status).toBe(201);
      expect(reset.status).toBe(201);
      secrets.add(invitation.data.token);
      secrets.add(reset.data.token);
      await page.goto("/users");
      await page.getByLabel("Role for second-admin").selectOption("viewer");
      await page
        .getByRole("button", { name: "Confirm role change", exact: true })
        .click();
      await expect(page.getByRole("alertdialog")).toHaveCount(0);
      expect((await api(adminPage, "/operator/accounts")).status).toBe(403);
      expect(
        (
          await api(anonymous, "/operator/register/inspect", "POST", {
            token: invitation.data.token,
          })
        ).status,
      ).toBe(410);
      expect(
        (
          await api(anonymous, "/operator/password/reset/inspect", "POST", {
            token: reset.data.token,
          })
        ).status,
      ).toBe(410);
      await page.getByLabel("Role for second-admin").selectOption("admin");
      await page
        .getByRole("button", { name: "Confirm role change", exact: true })
        .click();
      await expect(page.getByRole("alertdialog")).toHaveCount(0);
      expect((await api(adminPage, "/operator/accounts")).status).toBe(200);
      expect(
        (
          await api(anonymous, "/operator/register/inspect", "POST", {
            token: invitation.data.token,
          })
        ).status,
      ).toBe(410);
      expect(
        (
          await api(anonymous, "/operator/password/reset/inspect", "POST", {
            token: reset.data.token,
          })
        ).status,
      ).toBe(410);
    });

    await test.step("self demotion and deletion update browser identity and preserve the final administrator", async () => {
      await adminPage.goto("/users");
      await adminPage
        .getByLabel("Role for second-admin")
        .selectOption("viewer");
      await adminPage
        .getByRole("button", { name: "Confirm role change", exact: true })
        .click();
      await expect(adminPage).toHaveURL(`${origin}/seats`);
      await expect(
        adminPage.getByRole("link", { name: "Users", exact: true }),
      ).toHaveCount(0);
      expect((await api(adminPage, "/session")).status).toBe(200);
      expect((await api(adminPage, "/operator/accounts")).status).toBe(403);
      await page.goto("/users");
      await page.getByLabel("Role for second-admin").selectOption("admin");
      await page
        .getByRole("button", { name: "Confirm role change", exact: true })
        .click();
      await expect(page.getByRole("alertdialog")).toHaveCount(0);
      await adminPage.goto("/users");
      await userRow(adminPage, "second-admin")
        .getByRole("button", { name: "Delete", exact: true })
        .click();
      await adminPage
        .getByRole("button", { name: "Delete user", exact: true })
        .click();
      await expect(adminPage).toHaveURL(`${origin}/login`);
      expect((await api(adminPage, "/session")).status).toBe(401);
      expect(
        (await api(page, `/operator/accounts/${second.operator_id}`, "DELETE"))
          .status,
      ).toBe(404);
      const protectedDelete = await api(
        page,
        `/operator/accounts/${initial.operator_id}`,
        "DELETE",
      );
      expect(protectedDelete.status).toBe(409);
      expect((protectedDelete.data as { code: string }).code).toBe(
        "OPERATOR_LAST_ADMIN",
      );
    });

    await test.step("deleted sessions and UUID-bound grants cannot act on a same-name replacement", async () => {
      const link = await issueUI(page, "viewer", "reader");
      secrets.add(token(link));
      await userRow(page, "reader")
        .getByRole("button", { name: "Delete", exact: true })
        .click();
      await page
        .getByRole("button", { name: "Delete user", exact: true })
        .click();
      await expect(userRow(page, "reader")).toHaveCount(0);
      expect((await api(viewerPage, "/session")).status).toBe(401);
      expect(
        (
          await api(anonymous, "/operator/password/reset/inspect", "POST", {
            token: token(link),
          })
        ).status,
      ).toBe(410);
      const replacement = await issueUI(page, "viewer");
      secrets.add(token(replacement));
      await register(anonymous, replacement, "reader", viewerPassword);
      const replaced = await account(page, "reader");
      expect(replaced.operator_id === reader.operator_id).toBe(false);
      expect(
        (
          await api(anonymous, "/operator/password/reset/inspect", "POST", {
            token: token(link),
          })
        ).status,
      ).toBe(410);
    });

    await test.step("concurrent real HTTPS registration and reset each consume one authorization", async () => {
      const invite = await api<Invitation>(
        page,
        "/operator/invitations",
        "POST",
        { role: "viewer" },
      );
      secrets.add(invite.data.token);
      const attempts = await Promise.all(
        ["race-a", "race-b"].map((username) =>
          api(anonymous, "/operator/register", "POST", {
            token: invite.data.token,
            username,
            password: viewerPassword,
            password_confirmation: viewerPassword,
          }),
        ),
      );
      expect(attempts.map((value) => value.status).sort()).toEqual([201, 410]);
      const reset = await api<Reset>(
        page,
        `/operator/accounts/${(await account(page, "reader")).operator_id}/password-resets`,
        "POST",
      );
      secrets.add(reset.data.token);
      const resets = await Promise.all(
        [0, 1].map(() =>
          api(anonymous, "/operator/password/reset", "POST", {
            token: reset.data.token,
            password: changedPassword,
            password_confirmation: changedPassword,
          }),
        ),
      );
      expect(resets.map((value) => value.status).sort()).toEqual([204, 410]);
    });

    await test.step("expired invitations regenerate with the fixed role and revoked links contain no secrets", async () => {
      const link = await issueUI(page, "viewer");
      secrets.add(token(link));
      const grants = await api<
        components["schemas"]["OperatorInvitationResponse"][]
      >(page, "/operator/invitations");
      expect(grants.status).toBe(200);
      expect(
        grants.data.some((value) =>
          Object.keys(value).some(
            (key) => key.includes("token") || key.includes("hash"),
          ),
        ),
      ).toBe(false);
      const grant = grants.data.find((value) => value.role === "viewer")!;
      // Alter only this disposable fixture's expiry; no wall-clock wait or production API bypass.
      execFileSync("python3", [
        "-c",
        "import sqlite3,sys; db=sqlite3.connect(sys.argv[1]); db.execute('UPDATE operator_invitations SET expires_at_unix_ms = 1 WHERE invitation_id = ?', (sys.argv[2],)); db.commit()",
        join(root, "natsume.db"),
        grant.invitation_id,
      ]);
      await page.reload();
      await expect(page.getByText("Expired", { exact: true })).toBeVisible();
      await screenshots(page, "users-expired");
      await anonymous.goto(link);
      await expect(anonymous.getByRole("alert")).toContainText(unavailable);
      await page
        .getByRole("button", { name: "Regenerate", exact: true })
        .click();
      const replacement = await page
        .getByLabel("Link", { exact: true })
        .inputValue();
      secrets.add(token(replacement));
      expect(token(replacement) === token(link)).toBe(false);
      // Open a fresh registration page rather than changing a scrubbed page's fragment.
      await anonymous.goto("/login");
      await register(
        anonymous,
        replacement,
        "expiry-recovered",
        viewerPassword,
      );
      await anonymous.goto(link);
      await expect(anonymous.getByRole("alert")).toContainText(unavailable);
      const revokedLink = await issueUI(page, "viewer");
      secrets.add(token(revokedLink));
      const unused = await api<
        components["schemas"]["OperatorInvitationResponse"][]
      >(page, "/operator/invitations");
      expect(
        (
          await api(
            page,
            `/operator/invitations/${unused.data[0].invitation_id}`,
            "DELETE",
          )
        ).status,
      ).toBe(204);
      await anonymous.goto("/login");
      await anonymous.goto(revokedLink);
      await expect(anonymous.getByRole("alert")).toContainText(unavailable);
      await screenshots(anonymous, "register-unavailable");
      await page.reload();
      await expect(
        page.getByRole("heading", { name: "Users", exact: true }),
      ).toBeVisible();
      await expect(page.getByLabel("Link", { exact: true })).toHaveCount(0);
      await screenshots(page, "users");
      for (const current of [page, adminPage, viewerPage, anonymous]) {
        const storage = await current.evaluate(() =>
          JSON.stringify({
            local: { ...localStorage },
            session: { ...sessionStorage },
            history: history.state,
          }),
        );
        expect([...secrets].some((value) => storage.includes(value))).toBe(
          false,
        );
      }
      const logs = readFileSync(join(root, "server.log"), "utf8");
      expect(
        [...secrets].some(
          (value) =>
            logs.includes(value) ||
            consoleOutput.some((message) => message.includes(value)),
        ),
      ).toBe(false);
      expect(secretURLs).toBe(0);
      expect(
        requests
          .filter((value) => value.path.startsWith("/api/v2/operator/"))
          .every((value) => value.cache === "no-store"),
      ).toBe(true);
      writeFileSync(
        join(evidence, "http-evidence.json"),
        JSON.stringify(
          {
            tls: security?.protocol,
            secureHttpOnlyStrictCookie: true,
            serverAndConsoleSecretsAbsent: true,
            persistedBrowserSecretsAbsent: true,
            secretURLs,
            requests,
          },
          null,
          2,
        ) + "\n",
      );
    });
  } finally {
    for (const current of contexts) await current.close();
  }
});
