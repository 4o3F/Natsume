import { describe, expect, it } from "vitest";

import openapi from "../../openapi/natsume.openapi.json";

const ENROLLMENT_APPROVE_PATH =
  "/api/v2/enrollment-requests/{request_id}:approve";
const FORBIDDEN_CREDENTIAL_KEY =
  /^(gateway_(csr|spki|certificate|cert|leaf|chain|private_key)|install_certificate|certificate_issue_request)$/i;

function collectObjectKeys(value: unknown, output: string[] = []): string[] {
  if (Array.isArray(value)) {
    for (const item of value) {
      collectObjectKeys(item, output);
    }
  } else if (value !== null && typeof value === "object") {
    for (const [key, child] of Object.entries(value)) {
      output.push(key.replaceAll("-", "_"));
      collectObjectKeys(child, output);
    }
  }

  return output;
}

describe("Phase 0 Enrollment OpenAPI contract", () => {
  it("contains the Device Enrollment approval operation", () => {
    const enrollment = openapi.paths[ENROLLMENT_APPROVE_PATH];

    expect(enrollment.post.operationId).toBe("approveEnrollment");
    expect(enrollment.post.summary.toLowerCase()).toContain("device");
  });

  it("contains no Gateway credential object keys", () => {
    const keys = collectObjectKeys(openapi.paths[ENROLLMENT_APPROVE_PATH]);
    const forbidden = keys.filter((key) => FORBIDDEN_CREDENTIAL_KEY.test(key));

    expect(forbidden).toEqual([]);
  });
});
