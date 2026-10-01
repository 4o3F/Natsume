import { expect, test, type Page, type Route } from "@playwright/test";
import type { components } from "../src/api/generated/schema";

type Identity = components["schemas"]["SessionResponse"];
type Account = components["schemas"]["OperatorAccountResponse"];
type Invitation = components["schemas"]["OperatorInvitationResponse"];
const admin: Identity = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const otherAdmin: Account = {
  operator_id: "01912345-6789-7abc-8def-0123456789ac",
  role: "admin",
  username: "second-admin",
};
const viewer: Account = {
  operator_id: "01912345-6789-7abc-8def-0123456789ad",
  role: "viewer",
  username: "reader",
};
const password = "Abcdefghijklmno1@";
const inviteToken = `invite_${"a".repeat(64)}`;
const resetToken = `reset_${"b".repeat(64)}`;
const unavailable =
  "This link is no longer available. Contact an administrator.";

function json(route: Route, status: number, body?: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}
function failure(route: Route, status: number, code: string) {
  return json(route, status, { title: "Rejected", status, code });
}

async function mockUsers(page: Page, identity: Identity | null = admin) {
  const state = {
    identity,
    accounts: [
      { ...admin, role: "admin", username: "first-admin" },
      otherAdmin,
      viewer,
    ] as Account[],
    invitations: [
      {
        invitation_id: "01912345-6789-7abc-8def-0123456789ae",
        issuer_operator_id: admin.operator_id,
        role: "viewer",
        created_at_unix_ms: 1_700_000_000_000,
        expires_at_unix_ms: 1_700_000_000_001,
        expired: true,
      },
    ] as Invitation[],
    calls: [] as {
      path: string;
      method: string;
      body: Record<string, string> | null;
    }[],
    registrationConflict: false,
    linkUnavailable: false,
    inspectionFailure: false,
    wrongCurrent: false,
    lastAdminConflict: false,
    holdPath: "",
    held: [] as Route[],
    counter: 0,
  };
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    const body = request.postData() ? request.postDataJSON() : null;
    state.calls.push({ path, method, body });
    expect(new URL(request.url()).hash).toBe("");
    expect(new URL(request.url()).search).toBe("");
    if (path === state.holdPath) {
      state.held.push(route);
      return;
    }
    if (path === "/api/v2/session") {
      if (method === "DELETE") {
        state.identity = null;
        return json(route, 204);
      }
      if (method === "POST") {
        state.identity = admin;
        return json(route, 200, admin);
      }
      return state.identity
        ? json(route, 200, state.identity)
        : failure(route, 401, "AUTHENTICATION_FAILED");
    }
    if (
      path === "/api/v2/operator/register/inspect" ||
      path === "/api/v2/operator/password/reset/inspect"
    ) {
      expect(body).toEqual({
        token: path.includes("register") ? inviteToken : resetToken,
      });
      if (state.identity)
        return failure(route, 409, "OPERATOR_LOGOUT_REQUIRED");
      if (state.inspectionFailure)
        return failure(route, 503, "SERVICE_UNAVAILABLE");
      if (state.linkUnavailable)
        return failure(route, 410, "OPERATOR_LINK_UNAVAILABLE");
      return json(
        route,
        200,
        path.includes("register")
          ? { role: "viewer" }
          : { username: viewer.username },
      );
    }
    if (
      path === "/api/v2/operator/register" ||
      path === "/api/v2/operator/password/reset"
    ) {
      if (state.identity)
        return failure(route, 409, "OPERATOR_LOGOUT_REQUIRED");
      if (state.linkUnavailable)
        return failure(route, 410, "OPERATOR_LINK_UNAVAILABLE");
      if (path.includes("register")) {
        if (state.registrationConflict)
          return failure(route, 409, "OPERATOR_LOGIN_NAME_CONFLICT");
        expect(body).toEqual({
          username: body.username,
          password,
          password_confirmation: password,
          token: inviteToken,
        });
        return json(route, 201, { ...viewer, username: body.username });
      }
      expect(body).toEqual({
        password,
        password_confirmation: password,
        token: resetToken,
      });
      return json(route, 204);
    }
    if (!state.identity) return failure(route, 401, "AUTHENTICATION_FAILED");
    if (path === "/api/v2/operator/password/change") {
      expect(body).toEqual({
        current_password: "old password",
        password,
        password_confirmation: password,
      });
      if (state.wrongCurrent)
        return failure(route, 400, "OPERATOR_CURRENT_PASSWORD_INVALID");
      state.identity = null;
      return json(route, 204);
    }
    if (path.startsWith("/api/v2/operator/") && state.identity.role !== "admin")
      return failure(route, 403, "AUTHORIZATION_DENIED");
    if (path === "/api/v2/operator/accounts")
      return json(route, 200, state.accounts);
    if (path.endsWith("/password-resets")) {
      const target = state.accounts.find((account) =>
        path.includes(account.operator_id),
      )!;
      return json(route, 201, {
        operator_id: target.operator_id,
        reset_id: "01912345-6789-7abc-8def-0123456789af",
        username: target.username,
        token: resetToken,
        expires_at_unix_ms: Date.now() + 3_600_000,
      });
    }
    if (path.startsWith("/api/v2/operator/accounts/")) {
      if (state.lastAdminConflict)
        return failure(route, 409, "OPERATOR_LAST_ADMIN");
      const target = state.accounts.find((account) =>
        path.endsWith(account.operator_id),
      )!;
      if (method === "PATCH") {
        expect(body).toEqual({ role: body.role });
        target.role = body.role;
        if (target.operator_id === state.identity.operator_id)
          state.identity = { ...state.identity, role: target.role };
      } else {
        expect(method).toBe("DELETE");
        state.accounts = state.accounts.filter((account) => account !== target);
        if (target.operator_id === state.identity.operator_id)
          state.identity = null;
      }
      return json(route, 204);
    }
    if (path === "/api/v2/operator/invitations" && method === "GET")
      return json(route, 200, state.invitations);
    if (path.startsWith("/api/v2/operator/invitations")) {
      const previous = state.invitations.find((invitation) =>
        path.includes(invitation.invitation_id),
      );
      if (method === "DELETE") {
        state.invitations = state.invitations.filter(
          (invitation) => invitation !== previous,
        );
        return json(route, 204);
      }
      expect(method).toBe("POST");
      state.counter++;
      const invitation: Invitation = {
        invitation_id: `01912345-6789-7abc-8def-${String(state.counter).padStart(12, "0")}`,
        role: previous?.role ?? body.role,
        issuer_operator_id: state.identity.operator_id,
        created_at_unix_ms: Date.now(),
        expires_at_unix_ms: Date.now() + 7 * 86_400_000,
        expired: false,
      };
      if (previous)
        state.invitations = state.invitations.filter(
          (item) => item !== previous,
        );
      state.invitations.push(invitation);
      return json(route, 201, {
        invitation,
        token: `invite_${String(state.counter).repeat(64)}`,
      });
    }
    return json(route, 200, []);
  });
  return state;
}

async function fillPassword(page: Page) {
  await page.getByLabel("New password", { exact: true }).fill(password);
  await page.getByLabel("Confirm new password", { exact: true }).fill(password);
}
function userRow(page: Page, username: string) {
  return page
    .getByRole("row")
    .filter({ has: page.getByText(username, { exact: true }) });
}

test("admin creates, copies, regenerates and revokes invitations; plaintext disappears on reload", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const state = await mockUsers(page);
  await page.goto("/users");
  await expect(
    page.getByRole("link", { name: "Users", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Expired", { exact: true })).toBeVisible();
  await page.getByLabel("Invitation role").selectOption("admin");
  await page.getByRole("button", { name: "Create invitation" }).click();
  const link = page.getByLabel("Link", { exact: true });
  await expect(link).toHaveValue(
    new RegExp(`/register#token=invite_${"1".repeat(64)}$`),
  );
  const firstURL = await link.inputValue();
  expect(new URL(firstURL).origin).toBe(new URL(page.url()).origin);
  await page.getByRole("button", { name: "Copy link" }).click();
  await expect(
    page.getByRole("button", { name: "Copied", exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    firstURL,
  );
  const persisted = await page.evaluate(() =>
    JSON.stringify({
      local: { ...localStorage },
      session: { ...sessionStorage },
      history: history.state,
    }),
  );
  expect(persisted).not.toContain("invite_");
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Users", exact: true }),
  ).toBeVisible();
  await expect(link).toHaveCount(0);
  await page
    .getByRole("row")
    .filter({ hasText: "Expired" })
    .getByRole("button", { name: "Regenerate" })
    .click();
  await expect(page.getByRole("region", { name: "New link" })).toContainText(
    "VIEWER",
  );
  await expect(link).toHaveValue(new RegExp(`invite_${"2".repeat(64)}$`));
  expect(state.invitations.some((item) => item.expired)).toBe(false);
  await page
    .getByRole("row")
    .filter({ hasText: "VIEWER" })
    .getByRole("button", { name: "Revoke" })
    .click();
  await expect.poll(() => state.invitations.length).toBe(1);
  await expect(link).toHaveCount(0);
});

test("reset links identify the target and issuing/replacing leaves the current session intact", async ({
  page,
}) => {
  const state = await mockUsers(page);
  await page.goto("/users");
  await userRow(page, "reader")
    .getByRole("button", { name: "Reset password" })
    .click();
  await expect(page.getByRole("region", { name: "New link" })).toContainText(
    "Password reset link for reader",
  );
  await expect(page.getByLabel("Link", { exact: true })).toHaveValue(
    new RegExp(`/reset-password#token=${resetToken}$`),
  );
  expect(state.identity).toEqual(admin);
  expect(
    state.calls.filter((call) => call.path.endsWith("/password-resets")),
  ).toHaveLength(1);
  await userRow(page, "reader")
    .getByRole("button", { name: "Reset password" })
    .click();
  await expect
    .poll(
      () =>
        state.calls.filter((call) => call.path.endsWith("/password-resets"))
          .length,
    )
    .toBe(2);
  await page.getByRole("button", { name: "Dismiss" }).click();
  await expect(page.getByLabel("Link", { exact: true })).toHaveCount(0);
});

test("viewer has no Users entry or management requests but can change a password", async ({
  page,
}) => {
  const state = await mockUsers(page, viewer);
  await page.goto("/users");
  await expect(page).toHaveURL(/\/seats$/);
  await expect(
    page.getByRole("link", { name: "Users", exact: true }),
  ).toHaveCount(0);
  expect(
    state.calls.some((call) => call.path.startsWith("/api/v2/operator/")),
  ).toBe(false);
  await page
    .getByRole("link", { name: "Change password", exact: true })
    .click();
  await expect(page.getByLabel("Current password")).toBeVisible();
});

test("last administrator is visibly protected and a Server conflict is explained", async ({
  page,
}) => {
  const state = await mockUsers(page);
  state.accounts = state.accounts.filter((account) => account !== otherAdmin);
  await page.goto("/users");
  await expect(
    userRow(page, "first-admin").getByText("Last admin"),
  ).toBeVisible();
  await expect(page.getByLabel("Role for first-admin")).toBeDisabled();
  await expect(
    userRow(page, "first-admin").getByRole("button", {
      name: "Delete",
      exact: true,
    }),
  ).toBeDisabled();
  state.lastAdminConflict = true;
  await userRow(page, "reader")
    .getByRole("button", { name: "Delete", exact: true })
    .click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete user", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "The last administrator cannot be deleted or changed to Viewer.",
  );
});

test("self demotion immediately switches to Viewer and releases management state", async ({
  page,
}) => {
  const state = await mockUsers(page);
  await page.goto("/users");
  await page.getByRole("button", { name: "Create invitation" }).click();
  await expect(page.getByLabel("Link", { exact: true })).toBeVisible();
  await page.getByLabel("Role for first-admin").selectOption("viewer");
  await expect(page.getByRole("alertdialog")).toContainText(
    "This is your own account.",
  );
  await page.getByRole("button", { name: "Confirm role change" }).click();
  await expect(page).toHaveURL(/\/seats$/);
  await expect(page.getByText("VIEWER", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Users", exact: true }),
  ).toHaveCount(0);
  expect(state.identity?.role).toBe("viewer");
  await page.goto("/users");
  await expect(page).toHaveURL(/\/seats$/);
  await expect(page.getByLabel("Link", { exact: true })).toHaveCount(0);
});

test("deletion needs confirmation; deleting yourself returns to sign in", async ({
  page,
}) => {
  const state = await mockUsers(page);
  await page.goto("/users");
  await userRow(page, "reader")
    .getByRole("button", { name: "Delete", exact: true })
    .click();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(state.accounts).toHaveLength(3);
  await userRow(page, "reader")
    .getByRole("button", { name: "Delete", exact: true })
    .click();
  await page.getByRole("button", { name: "Delete user", exact: true }).click();
  await expect(userRow(page, "reader")).toHaveCount(0);
  await userRow(page, "first-admin")
    .getByRole("button", { name: "Delete", exact: true })
    .click();
  await page.getByRole("button", { name: "Delete user", exact: true }).click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("status")).toContainText(
    "Your user account was deleted.",
  );
  expect(state.identity).toBeNull();
});

for (const flow of [
  {
    path: "register",
    token: inviteToken,
    button: "Create account",
    inspect: "register/inspect",
  },
  {
    path: "reset-password",
    token: resetToken,
    button: "Set new password",
    inspect: "password/reset/inspect",
  },
]) {
  test(`${flow.path} retains its link through explicit logout then removes the fragment`, async ({
    page,
  }) => {
    const state = await mockUsers(page);
    await page.goto(`/${flow.path}#token=${flow.token}`);
    await expect(
      page.getByRole("button", { name: "Sign out and continue" }),
    ).toBeVisible();
    expect(state.calls.some((call) => call.path.endsWith(flow.inspect))).toBe(
      false,
    );
    await page.getByRole("button", { name: "Sign out and continue" }).click();
    await expect(
      page.getByLabel("New password", { exact: true }),
    ).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/${flow.path}$`));
    if (flow.path === "register") {
      await expect(page.getByText("Your role:")).toContainText("VIEWER");
      await page.getByLabel("Username", { exact: true }).fill("chosen-name");
    } else {
      await expect(page.getByText("Reset password for")).toContainText(
        "reader",
      );
      await expect(page.getByLabel("Username", { exact: true })).toHaveCount(0);
    }
    await fillPassword(page);
    await page.getByRole("button", { name: flow.button }).click();
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole("status")).toContainText(
      flow.path === "register"
        ? "Account created for chosen-name"
        : "Password reset",
    );
    expect(state.identity).toBeNull();
    expect(
      state.calls.filter(
        (call) => call.path === "/api/v2/session" && call.method === "POST",
      ),
    ).toHaveLength(0);
    const persisted = await page.evaluate(() =>
      JSON.stringify({
        local: { ...localStorage },
        session: { ...sessionStorage },
        history: history.state,
      }),
    );
    expect(persisted).not.toContain(flow.token);
    expect(persisted).not.toContain(password);
  });

  test(`${flow.path} handles inspection failure, retries, and submission-time expiry`, async ({
    page,
  }) => {
    const state = await mockUsers(page, null);
    state.inspectionFailure = true;
    await page.goto(`/${flow.path}#token=${flow.token}`);
    await expect(page.getByRole("alert")).toContainText("server is busy");
    state.inspectionFailure = false;
    await page.getByRole("button", { name: "Check link again" }).click();
    await fillPassword(page);
    if (flow.path === "register")
      await page.getByLabel("Username", { exact: true }).fill("new-user");
    state.linkUnavailable = true;
    await page.getByRole("button", { name: flow.button }).click();
    await expect(page.getByRole("alert")).toContainText(unavailable);
    await expect(page).toHaveURL(new RegExp(`/${flow.path}$`));
    await page.reload();
    await expect(page.getByRole("alert")).toContainText(unavailable);
    await expect(page.getByLabel("New password", { exact: true })).toHaveCount(
      0,
    );
  });

  test(`${flow.path} rejects missing, wrong-purpose and expired links without showing a form`, async ({
    page,
  }) => {
    const state = await mockUsers(page, null);
    await page.goto(`/${flow.path}`);
    await expect(page.getByRole("alert")).toContainText(unavailable);
    expect(state.calls.some((call) => call.path.endsWith(flow.inspect))).toBe(
      false,
    );
    await page.goto(
      `/${flow.path}#token=${flow.path === "register" ? resetToken : inviteToken}`,
    );
    await expect(page.getByRole("alert")).toContainText(unavailable);
    state.linkUnavailable = true;
    await page.goto(`/${flow.path}#token=${flow.token}`);
    await expect(page.getByRole("alert")).toContainText(unavailable);
    await expect(page.getByLabel("New password", { exact: true })).toHaveCount(
      0,
    );
  });
}

test("registration validates inputs locally and preserves the form after a username conflict", async ({
  page,
}) => {
  const state = await mockUsers(page, null);
  state.registrationConflict = true;
  await page.goto(`/register#token=${inviteToken}`);
  await page.getByLabel("Username", { exact: true }).fill("taken");
  await page.getByLabel("New password", { exact: true }).fill("short 1@");
  await page
    .getByLabel("Confirm new password", { exact: true })
    .fill("different");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(
    page.getByText("Use at least 16 characters", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Passwords must match exactly", { exact: true }),
  ).toBeVisible();
  expect(
    state.calls.filter((call) => call.path === "/api/v2/operator/register"),
  ).toHaveLength(0);
  await fillPassword(page);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "username is already in use",
  );
  await expect(page.getByLabel("New password", { exact: true })).toHaveValue(
    password,
  );
  await expect(page.getByLabel("Username", { exact: true })).toHaveValue(
    "taken",
  );
  state.registrationConflict = false;
  await page.getByLabel("Username", { exact: true }).fill("available");
  await page.getByRole("button", { name: "Create account" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByLabel("Login name")).toHaveValue("available");
});

for (const identity of [admin, viewer]) {
  test(`${identity.role} can use a legacy current password; wrong-password errors retain the session`, async ({
    page,
  }) => {
    const state = await mockUsers(page, identity);
    state.wrongCurrent = true;
    await page.goto("/change-password");
    await page.getByLabel("Current password").fill("old password");
    await fillPassword(page);
    await page
      .getByRole("button", { name: "Change password", exact: true })
      .click();
    await expect(page.getByRole("alert")).toContainText(
      "current password is incorrect",
    );
    await expect(
      page.getByRole("button", { name: "Logout", exact: true }),
    ).toBeVisible();
    expect(state.identity).toEqual(identity);
    state.wrongCurrent = false;
    await page
      .getByRole("button", { name: "Change password", exact: true })
      .click();
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole("status")).toContainText("Password changed");
    expect(state.identity).toBeNull();
  });
}

test("an issued link delivered after logout cannot appear in another session", async ({
  page,
}) => {
  const state = await mockUsers(page);
  state.holdPath = "/api/v2/operator/invitations";
  // Hold only the issuing POST; let the pending list complete.
  await page.route("**/api/v2/operator/invitations", (route) =>
    route.request().method() === "GET"
      ? json(route, 200, [])
      : route.fallback(),
  );
  await page.goto("/users");
  await page.getByRole("button", { name: "Create invitation" }).click();
  await expect.poll(() => state.held.length).toBe(1);
  await page.getByRole("button", { name: "Logout", exact: true }).click();
  await expect(page).toHaveURL(/\/login$/);
  await page.getByLabel("Login name").fill("first-admin");
  await page.getByLabel("Password", { exact: true }).fill("legacy");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.getByRole("link", { name: "Users", exact: true }).click();
  await json(state.held[0], 201, {
    invitation: state.invitations[0],
    token: inviteToken,
  });
  await expect(
    page.getByRole("heading", { name: "Users", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("Link", { exact: true })).toHaveCount(0);
});

test("late anonymous submission cannot navigate after leaving the page", async ({
  page,
}) => {
  const state = await mockUsers(page, null);
  state.holdPath = "/api/v2/operator/register";
  await page.goto(`/register#token=${inviteToken}`);
  await page.getByLabel("Username", { exact: true }).fill("new-user");
  await fillPassword(page);
  await page.getByRole("button", { name: "Create account" }).click();
  await expect.poll(() => state.held.length).toBe(1);
  await expect(page.getByRole("button", { name: "Saving..." })).toBeDisabled();
  await page.getByRole("link", { name: "Back to sign in" }).click();
  await json(state.held[0], 201, { ...viewer, username: "new-user" });
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("status")).toHaveCount(0);
});

test("Users and registration stay usable on a narrow viewport with keyboard navigation", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const state = await mockUsers(page);
  await page.goto("/users");
  await expect(
    page.getByRole("heading", { name: "Users", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.getByLabel("Invitation role").focus();
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "Create invitation" }),
  ).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByLabel("Link", { exact: true })).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("users-mobile.png"),
    fullPage: true,
  });
  await userRow(page, "reader")
    .getByRole("button", { name: "Reset password" })
    .click();
  await expect(page.getByRole("region", { name: "New link" })).toContainText(
    "reader",
  );
  await userRow(page, "reader")
    .getByRole("button", { name: "Delete", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  state.identity = null;
  await page.goto(`/register#token=${inviteToken}`);
  await expect(page.getByLabel("Username", { exact: true })).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.getByLabel("Username", { exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(page.getByLabel("New password", { exact: true })).toBeFocused();
  await page.screenshot({
    path: testInfo.outputPath("register-mobile.png"),
    fullPage: true,
  });
  await page.goto(`/reset-password#token=${resetToken}`);
  await expect(page.getByText("Reset password for")).toContainText("reader");
  await page.screenshot({
    path: testInfo.outputPath("reset-mobile.png"),
    fullPage: true,
  });
  state.identity = viewer;
  await page.goto("/change-password");
  await expect(page.getByLabel("Current password")).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: testInfo.outputPath("change-mobile.png"),
    fullPage: true,
  });
  state.identity = admin;
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/users");
  await expect(userRow(page, "first-admin")).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("users-desktop.png"),
    fullPage: true,
  });
});
