import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, expect, it } from "vitest";

const directory = mkdtempSync(join(tmpdir(), "natsume-openapi-lint-"));
const path = join(directory, "openapi.json");
const script = fileURLToPath(new URL("../../openapi/lint.ts", import.meta.url));
const document = {
  openapi: "3.1.0",
  info: { title: "Test API", version: "1.0.0", description: "Test API" },
  paths: {
    "/test": {
      get: {
        operationId: "getTest",
        summary: "Get test",
        responses: { "200": { description: "Success" } },
      },
    },
  },
};

afterEach(() => rmSync(path, { force: true }));
afterAll(() => rmSync(directory, { recursive: true, force: true }));

function lint(source: string) {
  writeFileSync(path, source);
  return spawnSync(process.execPath, [script, path], { encoding: "utf8" });
}

it("retains the existing rule exclusions for a valid OpenAPI document", () => {
  const result = lint(JSON.stringify(document));
  expect(result.status, result.stderr).toBe(0);
  expect(result.stdout).toBe("");
});

it("fails on warnings, not only errors", () => {
  const invalid = structuredClone(document);
  invalid.info.description = "";
  const result = lint(JSON.stringify(invalid));
  expect(result.status, result.stderr).toBe(1);
  expect(result.stdout).toContain("info-description");
});

it("rejects invalid OpenAPI schemas", () => {
  const result = lint(JSON.stringify({ ...document, openapi: "3.1.invalid" }));
  expect(result.status, result.stderr).toBe(1);
  expect(result.stdout).toContain("oas3-schema");
});

it("rejects malformed JSON", () => {
  const result = lint("{ invalid JSON }");
  expect(result.status).toBe(1);
  expect(result.stdout).toContain("parser");
});
