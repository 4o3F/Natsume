import { expect, test, type Page, type Route } from "@playwright/test";

const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const candidateId = "01934567-89ab-7cde-8f01-23456789abcd";
const correlationId = "01945678-9abc-7def-8012-3456789abcde";
const previewToken = "A".repeat(43);

const diff = {
  seats_added: ["B-02"],
  seats_removed: ["C-03"],
  mappings_changed: [
    {
      seat_code: "A-01",
      current_domjudge_username: "team-old",
      candidate_domjudge_username: "team-new",
    },
  ],
  unchanged_count: 2,
  affected_account_count: 3,
  binding_impacts: [
    {
      seat_code: "C-03",
      device_id: "01956789-abcd-7ef0-8123-456789abcdef",
    },
  ],
};

const pendingSummary = {
  candidate_id: candidateId,
  expires_at: "2099-08-16T12:30:00.000Z",
  baseline_configuration_revision: 7,
  baseline_binding_revision: 4,
  diff,
};

function fulfillJson(route: Route, status: number, body: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    headers: { "X-Correlation-Id": correlationId },
    body: JSON.stringify(body),
  });
}

async function mockPreparationApi(
  page: Page,
  options: {
    initialPending?: boolean;
    invalidUpload?: boolean;
    staleCommit?: boolean;
  } = {},
) {
  let pending = options.initialPending ? pendingSummary : null;

  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session" && request.method() === "GET") {
      return fulfillJson(route, 200, operator);
    }

    if (pathname === "/api/v2/imports" && request.method() === "GET") {
      return fulfillJson(route, 200, { pending });
    }

    if (pathname === "/api/v2/imports" && request.method() === "POST") {
      if (options.invalidUpload) {
        return fulfillJson(route, 400, {
          title: "Bad Request",
          status: 400,
          code: "IMPORT_CANDIDATE_INVALID",
          correlation_id: correlationId,
        });
      }
      pending = pendingSummary;
      return fulfillJson(route, 201, {
        ...pendingSummary,
        preview_token: previewToken,
      });
    }

    if (
      pathname === `/api/v2/imports/${candidateId}/actions/commit` &&
      request.method() === "POST"
    ) {
      if (options.staleCommit) {
        return fulfillJson(route, 409, {
          title: "Conflict",
          status: 409,
          code: "IMPORT_PREVIEW_STALE",
          correlation_id: correlationId,
        });
      }
      pending = null;
      return fulfillJson(route, 200, {
        configuration_revision: 8,
        binding_revision: 5,
      });
    }

    if (
      pathname === `/api/v2/imports/${candidateId}/actions/discard` &&
      request.method() === "POST"
    ) {
      pending = null;
      return route.fulfill({
        status: 204,
        headers: { "X-Correlation-Id": correlationId },
      });
    }

    return fulfillJson(route, 200, []);
  });
}

async function uploadCsv(page: Page) {
  await page.getByLabel("CSV file").setInputFiles({
    name: "contest.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(
      "seat,account,password\nA-01,team-new,e2e-password-canary",
    ),
  });
  await page.getByRole("button", { name: "Create preview" }).click();
  await expect(page.getByText("Pending import", { exact: true })).toBeVisible();
}

test("preparation empty state offers CSV upload", async ({ page }) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");

  await expect(
    page.getByRole("heading", { name: "Preparation Center" }),
  ).toBeVisible();
  await expect(
    page.getByText("Upload contest CSV", { exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("CSV file")).toBeVisible();
  await expect(
    page.getByRole("navigation").getByRole("link").first(),
  ).toHaveText("Preparation");
});

test("upload success renders the complete diff and enabled actions", async ({
  page,
}) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");
  await uploadCsv(page);

  await expect(page.getByText(candidateId, { exact: true })).toBeVisible();
  await expect(page.getByText("B-02", { exact: true })).toBeVisible();
  await expect(page.getByText("C-03", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("team-old", { exact: true })).toBeVisible();
  await expect(page.getByText("team-new", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Unchanged seats 2", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Affected accounts 3", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Discard preview" }),
  ).toBeEnabled();
});

test("a restored pending candidate disables commit but leaves discard available", async ({
  page,
}) => {
  await mockPreparationApi(page, { initialPending: true });
  await page.goto("/preparation");

  await expect(page.getByText("Pending import", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Discard preview" }),
  ).toBeEnabled();
  await expect(
    page.getByText(
      "Preview token is unavailable after reload; discard and re-upload to commit.",
      { exact: true },
    ),
  ).toBeVisible();
});

test("commit confirmation reaches the revision success state", async ({
  page,
}) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");
  await uploadCsv(page);

  await page.getByRole("button", { name: "Commit import" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText(
    "Replaces the entire confirmed configuration and advances every account credential revision.",
  );
  await dialog.getByRole("button", { name: "Confirm commit" }).click();

  const success = page
    .getByRole("alert")
    .filter({ hasText: "Import committed" });
  await expect(success).toContainText("Configuration revision 8");
  await expect(success).toContainText("binding revision 5");
});

test("a stale commit displays the discard and re-upload advisory", async ({
  page,
}) => {
  await mockPreparationApi(page, { staleCommit: true });
  await page.goto("/preparation");
  await uploadCsv(page);

  await page.getByRole("button", { name: "Commit import" }).click();
  await page.getByRole("button", { name: "Confirm commit" }).click();

  const alert = page
    .getByRole("alert")
    .filter({ hasText: "Import preview is stale" });
  await expect(alert).toContainText(
    "Discard this preview and re-upload the CSV before committing.",
  );
  await expect(alert).toContainText(`Correlation ID: ${correlationId}`);
});

test("reload loses the token but preserves tokenless discard recovery", async ({
  page,
}) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");
  await uploadCsv(page);
  await expect(page.locator("body")).not.toContainText(previewToken);

  await page.reload();
  await expect(page.getByText("Pending import", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeDisabled();
  await expect(
    page.getByText(
      "Preview token is unavailable after reload; discard and re-upload to commit.",
      { exact: true },
    ),
  ).toBeVisible();

  await page.getByRole("button", { name: "Discard preview" }).click();
  await expect(
    page.getByText("Upload contest CSV", { exact: true }),
  ).toBeVisible();
});

test("invalid CSV leaves a durable coded error notice", async ({ page }) => {
  await mockPreparationApi(page, { invalidUpload: true });
  await page.goto("/preparation");

  await page.getByLabel("CSV file").setInputFiles({
    name: "invalid.csv",
    mimeType: "text/csv",
    buffer: Buffer.from("seat,account,password\ninvalid-row"),
  });
  await page.getByRole("button", { name: "Create preview" }).click();

  const alert = page.getByRole("alert").filter({ hasText: "Bad Request" });
  await expect(alert).toContainText(
    "The CSV did not satisfy the import contract.",
  );
  await expect(alert).toContainText(`Correlation ID: ${correlationId}`);
});
