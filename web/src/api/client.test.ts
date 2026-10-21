import { afterEach, expect, expectTypeOf, it, vi } from "vitest";

import { createApiClient } from "./client";
import { ApiError, unwrap } from "./errors";
import type {
  DeviceUpdateRequest,
  PrintQueueEntry,
  TargetSubmissionBody,
} from "./generated/types";

const baseUrl = "https://natsume.test";
const xlsxType =
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

function client(onUnauthorized = vi.fn(), controller = new AbortController()) {
  return createApiClient(controller.signal, onUnauthorized);
}

afterEach(() => vi.unstubAllGlobals());

it("serializes device states as one CSV query parameter and preserves an omitted filter", async () => {
  const fetch = vi.fn(async () => new Response("[]"));
  vi.stubGlobal("fetch", fetch);
  const api = client();
  await api.listDevices(["enabled", "disabled"], { baseUrl });
  await api.listDevices(undefined, { baseUrl });
  const calls = vi.mocked(globalThis.fetch).mock.calls;
  const first = calls[0][0] as Request;
  const second = calls[1][0] as Request;
  expect(new URL(first.url).searchParams.getAll("state")).toEqual([
    "enabled,disabled",
  ]);
  expect(new URL(second.url).search).toBe("");
});

it("reads the operator print queue through the typed GET endpoint", async () => {
  let sent!: Request;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (request: Request) => {
      sent = request;
      return new Response("[]");
    }),
  );
  const result = await client().listPrintQueue({ baseUrl });
  expect(sent.method).toBe("GET");
  expect(new URL(sent.url).pathname).toBe("/api/v2/print/queue");
  expect(sent.headers.has("authorization")).toBe(false);
  expect(sent.body).toBeNull();
  expect(result.data).toEqual([]);
  expectTypeOf(result.data).toEqualTypeOf<PrintQueueEntry[] | undefined>();
});

it("invalidates an unauthorized session without treating rejected login as expiry", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response("{}", { status: 401 })),
  );
  const onUnauthorized = vi.fn();
  const api = client(onUnauthorized);
  await api.createSession(
    { login_name: "operator", password: "incorrect" },
    { baseUrl },
  );
  expect(onUnauthorized).not.toHaveBeenCalled();
  await api.getSession({ baseUrl });
  expect(onUnauthorized).toHaveBeenCalledOnce();
});

it("does not dispatch requests after the session scope is aborted", async () => {
  const fetch = vi.fn(async () => new Response("{}"));
  vi.stubGlobal("fetch", fetch);
  const controller = new AbortController();
  const api = client(vi.fn(), controller);
  controller.abort();
  await expect(api.getSession({ baseUrl })).rejects.toMatchObject({
    name: "AbortError",
  });
  expect(fetch).not.toHaveBeenCalled();
});

it.each(["scope", "request"])(
  "propagates %s cancellation to an in-flight request",
  async (owner) => {
    const scope = new AbortController();
    const local = new AbortController();
    let sent!: Request;
    vi.stubGlobal(
      "fetch",
      vi.fn((request: Request) => {
        sent = request;
        return new Promise<Response>((_resolve, reject) => {
          request.signal.addEventListener(
            "abort",
            () => reject(request.signal.reason),
            { once: true },
          );
        });
      }),
    );
    const pending = client(vi.fn(), scope).listDevices(undefined, {
      baseUrl,
      signal: local.signal,
    });
    const rejected = expect(pending).rejects.toMatchObject({
      name: "AbortError",
    });
    (owner === "scope" ? scope : local).abort();
    await rejected;
    expect(sent.signal.aborted).toBe(true);
  },
);

it("sends generated JSON inputs with the endpoint's method and encoded path", async () => {
  let sent!: Request;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (request: Request) => {
      sent = request;
      return new Response(null, { status: 204 });
    }),
  );
  const result = await client().updateDevice(
    "id/with ?#",
    { state: "disabled" },
    { baseUrl },
  );
  expect(sent.method).toBe("PATCH");
  expect(new URL(sent.url).pathname).toBe("/api/v2/devices/id%2Fwith%20%3F%23");
  expect(sent.headers.get("content-type")).toBe("application/json");
  expect(await sent.json()).toEqual({ state: "disabled" });
  await expect(unwrap(result)).resolves.toBeUndefined();
});

it("uploads raw XLSX bytes and sends the preview token only in the commit header", async () => {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (request: Request) => {
      requests.push(request);
      return requests.length === 1
        ? new Response('{"candidate_id":"candidate","preview_token":"token"}', {
            status: 201,
          })
        : new Response(null, { status: 204 });
    }),
  );
  const file = new Blob([new Uint8Array([0x50, 0x4b, 0, 0xff])]);
  const api = client();
  const created = await api.createImport(file, { baseUrl });
  await api.commitImport("candidate", "private-preview-token", file, {
    baseUrl,
  });
  expect(created.data?.preview_token).toBe("token");
  for (const request of requests) {
    expect(request.headers.get("content-type")).toBe(xlsxType);
    expect(new Uint8Array(await request.arrayBuffer())).toEqual(
      new Uint8Array(await file.arrayBuffer()),
    );
    expect(request.url).not.toContain("private-preview-token");
  }
  expect(requests[0].headers.has("x-natsume-preview-token")).toBe(false);
  expect(requests[1].headers.get("x-natsume-preview-token")).toBe(
    "private-preview-token",
  );
  expect(new URL(requests[1].url).pathname).toBe(
    "/api/v2/imports/candidate/actions/commit",
  );
});

it("returns ZIP bytes without trying to decode a successful download as JSON", async () => {
  const bytes = new Uint8Array([0x50, 0x4b, 0, 0xff]);
  vi.stubGlobal(
    "fetch",
    vi.fn(
      async () =>
        new Response(bytes, {
          headers: { "Content-Type": "application/zip" },
        }),
    ),
  );
  const blob = await unwrap(await client().exportDomjudge({ baseUrl }));
  expect(new Uint8Array(await blob.arrayBuffer())).toEqual(bytes);
});

it.each(["json", "text"])(
  "handles %s download errors through the shared HTTP error mapping",
  async (format) => {
    const body =
      format === "json"
        ? JSON.stringify({
            code: "AUTHORIZATION_DENIED",
            status: 403,
            title: "Forbidden",
          })
        : "upstream unavailable";
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(body, { status: 403 })),
    );
    await expect(
      unwrap(await client().exportDomjudge({ baseUrl })),
    ).rejects.toMatchObject({
      code: format === "json" ? "AUTHORIZATION_DENIED" : "UNPARSEABLE_RESPONSE",
      status: 403,
    });
  },
);

it("does not retry uncertain target submissions", async () => {
  const fetch = vi.fn(async () => {
    throw new TypeError("connection lost");
  });
  vi.stubGlobal("fetch", fetch);
  await expect(
    client().submitTargets(
      {
        operation_id: "request-id",
        scope: { kind: "all_enabled" },
        action: { kind: "reset_home" },
      },
      { baseUrl },
    ),
  ).rejects.toThrow("connection lost");
  expect(fetch).toHaveBeenCalledOnce();
});

it("preserves generated request and result types at named endpoint boundaries", () => {
  const api = client();
  expectTypeOf(api.updateDevice)
    .parameter(1)
    .toEqualTypeOf<DeviceUpdateRequest>();
  expectTypeOf(api.submitTargets)
    .parameter(0)
    .toEqualTypeOf<TargetSubmissionBody>();
  expectTypeOf(api.commitImport).parameter(2).toEqualTypeOf<Blob>();
  expectTypeOf(api.exportDomjudge)
    .returns.resolves.toHaveProperty("data")
    .toEqualTypeOf<Blob | undefined>();
  expect(new ApiError("INVALID_REQUEST", 400, "Bad Request").name).toBe(
    "ApiError",
  );
});
