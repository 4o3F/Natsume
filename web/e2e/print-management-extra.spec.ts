import { expect, test } from "@playwright/test";

test("pairing code display expires without retaining or recovering the code", async ({
  page,
}) => {
  await page.clock.install();
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/session"))
      return route.fulfill({
        json: { operator_id: "operator", username: "operator", role: "admin" },
      });
    if (path.endsWith("/print/printers"))
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
    if (path.endsWith("/pairing-code"))
      return route.fulfill({
        json: {
          printer_id: "printer",
          code: "EXPIRE-ME",
          expires_at_unix_ms: Date.now() + 600000,
        },
      });
    return route.fulfill({ status: 404, json: {} });
  });
  await page.goto("/print/printers");
  await page.getByRole("button", { name: "Issue pairing code" }).click();
  await expect(page.getByText("EXPIRE-ME", { exact: true })).toBeVisible();
  await page.clock.fastForward(660000);
  await expect(page.getByText("EXPIRE-ME", { exact: true })).toHaveCount(0);
});

test("management lists keep long names and IDs inside a narrow viewport", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/session"))
      return route.fulfill({
        json: { operator_id: "operator", username: "operator", role: "admin" },
      });
    if (path.endsWith("/print/printers"))
      return route.fulfill({
        json: [
          {
            printer_id: "01a167d2-ee05-7360-b976-c4f6a10be330",
            name: "打印队列".repeat(32),
            client_id: "01a167d2-ee05-7360-b976-c4f6a10be331",
            client_confirmed: true,
            pairing_expires_at_unix_ms: null,
          },
        ],
      });
    if (path.endsWith("/print/sources"))
      return route.fulfill({
        json: [
          {
            source_id: "01a167d2-ee05-7360-b976-c4f6a10be332",
            name: "来源名称".repeat(32),
            revoked: false,
          },
        ],
      });
    return route.fulfill({ status: 404, json: {} });
  });
  for (const path of ["printers", "sources"]) {
    await page.goto(`/print/${path}`);
    if (path === "printers") {
      const card = page.getByRole("article");
      await expect(card).toHaveCount(1);
      await expect(card).toContainText("01a167d2-ee05-7360-b976-c4f6a10be330");
      await expect(card).toContainText("01a167d2-ee05-7360-b976-c4f6a10be331");
    } else {
      const source = page
        .getByRole("region", { name: "Active sources", exact: true })
        .getByRole("listitem");
      await expect(source).toHaveCount(1);
      await expect(source).toContainText(
        "01a167d2-ee05-7360-b976-c4f6a10be332",
      );
    }
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(390);
  }
});
