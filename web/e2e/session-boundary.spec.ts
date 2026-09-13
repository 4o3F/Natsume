import { readFileSync } from "node:fs";
import { expect, test, type Page, type Route } from "@playwright/test";
import type { components } from "../src/api/generated/schema";

const admin = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const viewer = {
  operator_id: "01912345-6789-7abc-8def-0123456789ac",
  role: "viewer",
};
const unauthorized = {
  title: "No valid session",
  status: 401,
  code: "AUTHENTICATION_FAILED",
};
const otherAdmin = {
  ...admin,
  operator_id: "01912345-6789-7abc-8def-0123456789ad",
};
const candidateId = "01934567-89ab-7cde-8f01-23456789abcd";
const workbook = {
  name: "roster.xlsx",
  mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  buffer: readFileSync(
    new URL("../../crates/roster/examples/roster.xlsx", import.meta.url),
  ),
};

async function mockPreparationSession(page: Page) {
  const state = {
    identity: admin as typeof admin | null,
    nextIdentity: otherAdmin,
    expirePath: "",
    pending: null as components["schemas"]["ImportPendingSummary"] | null,
    uploadCount: 0,
    holdUpload: false,
    heldUploads: [] as {
      route: Route;
      preview: components["schemas"]["ImportPreviewResponse"];
    }[],
    committedTokens: [] as string[],
  };
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/v2/session" && request.method() === "POST") {
      state.identity = state.nextIdentity;
      state.expirePath = "";
      return json(route, 200, state.identity);
    }
    if (path === "/api/v2/session" && request.method() === "DELETE") {
      state.identity = null;
      return route.fulfill({ status: 204 });
    }
    if (path === state.expirePath) state.identity = null;
    if (!state.identity) return json(route, 401, unauthorized);
    if (path === "/api/v2/session") return json(route, 200, state.identity);
    if (path === "/api/v2/imports" && request.method() === "GET") {
      return json(route, 200, { pending: state.pending });
    }
    if (path === "/api/v2/imports" && request.method() === "POST") {
      state.uploadCount += 1;
      state.pending = {
        candidate_id:
          state.uploadCount === 1
            ? candidateId
            : "01934567-89ab-7cde-8f01-23456789abce",
        expires_at_unix_ms: 4_090_579_800_000,
        diff: {
          seats_added: ["A-01"],
          seats_removed: [],
          mappings_changed: [],
          unchanged_count: 0,
          affected_account_count: 1,
          binding_impacts: [],
          accounts_added: ["team-new"],
          accounts_removed: [],
          passwords_changed: [],
          organizations: [],
          organization_changes: [],
          team_changes: [],
        },
      };
      const preview = {
        ...state.pending,
        preview_token: String(state.uploadCount).repeat(43),
      };
      if (state.holdUpload) {
        state.heldUploads.push({ route, preview });
        return;
      }
      return json(route, 201, preview);
    }
    if (path.endsWith("/actions/commit")) {
      state.committedTokens.push(request.headers()["x-natsume-preview-token"]);
      state.pending = null;
      return route.fulfill({ status: 204 });
    }
    if (path.startsWith("/api/v2/imports/") && request.method() === "DELETE") {
      state.pending = null;
      return route.fulfill({ status: 204 });
    }
    return json(route, 200, []);
  });
  return state;
}

async function createPreview(page: Page) {
  await page.getByLabel("XLSX file").setInputFiles(workbook);
  await page.getByRole("button", { name: "Create preview" }).click();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeEnabled();
}

function json(route: Route, status: number, body: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

async function signIn(page: Page) {
  await page.getByLabel("Login name").fill("operator");
  await page.getByLabel("Password").fill("password");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("button", { name: "Logout" })).toBeVisible();
}

test("a new login does not display the expired operator's cached rows", async ({
  page,
}) => {
  let identity: typeof admin | null = admin;
  const heldReads: Route[] = [];
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/v2/session" && request.method() === "POST") {
      identity = viewer;
      return json(route, 200, identity);
    }
    if (!identity) return json(route, 401, unauthorized);
    if (path === "/api/v2/session") return json(route, 200, identity);
    if (path === "/api/v2/seats") {
      if (identity === viewer) {
        heldReads.push(route);
        return;
      }
      return json(route, 200, [
        { seat_id: "seat-1", seat_code: "OLD-PRIVATE-ROW" },
      ]);
    }
    return json(route, 200, []);
  });
  await page.clock.install();
  await page.goto("/seats");
  await expect(
    page.getByText("OLD-PRIVATE-ROW", { exact: true }),
  ).toBeVisible();
  identity = null;
  await page.clock.runFor(30_000);
  await expect(page).toHaveURL(/\/login$/);
  await signIn(page);
  await expect.poll(() => heldReads.length).toBeGreaterThan(0);
  await expect(page.getByText("OLD-PRIVATE-ROW", { exact: true })).toHaveCount(
    0,
  );
  for (const route of heldReads) await json(route, 200, []);
});

for (const transition of [
  {
    name: "logout and same-account login",
    expirePath: "",
    nextIdentity: admin,
  },
  {
    name: "session 401 and Viewer login",
    expirePath: "/api/v2/session",
    nextIdentity: viewer,
  },
  {
    name: "resource 401 and another Admin login",
    expirePath: "/api/v2/imports",
    nextIdentity: otherAdmin,
  },
]) {
  test(`${transition.name} clears preview authorization and operation errors`, async ({
    page,
  }) => {
    const state = await mockPreparationSession(page);
    state.nextIdentity = transition.nextIdentity;
    await page.clock.install();
    await page.goto("/preparation");
    await createPreview(page);
    await page.route("**/actions/commit", (route) =>
      json(route, 409, {
        title: "Conflict",
        status: 409,
        code: "IMPORT_PREVIEW_STALE",
      }),
    );
    await page.getByRole("button", { name: "Commit import" }).click();
    await page.getByRole("button", { name: "Confirm commit" }).click();
    await expect(
      page.getByRole("alert").filter({ hasText: "Import preview is stale" }),
    ).toBeVisible();

    if (transition.expirePath) {
      state.expirePath = transition.expirePath;
      await page.clock.runFor(
        transition.expirePath === "/api/v2/session" ? 30_000 : 10_000,
      );
    } else {
      await page.getByRole("button", { name: "Logout" }).click();
    }
    await expect(page).toHaveURL(/\/login$/);
    await signIn(page);
    await page.getByRole("link", { name: "Preparation", exact: true }).click();
    await expect(
      page.getByText("Pending import", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Commit import" }),
    ).toBeDisabled();
    await expect(
      page.getByRole("alert").filter({ hasText: "Import preview is stale" }),
    ).toHaveCount(0);
    await expect(
      page.getByText("Preview created", { exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByText(
        "Preview authorization and the reviewed XLSX are unavailable after a reload or session change; discard and re-upload to commit.",
        { exact: true },
      ),
    ).toBeVisible();
  });
}

test("polling identity and role changes remount the active preparation page", async ({
  page,
}) => {
  const state = await mockPreparationSession(page);
  await page.clock.install();
  await page.goto("/preparation");
  await createPreview(page);
  await page.clock.runFor(30_000);
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeEnabled();
  state.identity = otherAdmin;
  await page.clock.runFor(30_000);
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeDisabled();
  await expect(page.getByText("Preview created", { exact: true })).toHaveCount(
    0,
  );
  await page.getByRole("button", { name: "Discard preview" }).click();
  await createPreview(page);
  state.identity = { ...otherAdmin, role: "viewer" };
  await page.clock.runFor(30_000);
  await expect(page.getByText("VIEWER", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeDisabled();
  await expect(page.getByText("Preview created", { exact: true })).toHaveCount(
    0,
  );
});

test("a delayed upload cannot replace the next operator's preview", async ({
  page,
}) => {
  const state = await mockPreparationSession(page);
  state.holdUpload = true;
  await page.goto("/preparation");
  await page.getByLabel("XLSX file").setInputFiles(workbook);
  await page.getByRole("button", { name: "Create preview" }).click();
  await expect.poll(() => state.heldUploads.length).toBe(1);
  await page.getByRole("button", { name: "Logout" }).click();
  await signIn(page);
  await page.getByRole("link", { name: "Preparation", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "Discard preview" }).click();
  state.holdUpload = false;
  await createPreview(page);
  const held = state.heldUploads[0];
  await json(held.route, 201, held.preview);
  await page.getByRole("button", { name: "Commit import" }).click();
  await page.getByRole("button", { name: "Confirm commit" }).click();
  await expect.poll(() => state.committedTokens).toEqual(["2".repeat(43)]);
});

test("a selected workbook is cleared when the operator session changes", async ({
  page,
}) => {
  const state = await mockPreparationSession(page);
  await page.goto("/preparation");
  await page.getByLabel("XLSX file").setInputFiles(workbook);
  await expect(
    page.getByRole("button", { name: "Create preview" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Logout" }).click();
  await signIn(page);
  await page.getByRole("link", { name: "Preparation", exact: true }).click();
  await expect(page.getByLabel("XLSX file")).toHaveValue("");
  await expect(
    page.getByRole("button", { name: "Create preview" }),
  ).toBeDisabled();
  expect(state.uploadCount).toBe(0);
  await createPreview(page);
  await page.getByRole("button", { name: "Commit import" }).click();
  await page.getByRole("button", { name: "Confirm commit" }).click();
  await expect.poll(() => state.committedTokens).toEqual(["1".repeat(43)]);
  expect(state.uploadCount).toBe(1);
});
