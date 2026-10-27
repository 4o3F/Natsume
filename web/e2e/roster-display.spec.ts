import { expect, test, type Page } from "@playwright/test";

async function expectPlainSchoolLogos(page: Page) {
  for (const logo of [
    page.getByRole("img", { name: "示例科技大学 logo", exact: true }),
    page.getByRole("img", { name: /\(logo unavailable\)$/ }),
  ]) {
    await expect(logo).toBeVisible();
    await expect(logo).toHaveCSS("border-width", "0px");
    await expect(logo).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
    await expect(logo).toHaveCSS("box-shadow", "none");
    await expect(logo).toHaveCSS("width", "32px");
    await expect(logo).toHaveCSS("height", "32px");
  }
}

test("seat binding filters remain usable with no matches and support the keyboard", async ({
  page,
}) => {
  await page.route("**/api/v2/**", (route) => {
    expect(route.request().method()).toBe("GET");
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session")
      return route.fulfill({
        json: { operator_id: "operator", role: "admin", username: "operator" },
      });
    if (path === "/api/v2/seats")
      return route.fulfill({
        json: [
          { seat_id: "seat-a", seat_code: "A-01" },
          { seat_id: "seat-b", seat_code: "B-02" },
        ],
      });
    if (path === "/api/v2/bindings")
      return route.fulfill({
        json: [
          { binding_id: "binding-a", seat_id: "seat-a", device_id: "device-a" },
        ],
      });
    if (path === "/api/v2/accounts") return route.fulfill({ json: [] });
    return route.fulfill({ status: 404, json: {} });
  });
  await page.goto("/seats");
  await expect(
    page.getByRole("heading", { name: "Seats", exact: true }),
  ).toBeVisible();
  await page.getByRole("radio", { name: "List view", exact: true }).click();
  const filter = page.getByRole("group", {
    name: "Binding",
    exact: true,
  });
  const bound = filter.getByRole("checkbox", { name: /^Bound\b/ });
  const unbound = filter.getByRole("checkbox", {
    name: /^Unbound\b/,
  });
  await expect(page.locator("tbody tr")).toHaveCount(2);
  await expect(
    page
      .locator("tbody tr")
      .first()
      .getByRole("img", { name: "Binding: bound", exact: true }),
  ).toBeVisible();
  await expect(page.locator("tbody tr").last()).toHaveClass(
    /bg-warning-subtle/,
  );
  await bound.locator("..").click();
  await expect(page.locator("tbody tr td:first-child")).toHaveText(["B-02"]);
  await unbound.locator("..").click();
  await expect(page.getByText("No seats match", { exact: true })).toBeVisible();
  await expect(filter).toBeVisible();
  await bound.focus();
  await page.keyboard.press("Space");
  await expect(bound).toBeChecked();
  await expect(page.locator("tbody tr td:first-child")).toHaveText(["A-01"]);
  await unbound.locator("..").click();
  await expect(page.locator("tbody tr")).toHaveCount(2);
});

for (const width of [1024, 1440]) {
  test(`team and school display stays attached to its seat at ${width}`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    const teams = [
      {
        seat_id: "seat-b",
        seat_code: "B-02",
        organization_id: "INST-001",
        team_name_zh: "星河漫游",
        team_name_en: "Voyagers of the Stars",
        school_name_zh: "示例科技大学",
        school_name_en: "Example University of Science and Technology",
      },
      {
        seat_id: "seat-a",
        seat_code: "A-01",
        organization_id: "INST-002",
        team_name_zh: "需要完整显示的很长的队伍名称".repeat(4),
        team_name_en: "A long but fully readable team name ".repeat(4),
        school_name_zh: "",
        school_name_en: "Another University",
      },
    ];
    await page.route("**/api/v2/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === "/api/v2/session") {
        return route.fulfill({
          json: {
            operator_id: "operator",
            role: "admin",
            username: "operator",
          },
        });
      }
      if (path === "/api/v2/accounts") {
        return route.fulfill({
          json: teams.map((team, index) => ({
            account_id: `account-${index}`,
            domjudge_username: `team-${index}`,
            credential_revision: 1,
            team,
          })),
        });
      }
      if (path === "/api/v2/seats") {
        return route.fulfill({
          json: [...teams]
            .reverse()
            .map(({ seat_id, seat_code }) => ({ seat_id, seat_code })),
        });
      }
      if (path === "/api/v2/bindings") {
        return route.fulfill({ json: [] });
      }
      if (path === "/api/v2/organizations/INST-001/logo") {
        return route.fulfill({
          contentType: "image/png",
          body: Buffer.from(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aAuoAAAAASUVORK5CYII=",
            "base64",
          ),
        });
      }
      return route.fulfill({ status: 404, json: {} });
    });
    await page.goto("/accounts");
    await expect(page.getByText("Voyagers of the Stars")).toBeVisible();
    await expectPlainSchoolLogos(page);
    await page.screenshot({
      path: testInfo.outputPath("accounts-school-logos.png"),
      fullPage: true,
    });
    await expect(page.getByText("Offline", { exact: true })).toHaveCount(0);
    await page.goto("/seats");
    await page.getByRole("radio", { name: "List view", exact: true }).click();
    const row = page.getByRole("row").filter({ hasText: "B-02" });
    await expect(row).toContainText("星河漫游");
    await expect(row).toContainText("INST-001");
    await expect(
      page.getByRole("row").filter({ hasText: "A-01" }),
    ).toContainText(teams[1].team_name_zh);
    await expectPlainSchoolLogos(page);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    await page.screenshot({
      path: testInfo.outputPath("roster-display.png"),
      fullPage: true,
    });
  });
}
