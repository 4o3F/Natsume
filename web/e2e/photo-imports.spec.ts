import { expect, test, type Page } from "@playwright/test";

import type {
  PhotoPreviewEntry_Serialize as PhotoPreviewEntry,
  ResultImportJob_Serialize as ResultImportJob,
  ResultImportState,
} from "../src/api/generated/types";
import { ids, routeApi, zipBytes } from "./fixtures/photo";

const base = `/api/v2/photo/imports/${ids.importId}`;

// Match the candidate row by its exact team cell: the roster select in another
// row also contains option text such as "team1 · Alpha".
function rowOf(page: Page, username: string) {
  return page
    .getByRole("row")
    .filter({ has: page.getByText(username, { exact: true }) });
}

function job(
  state: ResultImportState,
  extra: Partial<ResultImportJob> = {},
): ResultImportJob {
  return {
    import_id: ids.importId,
    state,
    uploaded_bytes: zipBytes.length,
    verified_images: 0,
    failure_code: null,
    ...extra,
  };
}

// Pending candidates for Alpha (has a current photo) and for a team that left the roster.
function entries(): PhotoPreviewEntry[] {
  return [
    {
      submission_id: "sub-1",
      result_id: "res-1",
      account_id: ids.alpha,
      account: "team1",
      image_sha256: "1".repeat(64),
      state: "pending_review",
      submission_revision: 3,
      current: {
        source_id: "src-2",
        submission_id: "sub-alpha",
        revision: 1,
        image_sha256: "a".repeat(64),
      },
      active_account_id: null,
    },
    {
      submission_id: "sub-2",
      result_id: "res-2",
      account_id: ids.retired,
      account: "team-old",
      image_sha256: "2".repeat(64),
      state: "needs_association",
      submission_revision: 1,
      current: null,
      active_account_id: null,
    },
    {
      submission_id: "sub-3",
      result_id: "res-3",
      account_id: ids.beta,
      account: "team2",
      image_sha256: "b".repeat(64),
      state: "active",
      submission_revision: 1,
      current: {
        source_id: "src-2",
        submission_id: "sub-beta",
        revision: 1,
        image_sha256: "b".repeat(64),
      },
      active_account_id: ids.beta,
    },
  ];
}

async function importApi(page: Page, initial: ResultImportJob) {
  const state = {
    job: initial,
    committed: initial.state === "committed",
    entries: entries(),
    uploads: [] as string[],
    decisions: [] as unknown[],
    commits: 0,
    uploadStatus: null as number | null,
  };
  const preview = () => ({
    import_id: ids.importId,
    committed: state.committed,
    entries: state.entries,
  });
  const calls = await routeApi(page, "admin", async (route, path) => {
    const request = route.request();
    const method = request.method();
    if (path === "/api/v2/photo/imports" && method === "POST") {
      state.uploads.push(request.headers()["content-type"] ?? "");
      if (state.uploadStatus)
        await route.fulfill({
          status: state.uploadStatus,
          json: {
            title: "Payload too large",
            status: state.uploadStatus,
            code: "PAYLOAD_TOO_LARGE",
          },
        });
      else await route.fulfill({ json: state.job });
      return true;
    }
    if (path === base && method === "GET") {
      await route.fulfill({ json: state.job });
      return true;
    }
    if (path === `${base}/preview`) {
      await route.fulfill({ json: preview() });
      return true;
    }
    if (path === `${base}/commit`) {
      state.commits += 1;
      state.committed = true;
      state.job = job("committed");
      await route.fulfill({ json: preview() });
      return true;
    }
    if (path === `${base}/decisions`) {
      const decision = request.postDataJSON();
      state.decisions.push(decision);
      const entry = state.entries.find(
        (candidate) => candidate.submission_id === decision.submission_id,
      );
      if (entry)
        entry.state =
          decision.action.action === "reject" ? "rejected" : "active";
      await route.fulfill({ json: preview() });
      return true;
    }
    return false;
  });
  return { calls, state };
}

test("uploading a ZIP sends it as application/zip and opens the import", async ({
  page,
}) => {
  const { calls, state } = await importApi(page, job("processing"));
  await page.goto("/photo/imports");
  await page.getByLabel("Result package ZIP", { exact: true }).setInputFiles({
    name: "result.zip",
    mimeType: "application/zip",
    buffer: zipBytes,
  });
  await expect(page).toHaveURL(new RegExp(`/photo/imports/${ids.importId}$`));
  expect(state.uploads).toEqual(["application/zip"]);

  const status = page.getByRole("region", {
    name: "Import status",
    exact: true,
  });
  await expect(status).toContainText("Verifying package");
  await expect(status).toContainText("This page updates automatically.");
  await expect(page.getByRole("table", { name: "Package images" })).toHaveCount(
    0,
  );

  state.job = job("ready", { verified_images: 3 });
  await expect(status).toContainText("Ready to commit", { timeout: 10_000 });
  await expect(
    page
      .getByRole("table", { name: "Package images", exact: true })
      .locator("tbody tr"),
  ).toHaveCount(3);
  await expect
    .poll(() => calls.filter((call) => call === `GET ${base}`).length)
    .toBeGreaterThan(1);
});

test("a rejected upload shows the size message and stays on the upload page", async ({
  page,
}) => {
  const { state } = await importApi(page, job("processing"));
  state.uploadStatus = 413;
  await page.goto("/photo/imports");
  await page.getByLabel("Result package ZIP", { exact: true }).setInputFiles({
    name: "huge.zip",
    mimeType: "application/zip",
    buffer: zipBytes,
  });
  await expect(
    page
      .getByRole("alert")
      .filter({ hasText: "The package or export exceeds its limit." }),
  ).toBeVisible();
  await expect(page).toHaveURL(/\/photo\/imports$/);
});

test("a failed import shows its failure text and no package contents", async ({
  page,
}) => {
  const { calls } = await importApi(
    page,
    job("failed", { failure_code: "signature" }),
  );
  await page.goto(`/photo/imports/${ids.importId}`);
  await expect(
    page
      .getByRole("region", { name: "Import status", exact: true })
      .getByRole("alert"),
  ).toContainText(
    "The manifest signature or its initialization context was rejected.",
  );
  await expect(page.getByRole("table", { name: "Package images" })).toHaveCount(
    0,
  );
  expect(calls.some((call) => call.endsWith("/preview"))).toBe(false);
});

test("committing asks for confirmation before sending the commit", async ({
  page,
}) => {
  const { calls, state } = await importApi(page, job("ready"));
  await page.goto(`/photo/imports/${ids.importId}`);
  const table = page.getByRole("table", {
    name: "Package images",
    exact: true,
  });
  await expect(table).toBeVisible();
  await expect(table.getByText("Commit the package first")).toHaveCount(2);

  await page
    .getByRole("button", { name: "Commit package", exact: true })
    .click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("Commit this package?");
  await dialog.getByRole("button", { name: "Not yet", exact: true }).click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(calls).not.toContain(`POST ${base}/commit`);

  await page
    .getByRole("button", { name: "Commit package", exact: true })
    .click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Commit package", exact: true })
    .click();
  await expect.poll(() => state.commits).toBe(1);
  await expect(
    page.getByText("Package committed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("region", { name: "Import status", exact: true }),
  ).toContainText("Committed");
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Commit package", exact: true }),
  ).toHaveCount(0);
  await expect(table.getByText("Commit the package first")).toHaveCount(0);
});

test("rejecting a candidate sends a reject decision", async ({ page }) => {
  const { state } = await importApi(page, job("committed"));
  await page.goto(`/photo/imports/${ids.importId}`);
  const row = rowOf(page, "team1");
  await row.getByRole("button", { name: "Reject", exact: true }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("Reject the photo for team1?");
  await dialog
    .getByRole("button", { name: "Reject photo", exact: true })
    .click();
  await expect.poll(() => state.decisions.length).toBe(1);
  expect(state.decisions[0]).toEqual({
    submission_id: "sub-1",
    expected_submission_revision: 3,
    action: { action: "reject" },
  });
  await expect(rowOf(page, "team1")).toContainText("Rejected");
});

test("replacing a current photo sends activate with the photo it replaces", async ({
  page,
}) => {
  const { state } = await importApi(page, job("committed"));
  await page.goto(`/photo/imports/${ids.importId}`);
  const row = rowOf(page, "team1");
  await row.getByRole("button", { name: "Replace photo", exact: true }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText("Use this photo for team1?");
  await expect(dialog).toContainText("replaces the team");
  await dialog.getByRole("button", { name: "Use photo", exact: true }).click();
  await expect.poll(() => state.decisions.length).toBe(1);
  expect(state.decisions[0]).toEqual({
    submission_id: "sub-1",
    expected_submission_revision: 3,
    action: {
      action: "activate",
      account_id: ids.alpha,
      expected_current: { source_id: "src-2", submission_id: "sub-alpha" },
    },
  });
});

test("a candidate whose team left the roster needs a team before Use photo is enabled", async ({
  page,
}) => {
  const { state } = await importApi(page, job("committed"));
  await page.goto(`/photo/imports/${ids.importId}`);
  const row = rowOf(page, "team-old");
  await expect(
    row.getByRole("button", { name: "Use photo", exact: true }),
  ).toBeDisabled();
  await expect(row.getByRole("button", { name: "Replace photo" })).toHaveCount(
    0,
  );
  await expect(
    row.getByLabel("Assign team-old to team", { exact: true }),
  ).toBeVisible();
  expect(state.decisions).toHaveLength(0);
});

for (const [team, accountId, expectedCurrent] of [
  ["team2", ids.beta, { source_id: "src-2", submission_id: "sub-beta" }],
  ["team3", ids.gamma, null],
] as const)
  test(`assigning a candidate to ${team} sends its roster account and the current photo token`, async ({
    page,
  }) => {
    const { state } = await importApi(page, job("committed"));
    await page.goto(`/photo/imports/${ids.importId}`);
    const row = rowOf(page, "team-old");
    await row
      .getByLabel("Assign team-old to team", { exact: true })
      .selectOption(accountId);
    await row.getByRole("button", { name: "Use photo", exact: true }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText(`Use this photo for ${team}?`);
    await dialog
      .getByRole("button", { name: "Use photo", exact: true })
      .click();
    await expect.poll(() => state.decisions.length).toBe(1);
    expect(state.decisions[0]).toEqual({
      submission_id: "sub-2",
      expected_submission_revision: 1,
      action: {
        action: "activate",
        account_id: accountId,
        expected_current: expectedCurrent,
      },
    });
  });

test("cancelling the Use photo confirmation sends nothing", async ({
  page,
}) => {
  const { state } = await importApi(page, job("committed"));
  await page.goto(`/photo/imports/${ids.importId}`);
  const row = rowOf(page, "team-old");
  await row
    .getByLabel("Assign team-old to team", { exact: true })
    .selectOption(ids.beta);
  await row.getByRole("button", { name: "Use photo", exact: true }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(state.decisions).toHaveLength(0);
});

test("Open an earlier import navigates by a valid import ID", async ({
  page,
}) => {
  await importApi(page, job("ready"));
  await page.goto("/photo/imports");
  const input = page.getByLabel("Import ID", { exact: true });
  const open = page.getByRole("button", { name: "Open", exact: true });
  await input.fill("not-an-import-id");
  await expect(open).toBeDisabled();
  await input.fill(ids.importId.toUpperCase());
  await expect(open).toBeEnabled();
  await open.click();
  await expect(page).toHaveURL(new RegExp(`/photo/imports/${ids.importId}$`));
});
