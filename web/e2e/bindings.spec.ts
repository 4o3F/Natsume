import { expect, test, type Page } from "@playwright/test";

import type { components } from "../src/api/generated/schema";

const seats: components["schemas"]["SeatResponse"][] = [
  { seat_id: "01923456-789a-7bcd-8ef0-123456789a01", seat_code: "A-01" },
  { seat_id: "01923456-789a-7bcd-8ef0-123456789a02", seat_code: "A-2" },
  { seat_id: "01923456-789a-7bcd-8ef0-123456789a10", seat_code: "A-10" },
];
const bindings: components["schemas"]["BindingResponse"][] = seats.map(
  (seat, index) => ({
    binding_id: `01923456-789a-7bcd-8ef0-123456789b0${index}`,
    device_id: `01923456-789a-7bcd-8ef0-123456789c0${index}`,
    seat_id: seat.seat_id,
  }),
);

async function mockBindings(
  page: Page,
  { role = "admin", seatsFail = false } = {},
) {
  let currentBindings = [...bindings].reverse();
  const deleted: string[] = [];
  await page.route("**/api/v2/**", (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    let body: unknown;
    if (request.method() === "GET") {
      if (path === "/api/v2/session") {
        body = { operator_id: "01912345-6789-7abc-8def-0123456789ab", role };
      } else if (path === "/api/v2/bindings") {
        body = currentBindings;
      } else if (path === "/api/v2/seats") {
        if (seatsFail) {
          return route.fulfill({
            status: 500,
            contentType: "application/problem+json",
            body: JSON.stringify({
              code: "INTERNAL_ERROR",
              title: "Seats unavailable",
              status: 500,
            }),
          });
        }
        body = seats;
      }
    } else if (request.method() === "DELETE") {
      const binding = currentBindings.find(
        (binding) => path === `/api/v2/devices/${binding.device_id}/binding`,
      );
      if (binding) {
        deleted.push(binding.device_id);
        currentBindings = currentBindings.filter((row) => row !== binding);
        return route.fulfill({ status: 204 });
      }
    }
    return route.fulfill({
      status: body === undefined ? 404 : 200,
      contentType: "application/json",
      body: JSON.stringify(body ?? {}),
    });
  });
  return deleted;
}

for (const width of [1024, 1440]) {
  test(`bindings identify seats and unbind the selected device at ${width}`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    const deleted = await mockBindings(page);
    await page.goto("/bindings");
    const rows = page.locator("tbody tr");
    const seatCells = page.locator("tbody tr td:first-child");
    await expect(seatCells).toHaveText(["A-01", "A-2", "A-10"]);
    await expect(
      page.getByRole("columnheader", { name: "Seat ID" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("columnheader", { name: "Binding ID" }),
    ).toHaveCount(0);
    await expect(rows.locator("code")).toHaveText(
      bindings.map((binding) => binding.device_id),
    );
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    await page.screenshot({ path: testInfo.outputPath("bindings.png") });

    await page.getByRole("button", { name: "Seat", exact: true }).click();
    await expect(seatCells).toHaveText(["A-01", "A-2", "A-10"]);
    await page.getByRole("button", { name: "Seat", exact: true }).click();
    await expect(seatCells).toHaveText(["A-10", "A-2", "A-01"]);

    const search = page.getByRole("searchbox", { name: "Search seats" });
    await search.fill(" a-01 ");
    await expect(rows).toHaveCount(1);
    await rows.getByRole("button", { name: "Unbind" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toHaveAccessibleName("Unbind seat A-01?");
    await dialog.getByRole("button", { name: "Cancel" }).click();
    expect(deleted).toEqual([]);
    await rows.getByRole("button", { name: "Unbind" }).click();
    await dialog.getByRole("button", { name: "Unbind", exact: true }).click();
    await expect(page.getByText("No results.", { exact: true })).toBeVisible();
    expect(deleted).toEqual([bindings[0].device_id]);
    await search.clear();
    await expect(seatCells).toHaveText(["A-10", "A-2"]);
  });
}

test("non-admin operators can find bound seats without unbind controls", async ({
  page,
}) => {
  await mockBindings(page, { role: "viewer" });
  await page.goto("/bindings");
  await expect(
    page.getByRole("cell", { name: "A-01", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Unbind" })).toHaveCount(0);
  await page.getByRole("searchbox", { name: "Search seats" }).fill("A-10");
  await expect(page.locator("tbody tr")).toHaveCount(1);
});

test("seat lookup failure is visible before offering unbind controls", async ({
  page,
}) => {
  await mockBindings(page, { seatsFail: true });
  await page.goto("/bindings");
  await expect(
    page.getByText("Seats unavailable", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Unbind" })).toHaveCount(0);
});
