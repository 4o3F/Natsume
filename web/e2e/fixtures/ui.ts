import { expect, type Locator, type Page } from "@playwright/test";

export async function openNavigation(page: Page) {
  await expect(
    page
      .getByRole("dialog", { name: "Navigation", exact: true })
      .and(page.locator('[data-state="closed"]')),
  ).toHaveCount(0);
  const navigation = page.getByRole("navigation", {
    name: "Primary navigation",
    exact: true,
  });
  const opener = page.getByRole("button", {
    name: "Open navigation",
    exact: true,
  });
  await expect
    .poll(
      async () => (await navigation.isVisible()) || (await opener.isVisible()),
    )
    .toBe(true);
  if (!(await navigation.isVisible())) await opener.click();
  await expect(navigation).toBeVisible();
  return navigation;
}

export async function closeNavigation(page: Page) {
  const drawer = page.getByRole("dialog", { name: "Navigation", exact: true });
  if (await drawer.isVisible()) {
    await drawer.getByRole("button", { name: "Close", exact: true }).click();
    await expect(drawer).toHaveCount(0);
  }
}

export async function openAccountMenu(page: Page, username: string) {
  const trigger = page.getByRole("button", {
    name: `Account menu for ${username}`,
    exact: true,
  });
  await openNavigation(page);
  const menu = page.getByRole("menu");
  await expect(menu).toHaveCount(0);
  await trigger.click();
  await expect(menu).toBeVisible();
  return menu;
}

export function deviceRow(page: Page, hardwareId: string) {
  return page
    .getByRole("row")
    .filter({ has: page.getByTitle(hardwareId, { exact: true }) });
}

export async function openDeviceMenu(page: Page, hardwareId: string) {
  await closeDetails(page);
  const menu = page.getByRole("menu");
  await expect(menu).toHaveCount(0);
  await deviceRow(page, hardwareId)
    .getByRole("button", { name: /^Actions for / })
    .click();
  await expect(menu).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  return menu;
}

export async function closeDetails(page: Page) {
  const drawer = page.getByRole("dialog");
  if (await drawer.isVisible()) {
    await drawer.getByRole("button", { name: "Close", exact: true }).click();
    await expect(drawer).toHaveCount(0);
  }
}

export async function openDeviceDetails(page: Page, hardwareId: string) {
  await closeDetails(page);
  await deviceRow(page, hardwareId).click();
  const drawer = page.getByRole("dialog");
  await expect(drawer).toContainText(hardwareId);
  return drawer;
}

export async function seatCells(page: Page) {
  const table = page.getByRole("table").filter({
    has: page.getByRole("columnheader", { name: "Seat", exact: true }),
  });
  const index = await table
    .getByRole("columnheader", { name: "Seat", exact: true })
    .evaluate((element) => (element as HTMLTableCellElement).cellIndex + 1);
  return table.locator(`tbody > tr > td:nth-child(${index})`);
}

export function refreshLabel(timer: Locator) {
  return timer.locator("span").first();
}

export function refreshButton(page: Page, resource = "device") {
  return page
    .getByRole("timer", { name: `Next ${resource} refresh`, exact: true })
    .getByRole("button");
}

export function targetFact(page: Page, label: string) {
  return page
    .getByRole("dialog")
    .getByText(label, { exact: true })
    .locator("..")
    .locator("p")
    .last();
}
