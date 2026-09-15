import { afterEach, expect, it, vi } from "vitest";

import { createApiClient } from "./client";

afterEach(() => vi.unstubAllGlobals());

it("serializes device states as one CSV query parameter", async () => {
  const fetch = vi.fn(async () => new Response("[]"));
  vi.stubGlobal("fetch", fetch);
  const api = createApiClient(new AbortController().signal, vi.fn());

  await api.GET("/api/v2/devices", {
    baseUrl: "http://localhost",
    params: { query: { state: ["enabled", "disabled"] } },
  });

  expect(fetch).toHaveBeenCalledOnce();
  const request = vi.mocked(globalThis.fetch).mock.calls[0][0] as Request;
  expect(new URL(request.url).searchParams.getAll("state")).toEqual([
    "enabled,disabled",
  ]);
});
