import { readFileSync } from "node:fs";

import { expect, test } from "@playwright/test";

import type { PendingPhoto_Serialize as PendingPhoto } from "../src/api/generated/types";
import { ids, routeApi, zipBytes } from "./fixtures/photo";
import { openNavigation } from "./fixtures/ui";

const attention: PendingPhoto[] = [
  {
    source_id: "src-2",
    submission_id: "sub-1",
    import_id: ids.importId,
    account_id: ids.alpha,
    account: "team1",
    assigned_account_id: null,
    image_sha256: "c".repeat(64),
    state: "pending_review",
    revision: 3,
  },
  {
    source_id: "src-2",
    submission_id: "sub-2",
    import_id: ids.importId,
    account_id: ids.retired,
    account: "team-old",
    assigned_account_id: null,
    image_sha256: "d".repeat(64),
    state: "needs_association",
    revision: 1,
  },
];

test("admin sees the Photos navigation group", async ({ page }) => {
  await routeApi(page, "admin", async () => false);
  await page.goto("/seats");
  const navigation = await openNavigation(page);
  await expect(navigation.getByText("Photos", { exact: true })).toBeVisible();
  for (const [name, href] of [
    ["Team photos", "/photo"],
    ["Photo imports", "/photo/imports"],
    ["Photo sources", "/photo/sources"],
  ] as const)
    await expect(
      navigation.getByRole("link", { name, exact: true }),
    ).toHaveAttribute("href", href);
});

test("viewer sees no Photos navigation and is redirected away from Photo pages", async ({
  page,
}) => {
  const calls = await routeApi(page, "viewer", async () => false);
  await page.goto("/seats");
  const navigation = await openNavigation(page);
  await expect(navigation.getByText("Photos", { exact: true })).toHaveCount(0);
  for (const name of ["Team photos", "Photo imports", "Photo sources"])
    await expect(
      navigation.getByRole("link", { name, exact: true }),
    ).toHaveCount(0);
  for (const path of ["/photo", "/photo/imports", "/photo/sources"]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/seats$/);
  }
  expect(calls.some((call) => call.includes("/api/v2/photo"))).toBe(false);
});

test("team photo stat tiles and current photos list reflect the roster and photos", async ({
  page,
}) => {
  const calls = await routeApi(page, "admin", async (route, path) => {
    if (path !== "/api/v2/photo/submissions") return false;
    await route.fulfill({ json: { entries: attention, next: null } });
    return true;
  });
  await page.goto("/photo");
  await expect(
    page.getByRole("heading", { name: "Team photos", exact: true }),
  ).toBeVisible();

  const withPhoto = page.getByRole("button", { name: /^Teams with a photo/ });
  await expect(withPhoto.getByText("2 / 3", { exact: true })).toBeVisible();
  const without = page.getByRole("button", { name: /^Without a photo/ });
  await expect(without.getByText("1", { exact: true })).toBeVisible();
  const decisions = page.getByRole("button", { name: /^Need a decision/ });
  await expect(decisions.getByText("2", { exact: true })).toBeVisible();

  const current = page.getByRole("table", {
    name: "Current photos",
    exact: true,
  });
  await expect(current.locator("tbody tr")).toHaveCount(2);
  await expect(current).toContainText("Alpha");
  await expect(current).toContainText("Beta");
  const thumbnail = current.getByRole("img", {
    name: "Photo of team1",
    exact: true,
  });
  await expect(thumbnail).toHaveAttribute(
    "src",
    `/api/v2/photo/photos/${ids.alpha}/image`,
  );
  await expect
    .poll(() => calls.includes(`GET /api/v2/photo/photos/${ids.alpha}/image`))
    .toBe(true);
});

test("without-photo tab lists rostered teams that have no current photo", async ({
  page,
}) => {
  await routeApi(page, "admin", async () => false);
  await page.goto("/photo");
  await page.getByRole("tab", { name: "Without photo", exact: true }).click();
  const missing = page.getByRole("table", {
    name: "Teams without a photo",
    exact: true,
  });
  await expect(missing.locator("tbody tr")).toHaveCount(1);
  await expect(missing).toContainText("team3");
  await expect(missing).not.toContainText("team1");
  // The roster profile was removed, so this account is not a team to photograph.
  await expect(missing).not.toContainText("team-old");
});

test("needs-decision tab links each candidate to its import", async ({
  page,
}) => {
  await routeApi(page, "admin", async (route, path) => {
    if (path !== "/api/v2/photo/submissions") return false;
    await route.fulfill({ json: { entries: attention, next: null } });
    return true;
  });
  await page.goto("/photo");
  await page.getByRole("tab", { name: "Needs decision", exact: true }).click();
  const table = page.getByRole("table", {
    name: "Photos needing a decision",
    exact: true,
  });
  await expect(table.locator("tbody tr")).toHaveCount(2);
  const review = table.getByRole("row").filter({ hasText: "team1" });
  await expect(review).toContainText("Needs review");
  await expect(
    review.getByRole("link", { name: "Open import", exact: true }),
  ).toHaveAttribute("href", `/photo/imports/${ids.importId}`);
  const orphan = table.getByRole("row").filter({ hasText: "team-old" });
  await expect(orphan).toContainText("Needs team");
  await expect(
    orphan.getByRole("link", { name: "Open import", exact: true }),
  ).toHaveAttribute("href", `/photo/imports/${ids.importId}`);
});

test("text filter narrows the current photos and reports empty matches", async ({
  page,
}) => {
  await routeApi(page, "admin", async () => false);
  await page.goto("/photo");
  const current = page.getByRole("table", {
    name: "Current photos",
    exact: true,
  });
  await expect(current.locator("tbody tr")).toHaveCount(2);
  const filter = page.getByLabel("Filter by team, school, or username", {
    exact: true,
  });
  await filter.fill("beta");
  await expect(current.locator("tbody tr")).toHaveCount(1);
  await expect(current).toContainText("team2");
  await filter.fill("no such team");
  await expect(
    page.getByText("No photos match the filter.", { exact: true }),
  ).toBeVisible();
});

test("Export photos posts the export and downloads the archive", async ({
  page,
}) => {
  const calls = await routeApi(page, "admin", async (route, path) => {
    if (path !== "/api/v2/photo/exports") return false;
    await route.fulfill({ body: zipBytes, contentType: "application/zip" });
    return true;
  });
  await page.goto("/photo");
  const downloaded = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Export photos", exact: true })
    .click();
  const file = await downloaded;
  expect(file.suggestedFilename()).toBe("team-photos.zip");
  expect(readFileSync((await file.path())!)).toEqual(zipBytes);
  expect(calls).toContain("POST /api/v2/photo/exports");
});

test("Export Pyrite assets posts the Pyrite export and downloads the archive", async ({
  page,
}) => {
  const calls = await routeApi(page, "admin", async (route, path) => {
    if (path !== "/api/v2/photo/exports/pyrite") return false;
    await route.fulfill({ body: zipBytes, contentType: "application/zip" });
    return true;
  });
  await page.goto("/photo");
  const downloaded = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Export Pyrite assets", exact: true })
    .click();
  const file = await downloaded;
  expect(file.suggestedFilename()).toBe("pyrite-assets.zip");
  expect(calls).toContain("POST /api/v2/photo/exports/pyrite");
});

test("a rejected Pyrite export shows the busy message instead of downloading", async ({
  page,
}) => {
  await routeApi(page, "admin", async (route, path) => {
    if (path !== "/api/v2/photo/exports/pyrite") return false;
    await route.fulfill({
      status: 503,
      json: { title: "Busy", status: 503, code: "SERVICE_UNAVAILABLE" },
    });
    return true;
  });
  await page.goto("/photo");
  await page
    .getByRole("button", { name: "Export Pyrite assets", exact: true })
    .click();
  await expect(
    page
      .getByRole("alert")
      .filter({ hasText: "The server is busy with another photo operation." }),
  ).toBeVisible();
});

test("team photos fit a 390px viewport", async ({ page }) => {
  await routeApi(page, "admin", async () => false);
  await page.setViewportSize({ width: 390, height: 900 });
  await page.goto("/photo");
  await expect(
    page.getByRole("table", { name: "Current photos", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Export photos", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(390);
});
