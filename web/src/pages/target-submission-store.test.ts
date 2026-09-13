import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { SessionController } from "../auth/session";
import type { TargetRequest } from "./target-operations";

const admin = { operator_id: "admin-a", role: "admin" };
const action = { kind: "reset_home" } as const;
const scope = { kind: "all_enabled" } as const;
let saved: Map<string, string>;
let storage: {
  getItem: ReturnType<typeof vi.fn>;
  setItem: ReturnType<typeof vi.fn>;
  removeItem: ReturnType<typeof vi.fn>;
};

beforeEach(() => {
  saved = new Map();
  storage = {
    getItem: vi.fn((key: string) => saved.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => {
      saved.set(key, value);
    }),
    removeItem: vi.fn((key: string) => {
      saved.delete(key);
    }),
  };
  vi.stubGlobal("window", { sessionStorage: storage });
  const NativeRequest = globalThis.Request;
  vi.stubGlobal(
    "Request",
    class extends NativeRequest {
      constructor(input: RequestInfo | URL, init?: RequestInit) {
        super(
          typeof input === "string" && input.startsWith("/")
            ? new URL(input, "https://natsume.test")
            : input,
          init,
        );
      }
    },
  );
});

afterEach(() => vi.unstubAllGlobals());

function controller() {
  const owner = new SessionController();
  owner.getSnapshot().login(admin);
  return owner;
}

function success(request: TargetRequest) {
  return new Response(
    JSON.stringify({ operation_id: request.operation_id, results: [] }),
    { headers: { "Content-Type": "application/json" } },
  );
}

it("saves before sending and removes the request only after a matching receipt", async () => {
  const fetch = vi.fn(async (http: Request) => {
    const request: TargetRequest = await http.json();
    expect(new URL(http.url).pathname).toBe("/api/v2/target-submissions");
    expect(http.method).toBe("POST");
    expect([...saved.values()].map((value) => JSON.parse(value))).toEqual([
      request,
    ]);
    return success(request);
  });
  vi.stubGlobal("fetch", fetch);
  const store = controller().getSnapshot().targetSubmission;
  await store.submit(action, scope);
  expect(fetch).toHaveBeenCalledOnce();
  expect(saved.size).toBe(0);
  expect(store.getSnapshot().pending).toBeNull();
  expect(store.getSnapshot().completed?.request.action).toEqual(action);
});

it("restores the original request after refresh and only for its authenticated owner", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response("{}", { status: 503 })),
  );
  const old = controller();
  await old.getSnapshot().targetSubmission.submit(action, scope);
  const request = old.getSnapshot().targetSubmission.getSnapshot().pending;
  expect(request).not.toBeNull();
  old.getSnapshot().logout();
  const refreshed = new SessionController();
  expect(
    refreshed.getSnapshot().targetSubmission.getSnapshot().pending,
  ).toBeNull();
  expect(saved.size).toBe(1);
  refreshed.getSnapshot().login({ ...admin, operator_id: "admin-b" });
  expect(
    refreshed.getSnapshot().targetSubmission.getSnapshot().pending,
  ).toBeNull();
  const fetch = vi.fn(async (http: Request) => {
    const replay: TargetRequest = await http.json();
    expect(replay).toEqual(request);
    return success(replay);
  });
  vi.stubGlobal("fetch", fetch);
  refreshed.getSnapshot().login(admin);
  const store = refreshed.getSnapshot().targetSubmission;
  expect(store.getSnapshot().pending).toEqual(request);
  await store.submit({ kind: "terminate_session" }, scope);
  expect(fetch).not.toHaveBeenCalled();
  await store.retry();
  expect(fetch).toHaveBeenCalledOnce();
  expect(saved.size).toBe(0);
});

it("blocks a second submission while the first request is in flight", async () => {
  let finish!: (response: Response) => void;
  const fetch = vi.fn(
    () =>
      new Promise<Response>((resolve) => {
        finish = resolve;
      }),
  );
  vi.stubGlobal("fetch", fetch);
  const store = controller().getSnapshot().targetSubmission;
  const first = store.submit(action, scope);
  await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());
  await store.submit(
    { kind: "terminate_session" },
    { kind: "devices", device_ids: ["01900000-0000-7000-8000-000000000001"] },
  );
  await store.retry();
  expect(fetch).toHaveBeenCalledOnce();
  finish(success(store.getSnapshot().pending!));
  await first;
  expect(store.getSnapshot().pending).toBeNull();
});

it("late responses cannot remove another operator's pending request", async () => {
  const replies: ((response: Response) => void)[] = [];
  const fetch = vi.fn(
    () =>
      new Promise<Response>((resolve) => {
        replies.push(resolve);
      }),
  );
  vi.stubGlobal("fetch", fetch);
  const owner = controller();
  const firstStore = owner.getSnapshot().targetSubmission;
  const first = firstStore.submit(action, scope);
  await vi.waitFor(() => expect(replies).toHaveLength(1));
  const firstRequest = firstStore.getSnapshot().pending!;
  owner.getSnapshot().logout();
  owner.getSnapshot().login({ ...admin, operator_id: "admin-b" });
  const secondStore = owner.getSnapshot().targetSubmission;
  const second = secondStore.submit({ kind: "terminate_session" }, scope);
  await vi.waitFor(() => expect(replies).toHaveLength(2));
  const secondRequest = secondStore.getSnapshot().pending!;
  replies[0](success(firstRequest));
  await first;
  expect(secondStore.getSnapshot().pending).toEqual(secondRequest);
  expect([...saved.values()].map((value) => JSON.parse(value))).toEqual([
    firstRequest,
    secondRequest,
  ]);
  replies[1](success(secondRequest));
  await second;
  expect([...saved.values()].map((value) => JSON.parse(value))).toEqual([
    firstRequest,
  ]);
});

it("does not send when the original request cannot be persisted", async () => {
  const fetch = vi.fn();
  vi.stubGlobal("fetch", fetch);
  storage.setItem.mockImplementation(() => {
    throw new Error("quota");
  });
  const store = controller().getSnapshot().targetSubmission;
  await store.submit(action, scope);
  expect(fetch).not.toHaveBeenCalled();
  expect(store.getSnapshot().pending).toBeNull();
  expect(store.getSnapshot().error).toContain("No submission was sent");
});

it("keeps the request when a response identifies another operation", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () =>
      success({
        operation_id: "ec7737b0-1b2c-4def-8123-0123456789ab",
        action,
        scope,
      }),
    ),
  );
  const store = controller().getSnapshot().targetSubmission;
  await store.submit(action, scope);
  expect(store.getSnapshot().pending).not.toBeNull();
  expect(store.getSnapshot().completed).toBeNull();
  expect(saved.size).toBe(1);
});
