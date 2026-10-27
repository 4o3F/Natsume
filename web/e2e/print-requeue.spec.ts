import { expect, test, type Page, type Route } from "@playwright/test";
import { closeDetails } from "./fixtures/ui";
import type {
  PrintExecution,
  PrintQueueEntry,
} from "../src/api/generated/types";

const entry: PrintQueueEntry = {
  ready_sequence: 17,
  source_id: "01912345-6789-7abc-8def-0123456789ab",
  source_name: "Demo judge",
  request_id: "01912345-6789-7abc-8def-0123456789ac",
  account: "team-1",
  team_name: "Team One",
  seat_label: "A-01",
  original_filename: "main.cpp",
  pages: 2,
  bytes: 1024,
};
const initial: PrintExecution = {
  ready_sequence: 17,
  state: "exhausted",
  attempts_used: 3,
  requeue_count: 0,
  retry_at_unix_ms: null,
  last_outcome: "unknown",
};

async function mockRequeue(page: Page, role = "admin") {
  const data: {
    execution: PrintExecution | null;
    writes: unknown[];
    submit: (route: Route) => Promise<void>;
  } = {
    execution: { ...initial },
    writes: [],
    submit: async (route) => {
      data.execution = {
        ...initial,
        state: "queued",
        attempts_used: 0,
        requeue_count: (data.execution?.requeue_count ?? 0) + 1,
      };
      await route.fulfill({ status: 204 });
    },
  };
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/v2/session")
      return route.fulfill({
        json: { operator_id: "operator", username: "operator", role },
      });
    if (path === "/api/v2/print/queue") return route.fulfill({ json: [entry] });
    if (path === "/api/v2/print/executions")
      return route.fulfill({ json: data.execution ? [data.execution] : [] });
    if (path === "/api/v2/print/executions/17/requeue") {
      expect(route.request().method()).toBe("POST");
      data.writes.push(route.request().postDataJSON());
      return data.submit(route);
    }
    return route.fulfill({ status: 404, json: {} });
  });
  return data;
}

async function openDetails(page: Page) {
  await page.goto("/print");
  await page
    .getByRole("table", { name: "Admitted documents", exact: true })
    .getByRole("row")
    .filter({ hasText: "main.cpp" })
    .click();
  return page.getByRole("dialog");
}

for (const role of ["admin", "viewer"]) {
  test(`${role} only receives the authorized exhausted-document action`, async ({
    page,
  }) => {
    const data = await mockRequeue(page, role);
    for (const state of [
      "queued",
      "active",
      "waiting",
      "completed",
      "exhausted",
      null,
    ] as const) {
      data.execution = state ? { ...initial, state } : null;
      const details = await openDetails(page);
      await expect(
        details.getByRole("button", { name: "Requeue document", exact: true }),
      ).toHaveCount(role === "admin" && state === "exhausted" ? 1 : 0);
    }
    expect(data.writes).toEqual([]);
  });
}

for (const width of [390, 1440]) {
  test(`one explicit confirmation grants a round without duplicating admission at ${width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    const data = await mockRequeue(page);
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const submit = data.submit;
    data.submit = async (route) => {
      await held;
      await submit(route);
    };
    const details = await openDetails(page);
    const opener = details.getByRole("button", {
      name: "Requeue document",
      exact: true,
    });
    await opener.click();
    const confirmation = page.getByRole("alertdialog");
    const confirm = confirmation.getByRole("button", {
      name: "Requeue document",
      exact: true,
    });
    const acknowledgement = confirmation.getByRole("checkbox", {
      name: /accept the risk of duplicate printing/,
    });
    await expect(confirmation).toContainText("up to 3 new attempts");
    await expect(confirmation).toContainText("duplicate copies");
    await expect(confirm).toBeDisabled();
    await acknowledgement.check();
    await confirmation
      .getByRole("button", { name: "Cancel", exact: true })
      .click();
    await expect(confirmation).toHaveCount(0);
    expect(data.writes).toEqual([]);
    await expect(opener).toBeFocused();
    await opener.click();
    await expect(acknowledgement).not.toBeChecked();
    await acknowledgement.check();
    await page.screenshot({
      path: testInfo.outputPath("requeue-confirmation.png"),
    });
    await confirm.click();
    await expect.poll(() => data.writes.length).toBe(1);
    await expect(confirm).toBeDisabled();
    await expect(
      confirmation.getByRole("button", { name: "Cancel", exact: true }),
    ).toBeDisabled();
    await confirm.evaluate((button) => (button as HTMLButtonElement).click());
    expect(data.writes).toEqual([{ expected_requeue_count: 0 }]);
    release();
    await expect(confirmation).toHaveCount(0);
    await expect(
      details.getByText("Ready to claim", { exact: true }),
    ).toBeVisible();
    await expect(
      details
        .getByText("Attempt round", { exact: true })
        .locator("..")
        .getByRole("definition"),
    ).toHaveText("2");
    await expect(
      details
        .getByText("Attempts used (this round)", { exact: true })
        .locator("..")
        .getByRole("definition"),
    ).toHaveText("0 of 3");
    await expect(opener).toHaveCount(0);
    await closeDetails(page);
    await expect(
      page
        .getByRole("table", { name: "Admitted documents", exact: true })
        .locator("tbody > tr"),
    ).toHaveCount(1);
    expect(data.writes).toHaveLength(1);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
  });
}

test("a lost response refreshes the execution without silently submitting another round", async ({
  page,
}) => {
  const data = await mockRequeue(page);
  data.submit = async (route) => {
    data.execution = {
      ...initial,
      state: "queued",
      attempts_used: 0,
      requeue_count: 1,
    };
    await route.abort("failed");
  };
  const details = await openDetails(page);
  await details
    .getByRole("button", { name: "Requeue document", exact: true })
    .click();
  const confirmation = page.getByRole("alertdialog");
  await confirmation.getByRole("checkbox").check();
  await confirmation
    .getByRole("button", { name: "Requeue document", exact: true })
    .click();
  await expect(confirmation).toContainText("Refresh and verify the result");
  await expect(confirmation).toContainText("Print state changed");
  await expect(
    confirmation.getByRole("button", { name: "Requeue document", exact: true }),
  ).toBeDisabled();
  expect(data.writes).toEqual([{ expected_requeue_count: 0 }]);
  await confirmation
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await expect(
    details.getByText("Ready to claim", { exact: true }),
  ).toBeVisible();
});

test("a conflict refreshes the round and requires a new confirmation", async ({
  page,
}) => {
  const data = await mockRequeue(page);
  const submit = data.submit;
  data.submit = async (route) => {
    data.execution = { ...initial, requeue_count: 1 };
    await route.fulfill({
      status: 409,
      json: { title: "Conflict", status: 409, code: "CONFLICT" },
    });
  };
  const details = await openDetails(page);
  const opener = details.getByRole("button", {
    name: "Requeue document",
    exact: true,
  });
  await opener.click();
  const confirmation = page.getByRole("alertdialog");
  await confirmation.getByRole("checkbox").check();
  await confirmation
    .getByRole("button", { name: "Requeue document", exact: true })
    .click();
  await expect(confirmation).toContainText("Print state changed");
  await expect(
    confirmation.getByRole("button", { name: "Requeue document", exact: true }),
  ).toBeDisabled();
  await confirmation
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  data.submit = submit;
  await opener.click();
  await expect(confirmation.getByRole("checkbox")).not.toBeChecked();
  await confirmation.getByRole("checkbox").check();
  await confirmation
    .getByRole("button", { name: "Requeue document", exact: true })
    .click();
  await expect(confirmation).toHaveCount(0);
  expect(data.writes).toEqual([
    { expected_requeue_count: 0 },
    { expected_requeue_count: 1 },
  ]);
});

test("a poll changing the manual round invalidates an open confirmation", async ({
  page,
}) => {
  const data = await mockRequeue(page);
  await page.clock.install();
  const details = await openDetails(page);
  await details
    .getByRole("button", { name: "Requeue document", exact: true })
    .click();
  const confirmation = page.getByRole("alertdialog");
  await confirmation.getByRole("checkbox").check();
  data.execution = { ...initial, requeue_count: 1 };
  await page.clock.runFor(10_000);
  await expect(confirmation).toContainText("Print state changed");
  await expect(
    confirmation.getByRole("button", { name: "Requeue document", exact: true }),
  ).toBeDisabled();
  expect(data.writes).toEqual([]);
});
