import { expect, test, type Page } from "@playwright/test";

import type { PhotoSource } from "../src/api/generated/types";
import { routeApi } from "./fixtures/photo";

// One source per lifecycle state the page renders.
function initialSources(): PhotoSource[] {
  return [
    {
      source_id: "src-1",
      name: "Station North",
      state: "enabled",
      enrollment_confirmed: false,
      initialization_completed: false,
    },
    {
      source_id: "src-2",
      name: "Station South",
      state: "enabled",
      enrollment_confirmed: true,
      initialization_completed: false,
    },
    {
      source_id: "src-3",
      name: "Station East",
      state: "enabled",
      enrollment_confirmed: true,
      initialization_completed: true,
    },
    {
      source_id: "src-4",
      name: "Station Old",
      state: "disabled",
      enrollment_confirmed: true,
      initialization_completed: false,
    },
    {
      source_id: "src-5",
      name: "Station Gone",
      state: "revoked",
      enrollment_confirmed: true,
      initialization_completed: true,
    },
  ];
}

async function sourcesApi(page: Page, pairingStatus: number | null = null) {
  const api = {
    sources: initialSources(),
    created: [] as unknown[],
    stateChanges: [] as { id: string; body: unknown }[],
    pairingStatus,
  };
  const calls = await routeApi(page, "admin", async (route, path) => {
    const request = route.request();
    if (path === "/api/v2/photo/sources") {
      if (request.method() === "POST") {
        const body = request.postDataJSON();
        api.created.push(body);
        const source: PhotoSource = {
          source_id: "src-6",
          name: body.name,
          state: "enabled",
          enrollment_confirmed: false,
          initialization_completed: false,
        };
        api.sources = [...api.sources, source];
        await route.fulfill({ json: source });
      } else await route.fulfill({ json: api.sources });
      return true;
    }
    const match =
      /^\/api\/v2\/photo\/sources\/([^/]+)\/(pairing-code|reset-enrollment|state)$/.exec(
        path,
      );
    if (!match) return false;
    const [, id, action] = match;
    if (action === "state") {
      const body = request.postDataJSON();
      api.stateChanges.push({ id, body });
      api.sources = api.sources.map((source) =>
        source.source_id === id ? { ...source, state: body.state } : source,
      );
      await route.fulfill({ status: 204 });
      return true;
    }
    if (action === "pairing-code" && api.pairingStatus) {
      await route.fulfill({
        status: api.pairingStatus,
        json: { title: "Conflict", status: 409, code: "RESOURCE_CONFLICT" },
      });
      return true;
    }
    const code = action === "pairing-code" ? "ABCD-2345" : "WXYZ-6789";
    await route.fulfill({
      json: {
        source_id: id,
        code,
        expires_at_unix_ms: Date.now() + 600_000,
      },
    });
    return true;
  });
  return { calls, api };
}

function sourceRow(page: Page, region: "Active" | "Revoked", name: string) {
  return page
    .getByRole("region", { name: `${region} sources`, exact: true })
    .getByRole("listitem")
    .filter({ hasText: name });
}

test("each source lifecycle state shows its status and only its valid actions", async ({
  page,
}) => {
  await sourcesApi(page);
  await page.goto("/photo/sources");
  await expect(
    page.getByRole("heading", { name: "Photo sources", exact: true }),
  ).toBeVisible();

  const north = sourceRow(page, "Active", "Station North");
  await expect(north).toContainText("Awaiting pairing");
  for (const name of [
    "Issue pairing code",
    "Replace enrollment",
    "Disable",
    "Revoke",
  ])
    await expect(
      north.getByRole("button", { name, exact: true }),
    ).toBeVisible();

  const south = sourceRow(page, "Active", "Station South");
  await expect(south).toContainText("Awaiting initialization");
  await expect(
    south.getByRole("button", { name: "Issue pairing code", exact: true }),
  ).toHaveCount(0);
  await expect(
    south.getByRole("button", { name: "Replace enrollment", exact: true }),
  ).toBeVisible();

  const east = sourceRow(page, "Active", "Station East");
  await expect(east).toContainText("Initialized");
  for (const name of ["Issue pairing code", "Replace enrollment"])
    await expect(east.getByRole("button", { name, exact: true })).toHaveCount(
      0,
    );
  await expect(
    east.getByRole("button", { name: "Disable", exact: true }),
  ).toBeVisible();

  const old = sourceRow(page, "Active", "Station Old");
  await expect(old).toContainText("Disabled");
  await expect(
    old.getByRole("button", { name: "Enable", exact: true }),
  ).toBeVisible();
  await expect(
    old.getByRole("button", { name: "Disable", exact: true }),
  ).toHaveCount(0);

  const gone = sourceRow(page, "Revoked", "Station Gone");
  await expect(gone).toContainText("Revoked");
  await expect(
    gone.getByRole("button", {
      name: /^(Issue pairing code|Replace enrollment|Disable|Enable|Revoke)$/,
    }),
  ).toHaveCount(0);
});

test("creating a source posts its name and lists it awaiting pairing", async ({
  page,
}) => {
  const { api } = await sourcesApi(page);
  await page.goto("/photo/sources");
  await page
    .getByRole("button", { name: "Create source", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Source name", { exact: true }).fill("Station Five");
  await dialog
    .getByRole("button", { name: "Create source", exact: true })
    .click();
  await expect.poll(() => api.created).toEqual([{ name: "Station Five" }]);
  await expect(page.getByText("Source created", { exact: true })).toBeVisible();
  await expect(sourceRow(page, "Active", "Station Five")).toContainText(
    "Awaiting pairing",
  );
});

test("a pairing code is shown once and disappears on dismiss", async ({
  page,
}) => {
  const { calls } = await sourcesApi(page);
  await page.goto("/photo/sources");
  await sourceRow(page, "Active", "Station North")
    .getByRole("button", { name: "Issue pairing code", exact: true })
    .click();
  const secret = page.getByRole("region", {
    name: "Pairing code for Station North",
    exact: true,
  });
  await expect(secret.getByText("ABCD-2345", { exact: true })).toBeVisible();
  await expect(secret).toContainText("Shown once");
  expect(calls).toContain("POST /api/v2/photo/sources/src-1/pairing-code");

  await page.reload();
  await expect(page.getByText("ABCD-2345", { exact: true })).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Photo sources" }),
  ).toBeVisible();
  await sourceRow(page, "Active", "Station North")
    .getByRole("button", { name: "Issue pairing code", exact: true })
    .click();
  await expect(page.getByText("ABCD-2345", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(page.getByText("ABCD-2345", { exact: true })).toHaveCount(0);
});

test("a pairing code conflict shows the photo conflict message", async ({
  page,
}) => {
  await sourcesApi(page, 409);
  await page.goto("/photo/sources");
  await sourceRow(page, "Active", "Station North")
    .getByRole("button", { name: "Issue pairing code", exact: true })
    .click();
  await expect(
    page.getByRole("alert").filter({
      hasText:
        "Photo state changed or the roster, source, or import no longer allows this operation.",
    }),
  ).toBeVisible();
  await expect(page.getByRole("region", { name: /^Pairing code/ })).toHaveCount(
    0,
  );
});

test("replacing enrollment confirms first and then shows the new code", async ({
  page,
}) => {
  const { calls } = await sourcesApi(page);
  await page.goto("/photo/sources");
  await sourceRow(page, "Active", "Station South")
    .getByRole("button", { name: "Replace enrollment", exact: true })
    .click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("Replace enrollment for Station South?");
  await expect(dialog).toContainText("invalidates the current binding");
  await dialog
    .getByRole("button", { name: "Keep enrollment", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(calls.some((call) => call.endsWith("/reset-enrollment"))).toBe(false);

  await sourceRow(page, "Active", "Station South")
    .getByRole("button", { name: "Replace enrollment", exact: true })
    .click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Replace enrollment", exact: true })
    .click();
  await expect(
    page
      .getByRole("region", {
        name: "Pairing code for Station South",
        exact: true,
      })
      .getByText("WXYZ-6789", { exact: true }),
  ).toBeVisible();
  expect(calls).toContain("POST /api/v2/photo/sources/src-2/reset-enrollment");
});

test("disable and enable send the source state and update its status", async ({
  page,
}) => {
  const { api } = await sourcesApi(page);
  await page.goto("/photo/sources");
  await sourceRow(page, "Active", "Station North")
    .getByRole("button", { name: "Disable", exact: true })
    .click();
  await expect
    .poll(() => api.stateChanges)
    .toEqual([{ id: "src-1", body: { state: "disabled" } }]);
  const north = sourceRow(page, "Active", "Station North");
  await expect(north).toContainText("Disabled");
  await north.getByRole("button", { name: "Enable", exact: true }).click();
  await expect
    .poll(() => api.stateChanges)
    .toEqual([
      { id: "src-1", body: { state: "disabled" } },
      { id: "src-1", body: { state: "enabled" } },
    ]);
  await expect(sourceRow(page, "Active", "Station North")).toContainText(
    "Awaiting pairing",
  );
});

test("revoking requires confirmation and then moves the source to Revoked", async ({
  page,
}) => {
  const { calls, api } = await sourcesApi(page);
  await page.goto("/photo/sources");
  await sourceRow(page, "Active", "Station Old")
    .getByRole("button", { name: "Revoke", exact: true })
    .click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("Revoke Station Old?");
  await expect(dialog).toContainText("permanently stops this source");
  await dialog
    .getByRole("button", { name: "Keep source", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(api.stateChanges).toHaveLength(0);

  await sourceRow(page, "Active", "Station Old")
    .getByRole("button", { name: "Revoke", exact: true })
    .click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Revoke source", exact: true })
    .click();
  await expect
    .poll(() => api.stateChanges)
    .toEqual([{ id: "src-4", body: { state: "revoked" } }]);
  expect(calls).toContain("POST /api/v2/photo/sources/src-4/state");
  const revoked = sourceRow(page, "Revoked", "Station Old");
  await expect(revoked).toContainText("Revoked");
  await expect(
    revoked.getByRole("button", {
      name: /^(Issue pairing code|Replace enrollment|Disable|Enable|Revoke)$/,
    }),
  ).toHaveCount(0);
});

test("photo sources fit a 390px viewport", async ({ page }) => {
  await sourcesApi(page);
  await page.setViewportSize({ width: 390, height: 900 });
  await page.goto("/photo/sources");
  await expect(
    page.getByRole("region", { name: "Active sources", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
});
