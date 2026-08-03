import { describe, expect, it } from "vitest";

import openapi from "../../openapi/natsume.openapi.json";

const ENROLLMENT_APPROVE_PATH =
  "/api/v2/enrollment-requests/{request_id}/actions/approve";
const COMMAND_PATH = "/api/v2/commands/{command_id}";
// INV-SECRET-01 forbids exposing secret material on any API surface. Public
// certificate material (CSR, leaf, chain, SPKI digest) and one-way hashes are
// not secrets and must stay representable. `\w*_` rather than `.*_` keeps the
// prefix inside one identifier, so route keys such as
// `/api/v2/devices/{device_id}/actions/sync-secret` are not misread as secrets.
const FORBIDDEN_CREDENTIAL_KEY =
  /^(?:(?:\w*_)?private_key(?:_\w*)?|(?:\w*_)?pass(?:word|phrase)(?:_(?:value|plaintext|material|secret))?|(?:\w*_)?token(?:_(?:value|plaintext|material|secret))?|(?:\w*_)?secret(?:_(?:value|plaintext|material|key))?)$/i;

const SECRET_KEY_SAMPLES = [
  "private_key",
  "gateway_private_key_der",
  "signing_private_key",
  "private_key_pem",
  "password",
  "domjudge_password",
  "password_value",
  "passphrase",
  "token",
  "device_token",
  "raw_token",
  "access_token",
  "enrollment_token",
  "api_token",
  "session_token",
  "auth_token",
  "token_value",
  "secret",
  "client_secret",
  "shared_secret",
  "secret_key",
];

const PUBLIC_KEY_SAMPLES = [
  "gateway_csr_der",
  "gateway_leaf_der",
  "gateway_chain_der",
  "gateway_spki_sha256",
  "token_hash",
  "password_hash",
  "device_token_id",
  "public_key",
  "serial",
  "not_after",
];

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

  it("contains the Rust-owned health operation", () => {
    expect(openapi.paths["/api/v2/health"].get.operationId).toBe("getHealth");
  });

  it("preserves imports and freezes the Panel-owned Command resource", () => {
    expect(openapi.paths["/api/v2/imports"].post.operationId).toBe(
      "createCsvImport",
    );
    expect(
      openapi.paths["/api/v2/imports/{import_id}/actions/commit"].post
        .operationId,
    ).toBe("commitCsvImport");

    const command = openapi.paths[COMMAND_PATH].put;
    expect(command.operationId).toBe("putCommand");
    expect(Object.keys(command.responses).sort()).toEqual([
      "200",
      "201",
      "400",
      "409",
    ]);
    expect(command.description).toContain(
      "canonical lowercase hyphenated UUIDv7",
    );
    expect(command.description).toContain("same normalized request");
    expect(command.description).toContain("conflicts");

    const commandId = command.parameters?.find(
      (parameter) => "name" in parameter && parameter.name === "command_id",
    );
    expect(commandId).toMatchObject({
      in: "path",
      required: true,
      schema: {
        format: "uuid",
        pattern:
          "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
      },
    });

    const requestSchema = openapi.components.schemas.PutCommandRequest as {
      oneOf: Array<{ $ref: string }>;
    };
    const schemas = openapi.components.schemas as Record<
      string,
      {
        additionalProperties?: boolean;
        properties?: Record<string, unknown>;
      }
    >;
    for (const branch of requestSchema.oneOf) {
      const schemaName = branch.$ref.split("/").at(-1);
      expect(schemaName).toBeDefined();
      const schema = schemas[schemaName ?? ""];
      expect(schema.additionalProperties).toBe(false);
      expect(Object.keys(schema.properties ?? {}).sort()).toEqual([
        "device_id",
        "group_correlation_id",
        "input",
        "input_version",
        "kind",
        "reason_code",
      ]);
    }

    expect(openapi.paths).not.toHaveProperty(
      "/api/v2/devices/{device_id}/actions/sync-state",
    );
    expect(openapi.paths).not.toHaveProperty(
      "/api/v2/devices/{device_id}/actions/sync-secret",
    );
    expect(JSON.stringify(openapi).toLowerCase()).not.toContain(
      "idempotency-key",
    );
  });

  it("contains no secret credential object keys", () => {
    const keys = collectObjectKeys(openapi);

    expect(keys.filter((key) => FORBIDDEN_CREDENTIAL_KEY.test(key))).toEqual(
      [],
    );
  });

  it("recognises secret material while permitting public certificate material", () => {
    expect(
      SECRET_KEY_SAMPLES.filter((key) => !FORBIDDEN_CREDENTIAL_KEY.test(key)),
    ).toEqual([]);
    expect(
      PUBLIC_KEY_SAMPLES.filter((key) => FORBIDDEN_CREDENTIAL_KEY.test(key)),
    ).toEqual([]);
  });
});
