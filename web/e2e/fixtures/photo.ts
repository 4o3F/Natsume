import type { Page, Route } from "@playwright/test";

import type {
  AccountResponse_Serialize as AccountResponse,
  PhotoRecord,
} from "../../src/api/generated/types";

export type Role = "admin" | "viewer";
type Handle = (route: Route, path: string) => Promise<boolean>;

export const ids = {
  alpha: "01912345-6789-7abc-8def-0123456789a1",
  beta: "01912345-6789-7abc-8def-0123456789a2",
  gamma: "01912345-6789-7abc-8def-0123456789a3",
  retired: "01912345-6789-7abc-8def-0123456789a4",
  importId: "01912345-6789-7abc-8def-0123456789b1",
};

function team(number: number, zh: string, en: string) {
  return {
    seat_id: `seat-${number}`,
    seat_code: `A-0${number}`,
    organization_id: "101",
    team_name_zh: zh,
    team_name_en: en,
    school_name_zh: "北京大学",
    school_name_en: "Peking University",
  };
}

// Three rostered teams plus one whose roster profile was removed.
export const accounts: AccountResponse[] = [
  {
    account_id: ids.alpha,
    domjudge_username: "team1",
    credential_revision: 1,
    team: team(1, "Alpha 队", "Alpha"),
  },
  {
    account_id: ids.beta,
    domjudge_username: "team2",
    credential_revision: 1,
    team: team(2, "Beta 队", "Beta"),
  },
  {
    account_id: ids.gamma,
    domjudge_username: "team3",
    credential_revision: 1,
    team: team(3, "Gamma 队", "Gamma"),
  },
  {
    account_id: ids.retired,
    domjudge_username: "team-old",
    credential_revision: 1,
    team: null,
  },
];

// Current photos for Alpha and Beta; Gamma has none.
export const photoRecords: PhotoRecord[] = [
  {
    account_id: ids.alpha,
    account: "team1",
    source_id: "src-2",
    submission_id: "sub-alpha",
    revision: 1,
    image_sha256: "a".repeat(64),
    image_format: "jpeg",
    image_bytes: 20480,
  },
  {
    account_id: ids.beta,
    account: "team2",
    source_id: "src-2",
    submission_id: "sub-beta",
    revision: 1,
    image_sha256: "b".repeat(64),
    image_format: "jpeg",
    image_bytes: 18432,
  },
];

// 1x1 PNG served for every current-photo thumbnail.
export const pngBytes = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=",
  "base64",
);

// Smallest valid ZIP: an empty end-of-central-directory record.
export const zipBytes = Buffer.from(`504b0506${"00".repeat(18)}`, "hex");

/**
 * Routes every `/api/v2` request through one mock. The session, roster,
 * current photo list and current-photo images are answered here; `handle`
 * runs first for everything else and resolves to true when it fulfilled the
 * request. Unhandled paths get a 404. Returns the `METHOD path` call log.
 */
export async function routeApi(page: Page, role: Role, handle: Handle) {
  const calls: string[] = [];
  await page.route("**/api/v2/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    calls.push(`${request.method()} ${path}`);
    if (path === "/api/v2/session")
      return route.fulfill({
        json: { operator_id: "operator", username: "operator", role },
      });
    if (await handle(route, path)) return;
    if (path === "/api/v2/accounts") return route.fulfill({ json: accounts });
    if (path === "/api/v2/photo/photos")
      return route.fulfill({ json: photoRecords });
    if (/^\/api\/v2\/photo\/photos\/[^/]+\/image$/.test(path))
      return route.fulfill({ contentType: "image/png", body: pngBytes });
    return route.fulfill({ status: 404, json: {} });
  });
  return calls;
}
