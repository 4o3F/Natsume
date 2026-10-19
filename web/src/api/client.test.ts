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

it("invalidates an unauthorized session without treating rejected login as expiry", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response("{}", { status: 401 })),
  );
  const onUnauthorized = vi.fn();
  const api = createApiClient(new AbortController().signal, onUnauthorized);

  await api.POST("/api/v2/session", {
    baseUrl: "http://localhost",
    body: { login_name: "operator", password: "incorrect" },
  });
  expect(onUnauthorized).not.toHaveBeenCalled();

  await api.GET("/api/v2/session", { baseUrl: "http://localhost" });
  expect(onUnauthorized).toHaveBeenCalledOnce();
});

it("does not dispatch requests after the session scope is aborted", async () => {
  const fetch = vi.fn(async () => new Response("{}"));
  vi.stubGlobal("fetch", fetch);
  const controller = new AbortController();
  const api = createApiClient(controller.signal, vi.fn());
  controller.abort();

  await expect(
    api.GET("/api/v2/session", { baseUrl: "http://localhost" }),
  ).rejects.toMatchObject({ name: "AbortError" });
  expect(fetch).not.toHaveBeenCalled();
});
