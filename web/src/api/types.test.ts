import { readFileSync } from "node:fs";
import ts from "typescript";
import { expect, expectTypeOf, it } from "vitest";

import type {
  AttemptReport,
  BindingEvaluationResponse,
  BindingTargetResponse_Serialize,
  DeviceResponse_Serialize,
  ImportPendingResponse,
  ImportPendingSummary,
  OperatorRegistrationRequest,
  PrintAttempt,
  PrintClaim,
  PrintClientIssued,
  PrintClientPair,
  PrintDocument,
  PrintExecution,
  PrintGenerationFailure,
  PrintPairingCode,
  PrintPrinter,
  PrintQueueEntry,
  PrintRegistration,
  PrintRequest,
} from "./generated/types";

const source = ts.createSourceFile(
  "types.ts",
  readFileSync(new URL("./generated/types.ts", import.meta.url), "utf8"),
  ts.ScriptTarget.Latest,
  true,
);
const declarations = source.statements.filter(ts.isTypeAliasDeclaration);

it("keeps required nullable response fields distinct from omitted fields", () => {
  expectTypeOf<Pick<PrintRequest, "document">>().toEqualTypeOf<{
    document: PrintDocument | null;
  }>();
  expectTypeOf<ImportPendingResponse>().toEqualTypeOf<{
    pending: ImportPendingSummary | null;
  }>();
  expectTypeOf<Pick<DeviceResponse_Serialize, "network">>().toEqualTypeOf<{
    network: DeviceResponse_Serialize["network"];
  }>();
  type Unbound = Extract<BindingTargetResponse_Serialize, { state: "unbound" }>;
  expectTypeOf<Unbound["evaluation"]>().toEqualTypeOf<
    BindingEvaluationResponse | null | undefined
  >();
});

it("exports queue counters as JSON numbers and keeps the optional seat value required", () => {
  expectTypeOf<
    Pick<PrintQueueEntry, "ready_sequence" | "pages" | "bytes">
  >().toEqualTypeOf<{
    ready_sequence: number;
    pages: number;
    bytes: number;
  }>();
  expectTypeOf<Pick<PrintQueueEntry, "seat_label">>().toEqualTypeOf<{
    seat_label: string | null;
  }>();
});

it("exports pairing expiry as JSON numbers with required nullable printer fields", () => {
  expectTypeOf<
    PrintPairingCode["expires_at_unix_ms"]
  >().toEqualTypeOf<number>();
  expectTypeOf<
    Pick<PrintPrinter, "client_id" | "pairing_expires_at_unix_ms">
  >().toEqualTypeOf<{
    client_id: string | null;
    pairing_expires_at_unix_ms: number | null;
  }>();
});

it("keeps Print versions in URL paths rather than JSON fields", () => {
  expectTypeOf<
    Extract<
      "protocol_version",
      | keyof PrintRegistration
      | keyof PrintGenerationFailure
      | keyof PrintRequest
      | keyof PrintClientPair
      | keyof PrintClientIssued
      | keyof PrintClaim
      | keyof PrintAttempt
    >
  >().toEqualTypeOf<never>();
});

it("exports execution identities, deadlines, and nullable results without bigint", () => {
  expectTypeOf<
    Pick<
      PrintAttempt,
      "ready_sequence" | "lease_expires_at_unix_ms" | "last_contact_at_unix_ms"
    >
  >().toEqualTypeOf<{
    ready_sequence: number;
    lease_expires_at_unix_ms: number;
    last_contact_at_unix_ms: number;
  }>();
  expectTypeOf<
    Pick<
      PrintAttempt,
      "job_id" | "last_observed_at_unix_ms" | "finished_at_unix_ms"
    >
  >().toEqualTypeOf<{
    job_id: number | null;
    last_observed_at_unix_ms: number | null;
    finished_at_unix_ms: number | null;
  }>();
  expectTypeOf<PrintExecution["retry_at_unix_ms"]>().toEqualTypeOf<
    number | null
  >();
  expectTypeOf<Extract<AttemptReport, { kind: "contact" }>>().toEqualTypeOf<{
    kind: "contact";
  }>();
  expectTypeOf<
    Extract<AttemptReport, { kind: "printed" }>["job"]["job_id"]
  >().toEqualTypeOf<number>();
});

it("exports secret wrappers as JSON strings without their internal storage", () => {
  expectTypeOf<
    OperatorRegistrationRequest["password"]
  >().toEqualTypeOf<string>();
  const names = declarations.map((declaration) => declaration.name.text);
  expect(names).not.toContain("SecretValue");
  expect(names).not.toContain("SecretString");
});

it("exposes only the error response fields, without correlation or internal diagnostics", () => {
  const error = declarations.find(
    (declaration) => declaration.name.text === "ErrorResponse",
  )!;
  expect(ts.isTypeLiteralNode(error.type)).toBe(true);
  const fields = (error.type as ts.TypeLiteralNode).members.map((member) =>
    member.name?.getText(source),
  );
  expect(fields.sort()).toEqual(["code", "status", "title"]);
});

it("does not leak credential fields outside explicitly authorized HTTP DTOs", () => {
  const forbidden =
    /^(?:(?:\w*_)?private_key(?:_\w*)?|(?:\w*_)?pass(?:word|phrase)(?:_(?:value|plaintext|material|secret|confirmation))?|(?:\w*_)?token(?:_(?:value|plaintext|material|secret))?|(?:\w*_)?secret(?:_(?:value|plaintext|material|key))?)$/i;
  const allowed = new Set([
    "SessionRequest.password",
    "PrintSourceIssued.token",
    "PrintClientIssued.token",
    "ImportPreviewResponse.preview_token",
    "OperatorInvitationIssuedResponse.token",
    "OperatorPasswordResetIssuedResponse.token",
    "OperatorTokenRequest.token",
    "OperatorRegistrationRequest.token",
    "OperatorRegistrationRequest.password",
    "OperatorRegistrationRequest.password_confirmation",
    "OperatorPasswordResetRequest.token",
    "OperatorPasswordResetRequest.password",
    "OperatorPasswordResetRequest.password_confirmation",
    "OperatorPasswordChangeRequest.current_password",
    "OperatorPasswordChangeRequest.password",
    "OperatorPasswordChangeRequest.password_confirmation",
  ]);
  const found = new Set<string>();
  for (const declaration of declarations) {
    function visit(node: ts.Node) {
      if (ts.isPropertySignature(node) && node.name) {
        const name = node.name
          .getText(source)
          .replaceAll('"', "")
          .replaceAll("-", "_");
        if (forbidden.test(name)) {
          const path = `${declaration.name.text}.${name}`;
          expect(allowed.has(path), path).toBe(true);
          found.add(path);
        }
      }
      ts.forEachChild(node, visit);
    }
    visit(declaration.type);
  }
  expect(found).toEqual(allowed);
  for (const name of [
    "private_key",
    "gateway_private_key_der",
    "password",
    "password_confirmation",
    "domjudge_password",
    "passphrase",
    "token",
    "device_token",
    "raw_token",
    "access_token",
    "api_token",
    "session_token",
    "secret",
    "client_secret",
    "shared_secret",
    "secret_key",
  ])
    expect(forbidden.test(name), name).toBe(true);
});
