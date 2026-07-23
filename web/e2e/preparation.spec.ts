import { expect, test } from "@playwright/test";

test("preparation center shell exposes the v2.5 boundaries", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Natsume V2 Preparation Center" }),
  ).toBeVisible();
  await expect(
    page.getByText("Single authoritative seat/account/password CSV"),
  ).toBeVisible();
  await expect(
    page.getByText("Device-only manual or policy-approved enrollment"),
  ).toBeVisible();
  await expect(
    page.getByText("Explicit state synchronization with Gateway PKI"),
  ).toBeVisible();
  await expect(
    page.getByText("Human-only secret synchronization"),
  ).toBeVisible();
  await expect(page.getByText("Desktop session lock / unlock")).toBeVisible();
});
