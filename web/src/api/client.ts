import createClient from "openapi-fetch";

import type { paths } from "@/api/generated/schema";

let onUnauthorized: (() => void) | undefined;

export const api = createClient<paths>();

export function setOnUnauthorized(cb: () => void) {
  onUnauthorized = cb;
}

api.use({
  onResponse({ request, response }) {
    const isLoginRequest =
      request.method === "POST" &&
      new URL(request.url).pathname.endsWith("/api/v2/session");

    if (response.status === 401 && !isLoginRequest) {
      onUnauthorized?.();
    }
  },
});
