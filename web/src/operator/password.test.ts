import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  changePasswordSchema,
  PASSWORD_SPECIALS,
  registrationSchema,
  resetPasswordSchema,
} from "./password";
import { operatorLinkURL, readOperatorLink } from "./links";

function accepts(password: string) {
  return resetPasswordSchema.safeParse({
    password,
    password_confirmation: password,
  }).success;
}

describe("Operator password policy", () => {
  it("agrees with the Server schema for every ASCII character", () => {
    const document = JSON.parse(
      readFileSync(
        new URL("../../openapi/natsume.openapi.json", import.meta.url),
        "utf8",
      ),
    );
    const server = new RegExp(
      document.components.schemas.OperatorRegistrationRequest.properties
        .password.pattern,
    );
    for (let code = 0; code < 128; code++) {
      const character = String.fromCharCode(code);
      const password = `Abcdefghijklmn1@${character}`;
      const allowed =
        /[A-Za-z0-9]/.test(character) || PASSWORD_SPECIALS.includes(character);
      expect(accepts(password), `ASCII ${code}`).toBe(allowed);
      expect(server.test(password), `Server ASCII ${code}`).toBe(allowed);
    }
  });

  it("enforces exact length, digit, symbol, confirmation and ASCII boundaries", () => {
    for (const length of [16, 1024])
      expect(accepts(`1@${"a".repeat(length - 2)}`)).toBe(true);
    for (const length of [15, 1025])
      expect(accepts(`1@${"a".repeat(length - 2)}`)).toBe(false);
    expect(accepts("1".repeat(15) + "@")).toBe(true);
    for (const password of [
      "a".repeat(16),
      "1".repeat(16),
      "@".repeat(16),
      "Abcdefghijklmn1@中文",
      "Abcdefghijklmn1@\u00a0",
    ])
      expect(accepts(password)).toBe(false);
    expect(
      resetPasswordSchema.safeParse({
        password: "Abcdefghijklmn1@",
        password_confirmation: "Abcdefghijklmn1#",
      }).success,
    ).toBe(false);
  });

  it("validates username UTF-8 bytes and Unicode whitespace without normalization", () => {
    const credentials = {
      password: "Abcdefghijklmno1@",
      password_confirmation: "Abcdefghijklmno1@",
    };
    for (const username of [
      "A",
      "a",
      "中".repeat(42),
      "a".repeat(128),
      "a b",
      "\uFEFFname",
    ])
      expect(
        registrationSchema.safeParse({ ...credentials, username }).success,
      ).toBe(true);
    for (const username of [
      "",
      "a".repeat(129),
      "中".repeat(43),
      " name",
      "name\t",
      "\u0085name",
      "name\u3000",
    ])
      expect(
        registrationSchema.safeParse({ ...credentials, username }).success,
      ).toBe(false);
  });

  it("allows legacy current passwords while applying the policy to new passwords", () => {
    expect(
      changePasswordSchema.safeParse({
        current_password: "旧 密码",
        password: "Abcdefghijklmno1@",
        password_confirmation: "Abcdefghijklmno1@",
      }).success,
    ).toBe(true);
    expect(
      changePasswordSchema.safeParse({
        current_password: "中".repeat(342),
        password: "Abcdefghijklmno1@",
        password_confirmation: "Abcdefghijklmno1@",
      }).success,
    ).toBe(false);
  });
});

it("builds links from the current origin and reads only exact purpose-specific fragments", () => {
  const token = `invite_${"a".repeat(64)}`;
  const url = new URL(
    operatorLinkURL("https://panel.example:8443", "register", token),
  );
  expect(url.origin).toBe("https://panel.example:8443");
  expect(url.pathname).toBe("/register");
  expect(url.search).toBe("");
  expect(readOperatorLink(url.hash, "register")).toBe(token);
  for (const hash of [
    url.hash,
    "#token=reset_short",
    `#token=reset_${"A".repeat(64)}`,
    `#token=reset_${"a".repeat(64)}&token=other`,
  ])
    expect(readOperatorLink(hash, "reset-password")).toBe("");
  expect(
    readOperatorLink(`#token=reset_${"b".repeat(64)}`, "reset-password"),
  ).toBe(`reset_${"b".repeat(64)}`);
});
