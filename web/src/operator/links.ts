export type OperatorLinkKind = "register" | "reset-password";

export function readOperatorLink(hash: string, kind: OperatorLinkKind) {
  const prefix = kind === "register" ? "invite" : "reset";
  const match = new RegExp(`^#token=(${prefix}_[0-9a-f]{64})$`).exec(hash);
  return match?.[1] ?? "";
}

export function operatorLinkURL(
  origin: string,
  kind: OperatorLinkKind,
  token: string,
) {
  const url = new URL(`/${kind}`, origin);
  url.hash = `token=${token}`;
  return url.href;
}
