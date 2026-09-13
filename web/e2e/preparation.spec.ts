import { readFileSync } from "node:fs";
import { expect, test, type Page, type Route } from "@playwright/test";

const operator = {
  operator_id: "01912345-6789-7abc-8def-0123456789ab",
  role: "admin",
};
const candidateId = "01934567-89ab-7cde-8f01-23456789abcd";
const previewToken = "A".repeat(43);

const workbook = readFileSync(
  new URL("../../crates/roster/examples/roster.xlsx", import.meta.url),
);
const mediaType =
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const school = {
  organization_id: "INST-001",
  name_zh: "示例大学",
  name_en: "Example University",
  country: "CHN",
};
const team = {
  account: "team-new",
  seat: "B-02",
  organization_id: school.organization_id,
  name_zh: "星河",
  name_en: "Star River",
  category: "participant",
};
const diff = {
  accounts_added: ["team-new"],
  accounts_removed: ["team-old"],
  passwords_changed: ["team-002"],
  organizations: [school],
  organization_changes: [{ current: null, candidate: school }],
  team_changes: [{ account: team.account, current: null, candidate: team }],
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
      seat_code: "A-01",
      blocks_commit: false,
      device_id: "01956789-abcd-7ef0-8123-456789abcdef",
    },
  ],
};

const pendingSummary = {
  candidate_id: candidateId,
  expires_at_unix_ms: 4_090_579_800_000,
  diff,
};

function fulfillJson(route: Route, status: number, body: unknown) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });
}

async function mockPreparationApi(
  page: Page,
  options: {
    initialPending?: boolean;
    invalidUpload?: boolean;
    staleCommit?: boolean;
    blocked?: boolean;
  } = {},
) {
  const summary = {
    ...pendingSummary,
    diff: {
      ...diff,
      binding_impacts: diff.binding_impacts.map((impact) => ({
        ...impact,
        blocks_commit: options.blocked ?? false,
      })),
    },
  };
  let pending = options.initialPending ? summary : null;

  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;

    if (pathname === "/api/v2/session" && request.method() === "GET") {
      return fulfillJson(route, 200, operator);
    }

    if (
      pathname === "/api/v2/organizations" ||
      pathname === `/api/v2/imports/${candidateId}/organizations`
    ) {
      return fulfillJson(route, 200, [
        { ...school, status: "missing", files: [], detail: null },
      ]);
    }
    if (pathname === "/api/v2/imports" && request.method() === "GET") {
      return fulfillJson(route, 200, { pending });
    }

    if (pathname === "/api/v2/imports" && request.method() === "POST") {
      expect(request.headers()["content-type"]).toBe(mediaType);
      if (options.invalidUpload) {
        return fulfillJson(route, 400, {
          title: "Bad Request",
          status: 400,
          code: "IMPORT_CANDIDATE_INVALID",
        });
      }
      pending = summary;
      return fulfillJson(route, 201, {
        ...summary,
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
        });
      }
      expect(request.headers()["x-natsume-preview-token"]).toBe(previewToken);
      expect(request.headers()["content-type"]).toBe(mediaType);
      expect(request.postDataBuffer()).toEqual(workbook);
      pending = null;
      return route.fulfill({
        status: 204,
      });
    }

    if (
      pathname === `/api/v2/imports/${candidateId}` &&
      request.method() === "DELETE"
    ) {
      pending = null;
      return route.fulfill({
        status: 204,
      });
    }

    return fulfillJson(route, 200, []);
  });
}

async function uploadWorkbook(page: Page) {
  await page.getByLabel("XLSX file").setInputFiles({
    name: "roster.xlsx",
    mimeType: mediaType,
    buffer: workbook,
  });
  await page.getByRole("button", { name: "Create preview" }).click();
  await expect(page.getByText("Pending import", { exact: true })).toBeVisible();
}

test("preparation empty state offers XLSX upload", async ({ page }) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");

  await expect(
    page.getByRole("heading", { name: "Preparation Center" }),
  ).toBeVisible();
  await expect(
    page.getByText("Upload complete roster", { exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("XLSX file")).toBeVisible();
  await expect(
    page.getByRole("navigation").getByRole("link").first(),
  ).toHaveText("Preparation");
});

test("upload success renders the complete diff and enabled actions", async ({
  page,
}) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");
  await uploadWorkbook(page);

  await expect(page.getByText(candidateId, { exact: true })).toBeVisible();
  await expect(page.getByText("B-02", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("C-03", { exact: true }).first()).toBeVisible();
  await expect(
    page.getByText("team-old", { exact: true }).first(),
  ).toBeVisible();
  await expect(
    page.getByText("team-new", { exact: true }).first(),
  ).toBeVisible();
  await expect(
    page.getByText("Unchanged teams 2", { exact: true }),
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
      "Preview authorization and the reviewed XLSX are unavailable after a reload or session change; discard and re-upload to commit.",
      { exact: true },
    ),
  ).toBeVisible();
});

test("commit resubmits the reviewed XLSX and reaches the success state", async ({
  page,
}) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");
  await uploadWorkbook(page);

  await page.getByRole("button", { name: "Commit import" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText(
    "Applies the complete roster, including removals. Only changed passwords advance credential revisions. Bindings remain on their seats and session targets stay unchanged.",
  );
  await dialog.getByRole("button", { name: "Confirm commit" }).click();

  const success = page
    .getByRole("alert")
    .filter({ hasText: "Import committed" });
  await expect(success).toContainText(
    "The complete roster has been applied. Only changed passwords advance credential revisions.",
  );
});

test("a stale commit displays the discard and re-upload advisory", async ({
  page,
}) => {
  await mockPreparationApi(page, { staleCommit: true });
  await page.goto("/preparation");
  await uploadWorkbook(page);

  await page.getByRole("button", { name: "Commit import" }).click();
  await page.getByRole("button", { name: "Confirm commit" }).click();

  const alert = page
    .getByRole("alert")
    .filter({ hasText: "Import preview is stale" });
  await expect(alert).toContainText(
    "Discard this preview and re-upload the XLSX before committing.",
  );
});

test("reload loses the token but preserves tokenless discard recovery", async ({
  page,
}) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");
  await uploadWorkbook(page);
  await expect(page.locator("body")).not.toContainText(previewToken);

  await page.reload();
  await expect(page.getByText("Pending import", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeDisabled();
  await expect(
    page.getByText(
      "Preview authorization and the reviewed XLSX are unavailable after a reload or session change; discard and re-upload to commit.",
      { exact: true },
    ),
  ).toBeVisible();

  await page.getByRole("button", { name: "Discard preview" }).click();
  await expect(
    page.getByText("Upload complete roster", { exact: true }),
  ).toBeVisible();
});

test("invalid XLSX leaves a durable coded error notice", async ({ page }) => {
  await mockPreparationApi(page, { invalidUpload: true });
  await page.goto("/preparation");

  await page.getByLabel("XLSX file").setInputFiles({
    name: "invalid.xlsx",
    mimeType: mediaType,
    buffer: Buffer.from("seat,account,password\ninvalid-row"),
  });
  await page.getByRole("button", { name: "Create preview" }).click();

  const alert = page.getByRole("alert").filter({ hasText: "Bad Request" });
  await expect(alert).toContainText(
    "The XLSX did not satisfy the import contract.",
  );
});

test("occupied seat removal disables commit and shows the blocking device", async ({
  page,
}) => {
  await mockPreparationApi(page, { blocked: true });
  await page.goto("/preparation");
  await uploadWorkbook(page);
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeDisabled();
  await expect(
    page.getByText(
      "Removing an occupied seat requires releasing its binding first.",
    ),
  ).toBeVisible();
  await expect(page.getByText(diff.binding_impacts[0].device_id)).toBeVisible();
  await page.getByRole("button", { name: "Discard preview" }).click();
  await expect(page.getByLabel("XLSX file")).toHaveValue("");
  await expect(
    page.getByRole("button", { name: "Create preview" }),
  ).toBeDisabled();
});

test("navigation retains the exact reviewed workbook while metadata remains secret safe", async ({
  page,
}) => {
  await mockPreparationApi(page);
  await page.goto("/preparation");
  await uploadWorkbook(page);
  await expect(
    page.getByRole("region", { name: "Team changes" }),
  ).toContainText("Star River");
  await expect(
    page.getByRole("region", { name: "School changes" }),
  ).toContainText("Example University");
  await expect(
    page.getByRole("region", { name: "Passwords changed" }),
  ).toContainText("team-002");
  await page.getByRole("link", { name: "Devices", exact: true }).click();
  await page.getByRole("link", { name: "Preparation", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Commit import" }),
  ).toBeEnabled();
  await expect(page.locator("body")).not.toContainText("example-password");
  await expect(page.locator("body")).not.toContainText(previewToken);
  const storage = await page.evaluate(() =>
    JSON.stringify([localStorage, sessionStorage]),
  );
  expect(storage).not.toContain(previewToken);
  expect(storage).not.toContain("example-password");
  await page.getByRole("button", { name: "Commit import" }).click();
  await page.getByRole("button", { name: "Confirm commit" }).click();
  await expect(
    page.getByText("Import committed", { exact: true }),
  ).toBeVisible();
});

for (const width of [1024, 1440]) {
  test(`import table headers stay visible while scrolling at ${width}`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await mockPreparationApi(page, { initialPending: true });
    const roster = Array.from({ length: 80 }, (_, index) => {
      const number = String(index + 1).padStart(3, "0");
      return {
        ...team,
        account: `team-${number}`,
        seat: `A-${number}-${"W".repeat(56)}`,
        name_zh: `示例参赛队伍 ${number}`,
        organization_id: `INST-${number}`,
      };
    });
    const schools = roster.map((team) => ({
      ...school,
      organization_id: team.organization_id,
      name_zh: `示例大学 ${team.organization_id}`,
    }));
    const pending = {
      ...pendingSummary,
      diff: {
        ...diff,
        accounts_added: roster.map((team) => team.account),
        accounts_removed: [],
        passwords_changed: [],
        organizations: schools,
        affected_account_count: roster.length,
        unchanged_count: 0,
        team_changes: roster.map((team) => ({
          account: team.account,
          current: null,
          candidate: team,
        })),
        organization_changes: schools.map((school) => ({
          current: null,
          candidate: school,
        })),
        seats_added: roster.map((team) => team.seat),
        seats_removed: [],
        mappings_changed: roster.map((team) => ({
          seat_code: team.seat,
          current_domjudge_username: null,
          candidate_domjudge_username: team.account,
        })),
        binding_impacts: roster.map((team, index) => ({
          seat_code: team.seat,
          device_id: `01956789-abcd-7ef0-8123-${index.toString(16).padStart(12, "0")}`,
          blocks_commit: false,
        })),
      },
    };
    await page.route("**/api/v2/imports", (route) => {
      expect(route.request().method()).toBe("GET");
      return fulfillJson(route, 200, { pending });
    });
    await page.route(
      `**/api/v2/imports/${candidateId}/organizations`,
      (route) => {
        expect(route.request().method()).toBe("GET");
        return fulfillJson(
          route,
          200,
          schools.map((school) => ({
            ...school,
            status: "missing",
            files: [],
            detail: null,
          })),
        );
      },
    );
    await page.goto("/preparation");
    const areas = [
      page.getByRole("region", { name: "Team changes" }),
      page.getByRole("region", { name: "School changes" }),
      page.getByRole("region", { name: "Seat changes", exact: true }),
      page.getByRole("region", { name: "Mapping changes", exact: true }),
      page.getByRole("alert").filter({ hasText: "Binding impacts" }),
      page.getByTestId("preview-logos"),
    ];
    for (const [index, area] of areas.entries()) {
      const scroller = area.locator('[data-slot="table-container"]');
      await expect(scroller.getByRole("row")).toHaveCount(81);
      await scroller.evaluate((element) =>
        element.scrollIntoView({ block: "center" }),
      );
      const firstHeader = scroller.getByRole("columnheader").first();
      const before = await firstHeader.boundingBox();
      const beforeRow = await scroller
        .locator("tbody tr")
        .first()
        .boundingBox();
      const movement = await scroller.evaluate((element) => {
        element.scrollTop = element.clientHeight * 2;
        element.scrollLeft = Math.min(
          180,
          element.scrollWidth - element.clientWidth,
        );
        return { top: element.scrollTop, left: element.scrollLeft };
      });
      expect(movement.top).toBeGreaterThan(0);
      if (index === 0 && width === 1024)
        expect(movement.left).toBeGreaterThan(0);
      await expect
        .poll(async () => {
          const header = await firstHeader.boundingBox();
          const container = await scroller.boundingBox();
          return Math.abs(header!.y - container!.y);
        })
        .toBeLessThan(2);
      const after = await firstHeader.boundingBox();
      const afterRow = await scroller.locator("tbody tr").first().boundingBox();
      expect(Math.abs(before!.x - after!.x - movement.left)).toBeLessThan(2);
      expect(beforeRow!.y - afterRow!.y).toBeGreaterThan(0);
      expect(
        await scroller.evaluate((element) => {
          const container = element.getBoundingClientRect();
          const header = element.querySelector("thead")!;
          const bounds = header.getBoundingClientRect();
          return header.contains(
            document.elementFromPoint(
              container.x + container.width / 2,
              bounds.y + bounds.height / 2,
            ),
          );
        }),
      ).toBe(true);
      if (index === 0) {
        await area.screenshot({
          path: testInfo.outputPath("team-changes-scrolled.png"),
        });
      }
    }
  });

  test(`roster preview fits a ${width}-wide screen`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await mockPreparationApi(page);
    await page.goto("/preparation");
    await uploadWorkbook(page);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    await expect(
      page.getByTestId("preview-logos").getByText("INST-001", { exact: true }),
    ).toBeVisible();
    await page.screenshot({
      path: testInfo.outputPath("roster-preview.png"),
      fullPage: true,
    });
  });
}

for (const width of [1024, 1440]) {
  test(`school logos support 240 schools, filtering and refresh at ${width}`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 1000 });
    await mockPreparationApi(page);
    const schools = Array.from({ length: 240 }, (_, i) => ({
      ...school,
      organization_id: `INST-${String(i + 1).padStart(3, "0")}`,
      name_zh: `示例大学${i + 1}`,
      name_en: `Example University ${i + 1}`,
      status: i % 3 === 0 ? "missing" : "available",
      files: i % 3 === 0 ? [] : [`示例大学${i + 1}.webp`],
      detail: null,
    }));
    let changed = false;
    await page.route("**/api/v2/organizations", (route) =>
      fulfillJson(
        route,
        200,
        schools.map((school) =>
          changed && school.organization_id === "INST-001"
            ? { ...school, status: "available", files: ["示例大学1.webp"] }
            : school,
        ),
      ),
    );
    await page.route("**/api/v2/organizations/*/logo*", (route) =>
      route.fulfill({
        contentType: "image/png",
        body: Buffer.from(
          "iVBORw0KGgoAAAANSUhEUgAAADAAAAAwCAYAAABXAvmHAAABWElEQVR4nO1abRYBMQwszxE4AhfjWFyMI3AHftUj2m4+pkmx83ubzGSS6NuV0oxYLJDBtvvrnfPc5bSB5TUH4pKuwSpGfdhKnEIrRHwITZxCKmQpebg3eU0OtgAP8ppcLAGe5KU5JwVEkJfkbgqIJM/lUBWAIn8+rs0xWlyKKwtBvkR8d7iZYpZWrGiNclGrOsINig9FlupLCGrdoC6sVFEINJXNZ6xt9aZGWn1kS0iEvLqgngF0P2vjiVuoxyDS2BI3nlZEtk+Gpo3ULWQdPlQ80xbKSS1uWAsBWaMaISgHIQIyOELQrdflKlEjiSafkvMvcY8LXhcHPDELiIbpMhcFyGVuFPyWAORb416gHH/LgZTGdsHtrYQnigJGdKHGqerASCJaXJotNIKIKQ6TMxApgpObNcQRIrg52VvIU4Qkl2iNeoiQ5vi/z6wUX/uhu4SIvxrMiMYD1Y+eNwLbNdkAAAAASUVORK5CYII=",
          "base64",
        ),
      }),
    );
    await page.goto("/preparation");
    await expect(page.getByText("240 schools", { exact: true })).toBeVisible();
    const table = page
      .getByTestId("committed-logos")
      .locator('[data-slot="table-container"]');
    await expect(table.getByRole("row")).toHaveCount(241);
    await page.getByRole("button", { name: "Problems (80)" }).click();
    await expect(table.getByRole("row")).toHaveCount(81);
    await page.getByLabel("Search committed schools").fill("INST-001");
    await expect(table.getByRole("row")).toHaveCount(2);
    await expect(table.getByText("Missing", { exact: true })).toBeVisible();
    changed = true;
    await page.getByRole("button", { name: "Refresh logos" }).click();
    await expect(
      page.getByRole("button", { name: "Problems (79)" }),
    ).toBeVisible();
    await expect(table.getByText("No results.")).toBeVisible();
    await page.getByRole("button", { name: "Problems (79)" }).click();
    await expect(table.getByText("Available", { exact: true })).toBeVisible();
    await page.getByLabel("Search committed schools").fill("");
    await table.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    const scrollTop = await table.evaluate((element) => element.scrollTop);
    expect(scrollTop).toBeGreaterThan(0);
    await page.getByRole("button", { name: "Refresh logos" }).click();
    await expect
      .poll(() => table.evaluate((element) => element.scrollTop))
      .toBe(scrollTop);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    await table.evaluate((element) => {
      element.scrollTop = 0;
    });
    await page.screenshot({
      path: testInfo.outputPath("school-logos.png"),
      fullPage: true,
    });
  });
}

test("DOMjudge download reports image failure and downloads the full ZIP after retry", async ({
  page,
}) => {
  await mockPreparationApi(page);
  let broken = true;
  await page.route("**/api/v2/exports/domjudge", (route) =>
    broken
      ? fulfillJson(route, 500, {
          code: "INTERNAL_ERROR",
          status: 500,
          title: "INST-001 示例大学: 示例大学.webp: cannot decode image",
        })
      : route.fulfill({
          contentType: "application/zip",
          headers: {
            "Content-Disposition": 'attachment; filename="domjudge-export.zip"',
            "Cache-Control": "no-store",
          },
          body: Buffer.from("PK\x03\x04synthetic-export"),
        }),
  );
  await page.goto("/preparation");
  await page.getByRole("button", { name: "Download DOMjudge ZIP" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "INST-001 示例大学: 示例大学.webp",
  );
  broken = false;
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download DOMjudge ZIP" }).click();
  expect((await download).suggestedFilename()).toBe("domjudge-export.zip");
  expect(
    await page.evaluate(() =>
      JSON.stringify({
        local: { ...localStorage },
        session: { ...sessionStorage },
      }),
    ),
  ).not.toContain("synthetic-export");
});
