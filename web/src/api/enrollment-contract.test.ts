import { describe, expect, it } from "vitest";

import openapi from "../../openapi/natsume.openapi.json";

const ENROLLMENT_APPROVE_PATH =
  "/api/v2/enrollment-requests/{request_id}/actions/approve";
const COMMAND_PATH = "/api/v2/commands/{command_id}";
const SESSION_REQUEST_PASSWORD_PATH =
  "/components/schemas/SessionRequest/properties/password";
const IMPORT_COMMIT_TOKEN_PATH =
  "/components/schemas/ImportCommitRequest/properties/preview_token";
const IMPORT_PREVIEW_TOKEN_PATH =
  "/components/schemas/ImportPreviewResponse/properties/preview_token";
const SESSION_REQUEST_REFERENCE = "#/components/schemas/SessionRequest";
const CANONICAL_UUID_V7_REFERENCE = "#/components/schemas/CanonicalUuidV7";
const UUID_V7_PATTERN =
  "^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
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

interface ObjectKeyOccurrence {
  key: string;
  path: string;
}

function escapeJsonPointer(value: string): string {
  return value.replaceAll("~", "~0").replaceAll("/", "~1");
}

function collectObjectKeys(
  value: unknown,
  path = "",
  output: ObjectKeyOccurrence[] = [],
): ObjectKeyOccurrence[] {
  if (Array.isArray(value)) {
    for (const [index, item] of value.entries()) {
      collectObjectKeys(item, `${path}/${index}`, output);
    }
  } else if (value !== null && typeof value === "object") {
    for (const [key, child] of Object.entries(value)) {
      const childPath = `${path}/${escapeJsonPointer(key)}`;
      output.push({ key: key.replaceAll("-", "_"), path: childPath });
      collectObjectKeys(child, childPath, output);
    }
  }

  return output;
}

function collectStringPaths(
  value: unknown,
  expected: string,
  path = "",
  output: string[] = [],
): string[] {
  if (Array.isArray(value)) {
    for (const [index, item] of value.entries()) {
      collectStringPaths(item, expected, `${path}/${index}`, output);
    }
  } else if (value !== null && typeof value === "object") {
    for (const [key, child] of Object.entries(value)) {
      collectStringPaths(
        child,
        expected,
        `${path}/${escapeJsonPointer(key)}`,
        output,
      );
    }
  } else if (value === expected) {
    output.push(path);
  }

  return output;
}

function expectResponseSet(
  operation: { responses: Record<string, unknown> },
  expected: string[],
): void {
  expect(Object.keys(operation.responses).sort()).toEqual(expected);
}

describe("Natsume V2 browser OpenAPI contract", () => {
  it("contains the Device Enrollment approval operation", () => {
    const enrollment = openapi.paths[ENROLLMENT_APPROVE_PATH];

    expect(enrollment.post.operationId).toBe("approveEnrollment");
    expect(enrollment.post.summary.toLowerCase()).toContain("device");
  });

  it("contains the Rust-owned health operation", () => {
    expect(openapi.paths["/api/v2/health"].get.operationId).toBe("getHealth");
  });

  it("contains exactly the mounted and declared-but-unmounted frozen surface", () => {
    expect(Object.keys(openapi.paths).sort()).toEqual([
      "/api/v2/accounts",
      "/api/v2/bindings",
      "/api/v2/commands/{command_id}",
      "/api/v2/devices",
      "/api/v2/devices/{device_id}/actions/disable",
      "/api/v2/devices/{device_id}/actions/revoke",
      "/api/v2/enrollment-requests/{request_id}/actions/approve",
      "/api/v2/health",
      "/api/v2/imports",
      "/api/v2/imports/{import_id}/actions/commit",
      "/api/v2/imports/{import_id}/actions/discard",
      "/api/v2/seats",
      "/api/v2/session",
    ]);

    expect(openapi.paths["/api/v2/health"].get.operationId).toBe("getHealth");
    expect(openapi.paths["/api/v2/session"].post.operationId).toBe(
      "createSession",
    );
    expect(openapi.paths["/api/v2/session"].get.operationId).toBe("getSession");
    expect(openapi.paths["/api/v2/session"].delete.operationId).toBe(
      "deleteSession",
    );
    expect(openapi.paths["/api/v2/seats"].get.operationId).toBe("listSeats");
    expect(openapi.paths["/api/v2/accounts"].get.operationId).toBe(
      "listAccounts",
    );
    expect(openapi.paths["/api/v2/devices"].get.operationId).toBe(
      "listDevices",
    );
    expect(openapi.paths["/api/v2/bindings"].get.operationId).toBe(
      "listBindings",
    );
    expect(
      openapi.paths["/api/v2/devices/{device_id}/actions/revoke"].post
        .operationId,
    ).toBe("revokeDevice");
    expect(
      openapi.paths["/api/v2/devices/{device_id}/actions/disable"].post
        .operationId,
    ).toBe("disableDevice");

    expect(openapi.paths["/api/v2/imports"].get.operationId).toBe(
      "getCsvImport",
    );
    expect(openapi.paths["/api/v2/imports"].post.operationId).toBe(
      "createCsvImport",
    );
    expect(
      openapi.paths["/api/v2/imports/{import_id}/actions/commit"].post
        .operationId,
    ).toBe("commitCsvImport");
    expect(
      openapi.paths["/api/v2/imports/{import_id}/actions/discard"].post
        .operationId,
    ).toBe("discardCsvImport");
    expect(openapi.paths[ENROLLMENT_APPROVE_PATH].post.operationId).toBe(
      "approveEnrollment",
    );
    expect(openapi.paths[COMMAND_PATH].put.operationId).toBe("putCommand");
  });

  it("freezes the operator response sets, path IDs, and redacted DTOs", () => {
    for (const path of [
      "/api/v2/seats",
      "/api/v2/accounts",
      "/api/v2/devices",
      "/api/v2/bindings",
    ] as const) {
      expectResponseSet(openapi.paths[path].get, ["200", "401", "500"]);
    }

    for (const path of [
      "/api/v2/devices/{device_id}/actions/revoke",
      "/api/v2/devices/{device_id}/actions/disable",
    ] as const) {
      const operation = openapi.paths[path].post;
      expectResponseSet(operation, ["200", "400", "401", "403", "404", "500"]);
      const deviceId = operation.parameters?.find(
        (parameter) => "name" in parameter && parameter.name === "device_id",
      );
      expect(deviceId).toMatchObject({
        in: "path",
        required: true,
        schema: { $ref: CANONICAL_UUID_V7_REFERENCE },
      });
    }

    expect(openapi.components.schemas.CanonicalUuidV7).toEqual({
      type: "string",
      format: "uuid",
      pattern: UUID_V7_PATTERN,
    });

    expect(
      Object.keys(openapi.components.schemas.SessionResponse.properties).sort(),
    ).toEqual(["operator_id", "role"]);
    expect(
      openapi.components.schemas.SessionResponse.additionalProperties,
    ).toBe(false);
    expect(
      openapi.components.schemas.DeviceResponse.properties,
    ).not.toHaveProperty("machine_hardware_id");
    expect(
      openapi.components.schemas.AccountResponse.properties,
    ).not.toHaveProperty("credential_vault_record_id");
  });

  it("freezes the mounted import and Panel-owned Command resources", () => {
    expectResponseSet(openapi.paths["/api/v2/imports"].get, [
      "200",
      "401",
      "403",
      "500",
    ]);
    expectResponseSet(openapi.paths["/api/v2/imports"].post, [
      "201",
      "400",
      "401",
      "403",
      "409",
      "413",
      "500",
    ]);
    expectResponseSet(
      openapi.paths["/api/v2/imports/{import_id}/actions/commit"].post,
      ["200", "400", "401", "403", "404", "409", "413", "500"],
    );
    expectResponseSet(
      openapi.paths["/api/v2/imports/{import_id}/actions/discard"].post,
      ["204", "400", "401", "403", "404", "500"],
    );

    const command = openapi.paths[COMMAND_PATH].put;
    expect(command.operationId).toBe("putCommand");
    expectResponseSet(command, [
      "200",
      "201",
      "400",
      "401",
      "403",
      "404",
      "409",
      "500",
    ]);
    expect(command.description).toContain(
      "canonical lowercase hyphenated UUIDv7",
    );
    expect(command.description).toContain("same canonical request");
    expect(command.description).toContain("conflicts");

    const commandId = command.parameters?.find(
      (parameter) => "name" in parameter && parameter.name === "command_id",
    );
    expect(commandId).toMatchObject({
      in: "path",
      required: true,
      schema: {
        $ref: CANONICAL_UUID_V7_REFERENCE,
      },
    });

    const requestSchema = openapi.components.schemas.PutCommandRequest;
    expect(requestSchema.additionalProperties).toBe(false);
    expect(Object.keys(requestSchema.properties).sort()).toEqual([
      "device_id",
      "group_correlation_id",
      "kind",
      "payload",
      "payload_version",
      "reason_code",
    ]);
    expect(requestSchema.properties.device_id).toEqual({
      $ref: CANONICAL_UUID_V7_REFERENCE,
    });

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

  it("permits only the frozen session and import credential keys", () => {
    const forbiddenKeys = collectObjectKeys(openapi).filter(({ key }) =>
      FORBIDDEN_CREDENTIAL_KEY.test(key),
    );
    expect(forbiddenKeys).toEqual([
      { key: "preview_token", path: IMPORT_COMMIT_TOKEN_PATH },
      { key: "preview_token", path: IMPORT_PREVIEW_TOKEN_PATH },
      { key: "password", path: SESSION_REQUEST_PASSWORD_PATH },
    ]);

    const commitToken =
      openapi.components.schemas.ImportCommitRequest.properties.preview_token;
    expect(commitToken.writeOnly).toBe(true);
    const previewToken =
      openapi.components.schemas.ImportPreviewResponse.properties.preview_token;
    expect(previewToken).not.toHaveProperty("writeOnly");

    const password =
      openapi.components.schemas.SessionRequest.properties.password;
    expect(password.writeOnly).toBe(true);
    expect(password).not.toHaveProperty("example");
    expect(password).not.toHaveProperty("examples");
    expect(password).not.toHaveProperty("default");
    expect(collectStringPaths(openapi, SESSION_REQUEST_REFERENCE)).toEqual([
      "/paths/~1api~1v2~1session/post/requestBody/content/application~1json/schema/$ref",
    ]);
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
