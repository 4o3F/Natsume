import { defineConfig, devices } from "@playwright/test";

const origin = process.env.NATSUME_OPERATOR_HTTPS_ORIGIN;
const pin = process.env.NATSUME_OPERATOR_HTTPS_SPKI;
const evidence = process.env.NATSUME_OPERATOR_HTTPS_EVIDENCE;
if (!origin || !pin || !evidence) {
  throw new Error(
    "Run pnpm --filter @natsume/web e2e:https to create the isolated Server fixture.",
  );
}

export default defineConfig({
  testDir: "./e2e",
  testMatch: "operator-https.spec.ts",
  workers: 1,
  retries: 0,
  timeout: 180_000,
  reporter: [["list"]],
  outputDir: `${evidence}/browser`,
  use: {
    ...devices["Desktop Chrome"],
    baseURL: origin,
    ignoreHTTPSErrors: false,
    launchOptions: { args: [`--ignore-certificate-errors-spki-list=${pin}`] },
    // Real credentials must not be recorded in network traces or failure videos.
    trace: "off",
    video: "off",
    screenshot: "off",
  },
});
