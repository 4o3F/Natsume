// Opt-in manual capture: pnpm --dir web build && node web/scripts/capture-gallery.mjs
// Uses only built production assets and synthetic API/media fixtures; never a Server.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { chromium, expect } from "@playwright/test";
import { preview } from "vite";

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const screenshots = new URL("../../docs/screenshots/", import.meta.url);
const now = Date.parse("2026-10-19T09:30:00Z");
const uuid = (group, index) =>
  `019a1234-5678-7abc-8def-${group}${String(index).padStart(8, "0")}`;
const hardware = (index) =>
  `${String(index).padStart(8, "0")}-1234-5abc-8def-0123456789ab`;
/** @type {import('../src/api/generated/types.js').SessionResponse} */
const operator = {
  operator_id: uuid("0001", 1),
  role: "admin",
  username: "ops-admin",
};
/** @type {import('../src/api/generated/types.js').OrganizationLogoResponse[]} */
const schools = [
  ["INST-001", "星河大学", "Star River University"],
  ["INST-002", "青松理工学院", "Pine Institute of Technology"],
  ["INST-003", "海湾大学", "Harbor University"],
].map(([organization_id, name_zh, name_en]) => ({
  organization_id,
  name_zh,
  name_en,
  country: "CHN",
  status: "available",
  files: [`${organization_id}.png`],
  detail: null,
}));
// Three small synthetic PNG school-logo fixtures.
const schoolLogos = [
  "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",
];
/** @type {import('../src/api/generated/types.js').SeatResponse[]} */
const seats = Array.from({ length: 5 }, (_, i) => ({
  seat_id: uuid("0002", i + 1),
  seat_code: `A-${String(i + 1).padStart(2, "0")}`,
}));
/** @type {import('../src/api/generated/types.js').AccountResponse[]} */
const accounts = [
  ["星河漫游", "Star Voyagers"],
  ["松间算法", "Pine Algorithms"],
  ["潮汐", "Tidal Code"],
  ["银河快线", "Galaxy Express"],
  ["常青树", "Evergreen"],
].map(([team_name_zh, team_name_en], i) => {
  const school = schools[i % schools.length];
  return {
    account_id: uuid("0003", i + 1),
    domjudge_username: `team${String(i + 1).padStart(3, "0")}`,
    credential_revision: 1,
    team: {
      ...seats[i],
      organization_id: school.organization_id,
      team_name_zh,
      team_name_en,
      school_name_zh: school.name_zh,
      school_name_en: school.name_en,
    },
  };
});
/** @type {import('../src/api/generated/types.js').BindingResponse[]} */
const bindings = seats.slice(0, 4).map((seat, i) => ({
  binding_id: uuid("0004", i + 1),
  seat_id: seat.seat_id,
  device_id: uuid("0005", i + 1),
}));
/** @type {import('../src/api/generated/types.js').DeviceResponse_Serialize[]} */
const devices = Array.from({ length: 5 }, (_, i) => {
  const offline = i === 3;
  const spare = i === 4;
  const context = {
    binding_id: uuid("0004", i + 1),
    account_id: accounts[i].account_id,
    credential_revision: 1,
    domjudge_username: accounts[i].domjudge_username,
    seat_code: seats[i].seat_code,
  };
  const foreground = i === 2 || spare ? "waiting" : "contest";
  return {
    device_id: uuid("0005", i + 1),
    machine_hardware_id: hardware(i + 1),
    evidence_quality: "strong",
    state: spare ? "disabled" : "enabled",
    created_at_unix_ms: now - 86_400_000,
    network: {
      client_ip: `192.0.2.${i + 11}`,
      server_observed_ip: `192.0.2.${i + 11}`,
      observed_at_unix_ms: now - (offline ? 120_000 : 2_000),
    },
    convergence: {
      connection_state: offline || spare ? "offline" : "active",
      received_at_unix_ms: now - (offline || spare ? 120_000 : 2_000),
      binding: {
        status: offline || spare ? "awaiting_actual" : "converged",
        target: spare ? null : { state: "bound", context },
        actual:
          offline || spare
            ? null
            : {
                assignment_state: "applied",
                credential_state: "applied",
                context,
              },
      },
      gateway: {
        status: offline || spare ? "awaiting_actual" : "converged",
        target: spare
          ? null
          : {
              credential_id: uuid("0006", i + 1),
              gateway_leaf_sha256: "a".repeat(64),
            },
        actual:
          offline || spare
            ? null
            : {
                credential_id: uuid("0006", i + 1),
                gateway_leaf_sha256: "a".repeat(64),
                state: "ready",
              },
      },
      runtime_config: {
        status: offline || spare ? "awaiting_actual" : "converged",
        target_domjudge_origin: spare ? null : "https://judge.example.test",
        actual:
          offline || spare
            ? null
            : {
                state: "applied",
                applied_domjudge_origin: "https://judge.example.test",
              },
      },
      session_control: {
        status: offline || spare ? "awaiting_actual" : "converged",
        target: spare
          ? null
          : { foreground_target: foreground, terminate_epoch: null },
        actual:
          offline || spare
            ? null
            : {
                session_state: "running",
                completed_terminate_epoch: null,
                foreground,
                waiting_ready: true,
                contest_ready: true,
              },
      },
      home: {
        status: offline || spare ? "awaiting_actual" : "converged",
        target_reset_epoch: null,
        actual:
          offline || spare
            ? null
            : { state: "steady", completed_reset_epoch: null },
      },
    },
  };
});
/** @type {import('../src/api/generated/types.js').EnrollmentReviewResponse[]} */
const reviews = [6, 7].map((i) => ({
  review_id: uuid("0007", i),
  machine_hardware_id: hardware(i),
  candidate_public_key: Buffer.alloc(32, i).toString("base64url"),
  evidence_quality: "strong",
  daemon_version: "2.10.3",
  agent_version: "2.10.3",
}));
/** @type {import('../src/api/generated/types.js').OperatorAccountResponse[]} */
const users = [
  { ...operator, role: "admin" },
  { operator_id: uuid("0001", 2), role: "admin", username: "floor-admin" },
  { operator_id: uuid("0001", 3), role: "viewer", username: "helpdesk" },
  { operator_id: uuid("0001", 4), role: "viewer", username: "scoreboard" },
];
/** @type {import('../src/api/generated/types.js').OperatorInvitationResponse[]} */
const invitations = [
  {
    invitation_id: uuid("0008", 1),
    issuer_operator_id: operator.operator_id,
    role: "viewer",
    created_at_unix_ms: now - 1_800_000,
    expires_at_unix_ms: now - 1_800_000 + 7 * 86_400_000,
    expired: false,
  },
];
const responses = {
  "/api/v2/session": operator,
  "/api/v2/accounts": accounts,
  "/api/v2/seats": seats,
  "/api/v2/bindings": bindings,
  "/api/v2/imports": { pending: null },
  "/api/v2/organizations": schools,
  "/api/v2/enrollment-reviews": reviews,
  "/api/v2/provisioning-window": { state: "closed" },
  "/api/v2/operator/accounts": users,
  "/api/v2/operator/invitations": invitations,
};
const scenes = [
  [
    "preparation",
    "Preparation",
    "Preparation Center",
    "Upload complete roster",
  ],
  ["enrollment", "Enrollment", "Enrollment Reviews", hardware(6)],
  ["accounts", "Accounts", null, "Star Voyagers"],
  ["seats", "Seats", "Seats", "Star Voyagers"],
  ["devices", "Devices", "Devices", "00000001…"],
  ["targets", "Targets", "Targets", "Manage Session and Home targets"],
  ["users", "Users", "Users", "helpdesk"],
];

const server = await preview({
  root: webRoot,
  preview: { host: "127.0.0.1", port: 0, strictPort: true, proxy: {} },
});
let browser;
const failures = [];
const requests = new Set();
try {
  const address = server.httpServer.address();
  assert(address && typeof address !== "string");
  const origin = `http://127.0.0.1:${address.port}`;
  browser = await chromium.launch();
  console.log(
    JSON.stringify({
      origin,
      browser: browser.version(),
      executable: chromium.executablePath(),
      syntheticTime: new Date(now).toISOString(),
    }),
  );
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1080 },
    deviceScaleFactor: 2,
    locale: "en-US",
    timezoneId: "UTC",
    colorScheme: "light",
    serviceWorkers: "block",
  });
  await context.route("**/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (url.origin !== origin || request.method() !== "GET") {
      failures.push(`Blocked unexpected request: ${request.method()} ${url}`);
      return route.abort("blockedbyclient");
    }
    if (!path.startsWith("/api/")) return route.continue();
    requests.add(`${request.method()} ${path}${url.search}`);
    if (path === "/api/v2/devices") {
      assert.deepEqual([...url.searchParams.keys()], ["state"]);
      const states = url.searchParams.get("state").split(",");
      assert(
        states.every((state) =>
          ["enabled", "disabled", "revoked"].includes(state),
        ),
      );
      return route.fulfill({
        json: devices.filter((device) => states.includes(device.state)),
      });
    }
    const logo = schools.find(
      (school) =>
        path === `/api/v2/organizations/${school.organization_id}/logo`,
    );
    if (logo) {
      // Synthetic PNG school monograms are media fixtures, not recreated UI.
      return route.fulfill({
        contentType: "image/png",
        body: Buffer.from(schoolLogos[schools.indexOf(logo)], "base64"),
      });
    }
    if (!url.search && Object.hasOwn(responses, path))
      return route.fulfill({ json: responses[path] });
    failures.push(`Blocked unexpected API: ${url}`);
    return route.abort("blockedbyclient");
  });
  const page = await context.newPage();
  page.on("pageerror", (error) =>
    failures.push(`Page error: ${error.message}`),
  );
  page.on("console", (message) => {
    if (message.type() === "error")
      failures.push(`Console error: ${message.text()}`);
  });
  page.on("requestfailed", (request) =>
    failures.push(
      `Failed request: ${request.url()}: ${request.failure()?.errorText}`,
    ),
  );
  page.on("response", (response) => {
    if (!response.ok())
      failures.push(`HTTP ${response.status()}: ${response.url()}`);
  });
  await page.clock.setFixedTime(new Date(now));
  for (const [name, nav, heading, content] of scenes) {
    await page.goto(`${origin}/${name}`);
    await expect(
      page.getByRole("link", { name: nav, exact: true }),
    ).toHaveAttribute("aria-current", "page");
    if (heading) {
      await expect(
        page.getByRole("heading", { name: heading, exact: true }),
      ).toBeVisible();
    } else {
      // The production Accounts page uses table headings, not a page h1.
      await expect(
        page.getByRole("columnheader", {
          name: "DOMjudge username",
          exact: true,
        }),
      ).toBeVisible();
    }
    await expect(
      page
        .getByRole("navigation", { name: "Primary navigation" })
        .getByRole("link", { name: "Bindings", exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByText(content, { exact: false }).first(),
    ).toBeVisible();
    await expect(page.locator("tbody tr")).toHaveCount(
      name === "preparation" ? 3 : name === "enrollment" ? 2 : 5,
    );
    await page.waitForLoadState("networkidle");
    await page.evaluate(async () => {
      await document.fonts.ready;
      await Promise.all(Array.from(document.images, (image) => image.decode()));
    });
    await expect(
      page.locator(
        '[role="alert"], [role="alertdialog"], [data-slot="skeleton"], .animate-pulse, .animate-spin',
      ),
    ).toHaveCount(0);
    await expect(
      page.getByText(
        /^(Loading\.\.\.|Refreshing\. Showing previous result\.)$/,
      ),
    ).toHaveCount(0);
    if (name === "devices") {
      await expect(
        page.getByRole("columnheader", { name: "Seat", exact: true }),
      ).toBeVisible();
      await expect(
        page.getByRole("button", { name: "Unbind", exact: true }).first(),
      ).toBeVisible();
      for (const facet of ["Lifecycle", "Connection", "Binding"]) {
        await expect(
          page.getByRole("button", { name: new RegExp(`^${facet}`) }),
        ).toBeVisible();
      }
      await expect(
        page.getByRole("button", { name: /Emergency/ }),
      ).toBeVisible();
    }
    if (name === "users") {
      await expect(page.locator('input[type="password"]')).toHaveCount(0);
      assert(
        !/invite_[a-z0-9]{64}|reset_[a-z0-9]{64}/i.test(
          await page.locator("body").innerText(),
        ),
      );
    }
    const geometry = await page.evaluate(() => ({
      width: innerWidth,
      height: innerHeight,
      dpr: devicePixelRatio,
      documentWidth: document.documentElement.scrollWidth,
      scrollY,
    }));
    assert.deepEqual(geometry, {
      width: 1440,
      height: 1080,
      dpr: 2,
      documentWidth: 1440,
      scrollY: 0,
    });
    assert.deepEqual(failures, []);
    const png = await page.screenshot({
      path: fileURLToPath(new URL(`${name}.png`, screenshots)),
      fullPage: false,
    });
    assert.equal(png.readUInt32BE(16), 2880);
    assert.equal(png.readUInt32BE(20), 2160);
    console.log(JSON.stringify({ scene: name, geometry, png: [2880, 2160] }));
  }
  console.log(
    JSON.stringify(
      {
        unexpectedRequestsAndErrors: failures,
        interceptedRequests: [...requests].sort(),
      },
      null,
      2,
    ),
  );
  await context.close();
} finally {
  await browser?.close();
  await new Promise((resolve, reject) =>
    server.httpServer.close((error) => (error ? reject(error) : resolve())),
  );
  console.log("Closed owned preview and browser.");
}
