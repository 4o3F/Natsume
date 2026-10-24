import { expect, test, type Page } from "@playwright/test";
import type {
  PrintExecution,
  PrintPrinter,
  PrintSource,
} from "../src/api/generated/types";

async function management(page: Page, role = "admin") {
  const calls: string[] = [];
  let printers: PrintPrinter[] = [
    {
      printer_id: "printer",
      name: "Adobe PDF",
      client_id: "client",
      client_confirmed: true,
      pairing_expires_at_unix_ms: null,
    },
  ];
  let sources: PrintSource[] = [
    { source_id: "source", name: "DOMjudge", revoked: false },
  ];
  await page.route("**/api/v2/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const method = request.method();
    calls.push(`${method} ${path}`);
    if (path === "/api/v2/session")
      return route.fulfill({
        json: { operator_id: "operator", username: "operator", role },
      });
    if (path === "/api/v2/print/printers") {
      if (method === "POST") {
        printers = [
          ...printers,
          {
            printer_id: "new",
            name: request.postDataJSON().name,
            client_id: null,
            client_confirmed: false,
            pairing_expires_at_unix_ms: null,
          },
        ];
        return route.fulfill({ json: printers[1] });
      }
      return route.fulfill({ json: printers });
    }
    if (path === "/api/v2/print/printers/printer/binding")
      return route.fulfill({
        status: 409,
        json: { title: "Conflict", status: 409, code: "RESOURCE_CONFLICT" },
      });
    if (path === "/api/v2/print/printers/new/pairing-code")
      return route.fulfill({
        json: {
          printer_id: "new",
          code: "ABCD-2345",
          expires_at_unix_ms: Date.now() + 600000,
        },
      });
    if (path === "/api/v2/print/sources") {
      if (method === "POST") {
        const source = {
          source_id: "new-source",
          name: request.postDataJSON().name,
          revoked: false,
        };
        sources = [...sources, source];
        return route.fulfill({
          json: { ...source, token: "test-only-source-secret" },
        });
      }
      return route.fulfill({ json: sources });
    }
    if (path === "/api/v2/print/sources/source" && method === "DELETE") {
      sources = sources.map((source) =>
        source.source_id === "source" ? { ...source, revoked: true } : source,
      );
      return route.fulfill({ status: 204 });
    }
    if (path === "/api/v2/print/queue") return route.fulfill({ json: [] });
    if (path === "/api/v2/print/executions") return route.fulfill({ json: [] });
    return route.fulfill({ status: 404, json: {} });
  });
  return calls;
}

test("printer registration, one-time code, and active-binding conflict", async ({
  page,
}) => {
  const calls = await management(page);
  await page.goto("/print/printers");
  await expect(
    page.getByRole("heading", { name: "Printers and pairing" }),
  ).toBeVisible();
  const bound = page.getByRole("row").filter({ hasText: "Adobe PDF" });
  await expect(bound).toContainText("Binding confirmed");
  await expect(
    bound.getByRole("button", { name: "Issue pairing code" }),
  ).toBeDisabled();
  await bound.getByRole("button", { name: "Clear binding" }).click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "Task history and attempt counts are not reset",
  );
  await page.getByRole("button", { name: "Confirm clear binding" }).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "Print state changed" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Keep binding" }).click();
  await page.getByLabel("Printer name", { exact: true }).fill("Second printer");
  await page.getByRole("button", { name: "Register printer" }).click();
  const added = page.getByRole("row").filter({ hasText: "Second printer" });
  await added.getByRole("button", { name: "Issue pairing code" }).click();
  await expect(page.getByText("ABCD-2345", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Dismiss secret" }).click();
  await expect(page.getByText("ABCD-2345", { exact: true })).toHaveCount(0);
  expect(
    calls.filter((call) => call === "POST /api/v2/print/printers"),
  ).toHaveLength(1);
});

test("source secrets are ephemeral and revocation requires confirmation", async ({
  page,
}) => {
  const calls = await management(page);
  await page.goto("/print/sources");
  await page.getByLabel("Source name", { exact: true }).fill("New judge");
  await page.getByRole("button", { name: "Create source" }).click();
  await expect(
    page.getByText("test-only-source-secret", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() =>
      JSON.stringify({ ...localStorage, ...sessionStorage }),
    ),
  ).not.toContain("test-only-source-secret");
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      value: undefined,
      configurable: true,
    }),
  );
  await page.getByRole("button", { name: "Copy secret" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "Copy unavailable" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Dismiss secret" }).click();
  await page.reload();
  await expect(page.getByText("New judge", { exact: true })).toBeVisible();
  await expect(
    page.getByText("test-only-source-secret", { exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("row")
    .filter({ hasText: "DOMjudge" })
    .getByRole("button", { name: "Revoke source" })
    .click();
  await expect(page.getByRole("alertdialog")).toContainText(
    "does not cancel printing",
  );
  expect(calls.filter((call) => call.startsWith("DELETE"))).toHaveLength(0);
  await page.getByRole("button", { name: "Confirm revoke source" }).click();
  await expect(
    page.getByRole("row").filter({ hasText: "DOMjudge" }),
  ).toContainText("Revoked");
});

for (const path of ["printers", "sources"])
  test(`viewer cannot access print ${path} management`, async ({ page }) => {
    const calls = await management(page, "viewer");
    await page.goto(`/print/${path}`);
    await expect(page).toHaveURL(/\/seats$/);
    expect(calls.some((call) => call.includes(`/print/${path}`))).toBe(false);
  });

test("execution states never turn missing data into queued and retain stale results", async ({
  page,
}) => {
  let unavailable = false;
  const executions: PrintExecution[] = [
    {
      ready_sequence: 1,
      state: "completed",
      attempts_used: 2,
      retry_at_unix_ms: null,
      last_outcome: "printed",
    },
    {
      ready_sequence: 2,
      state: "waiting",
      attempts_used: 1,
      retry_at_unix_ms: 30000,
      last_outcome: "unknown",
    },
  ];
  await page.route("**/api/v2/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/session"))
      return route.fulfill({
        json: { operator_id: "op", username: "op", role: "viewer" },
      });
    if (path.endsWith("/print/queue"))
      return route.fulfill({
        json: [1, 2, 3].map((id) => ({
          ready_sequence: id,
          source_id: "s",
          source_name: "Judge",
          request_id: `r${id}`,
          account: `team${id}`,
          team_name: `Team ${id}`,
          seat_label: null,
          original_filename: "main.cpp",
          pages: 1,
          bytes: 721,
        })),
      });
    if (path.endsWith("/print/executions"))
      return unavailable
        ? route.fulfill({
            status: 503,
            json: {
              title: "Unavailable",
              status: 503,
              code: "SERVICE_UNAVAILABLE",
            },
          })
        : route.fulfill({ json: executions });
    return route.fulfill({ status: 404, json: {} });
  });
  await page.goto("/print");
  await expect(
    page.getByText("0 ready to claim", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("row").filter({ hasText: "team1" }),
  ).toContainText("Spooler reported printed");
  await expect(
    page.getByRole("row").filter({ hasText: "team3" }),
  ).toContainText("State unavailable");
  await page
    .getByLabel("Execution state", { exact: true })
    .selectOption("waiting");
  await expect(page.locator("tbody tr")).toHaveCount(1);
  unavailable = true;
  await page.getByRole("button", { name: "Refresh queue" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Execution refresh failed",
  );
  await expect(page.locator("tbody tr")).toHaveCount(1);
  await expect(
    page.getByRole("row").filter({ hasText: "team2" }),
  ).toContainText("Retry waiting");
});
