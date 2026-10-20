import { readFileSync } from "node:fs";
import ts from "typescript";
import { expect, expectTypeOf, it } from "vitest";

import type {
  BindingEvaluationResponse,
  BindingTargetResponse_Serialize,
  DeviceResponse_Serialize,
  ImportPendingResponse,
  ImportPendingSummary,
  OperatorRegistrationRequest,
  PrintDocument,
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
