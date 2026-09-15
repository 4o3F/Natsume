import createClient from "openapi-fetch";

import type { paths } from "@/api/generated/schema";

export function createApiClient(
  signal: AbortSignal,
  onUnauthorized: () => void,
) {
  const api = createClient<paths>({
    querySerializer: { array: { style: "form", explode: false } },
  });
  api.use({
    onRequest({ request }) {
      signal.throwIfAborted();
      return new Request(request, {
        signal: AbortSignal.any([signal, request.signal]),
      });
    },
    onResponse({ request, response }) {
      signal.throwIfAborted();
      const isLoginRequest =
        request.method === "POST" &&
        new URL(request.url).pathname.endsWith("/api/v2/session");
      if (response.status === 401 && !isLoginRequest) {
        onUnauthorized();
      }
    },
  });
  return api;
}
