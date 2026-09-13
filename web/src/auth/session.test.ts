import { afterEach, describe, expect, it, vi } from "vitest";

import { SessionController } from "./session";

const admin = { operator_id: "admin-1", role: "admin" };
const preview = {
  candidate_id: "candidate-1",
  preview_token: "old-token",
  file: new File(["synthetic workbook"], "roster.xlsx"),
};
const baseUrl = "https://natsume.test";

afterEach(() => vi.unstubAllGlobals());

describe("session ownership", () => {
  it("keeps the current cache and preview during ordinary polling", () => {
    const controller = new SessionController();
    controller.getSnapshot().observe(admin);
    const scope = controller.getSnapshot();
    scope.preparation.set(preview);
    scope.queryClient.setQueryData(["seats"], ["A-01"]);
    scope.observe({ ...admin });
    expect(controller.getSnapshot()).toBe(scope);
    expect(scope.preparation.get()).toEqual(preview);
    expect(scope.queryClient.getQueryData(["seats"])).toEqual(["A-01"]);
  });

  it.each(["logout", "new identity", "new role", "same-account login"])(
    "%s starts with empty business caches, mutations and preview",
    async (transition) => {
      const controller = new SessionController();
      controller.getSnapshot().login(admin);
      const old = controller.getSnapshot();
      old.preparation.set(preview);
      old.queryClient.setQueryData(["seats"], ["old-row"]);
      const failed = old.queryClient.getMutationCache().build(old.queryClient, {
        mutationFn: () => Promise.reject(new Error("old operation error")),
      });
      await expect(failed.execute(undefined)).rejects.toThrow(
        "old operation error",
      );

      switch (transition) {
        case "logout":
          old.logout();
          break;
        case "new identity":
          old.observe({ ...admin, operator_id: "admin-2" });
          break;
        case "new role":
          old.observe({ ...admin, role: "viewer" });
          break;
        default:
          old.login(admin);
      }
      const current = controller.getSnapshot();
      expect(current.generation).toBe(old.generation + 1);
      expect(current.queryClient.getQueryData(["seats"])).toBeUndefined();
      expect(current.queryClient.getMutationCache().getAll()).toHaveLength(0);
      expect(current.preparation.get()).toBeNull();
      expect(old.queryClient.getQueryCache().getAll()).toHaveLength(0);
      expect(old.queryClient.getMutationCache().getAll()).toHaveLength(0);
      expect(old.preparation.get()).toBeNull();
      expect(() => old.preparation.set(preview)).toThrow();
    },
  );

  it("late mutation callbacks can only affect the retired scope", async () => {
    const controller = new SessionController();
    controller.getSnapshot().login(admin);
    const old = controller.getSnapshot();
    let finish!: () => void;
    const result = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const mutation = old.queryClient.getMutationCache().build(old.queryClient, {
      mutationFn: () => result,
      onSuccess: () => {
        old.preparation.clear();
        old.queryClient.setQueryData(["seats"], ["old-result"]);
        old.logout();
        old.login(admin);
      },
    });
    const pending = mutation.execute(undefined);
    old.logout();
    controller.getSnapshot().login({ ...admin, operator_id: "admin-2" });
    const current = controller.getSnapshot();
    current.preparation.set({ ...preview, preview_token: "new-token" });
    current.queryClient.setQueryData(["seats"], ["new-result"]);
    finish();
    await pending;
    expect(controller.getSnapshot()).toBe(current);
    expect(current.preparation.get()?.preview_token).toBe("new-token");
    expect(current.queryClient.getQueryData(["seats"])).toEqual(["new-result"]);
    expect(current.queryClient.getMutationCache().getAll()).toHaveLength(0);
  });

  it("ignores an old 401 even if the transport delivers it after cancellation", async () => {
    let finish!: (response: Response) => void;
    const response = new Promise<Response>((resolve) => {
      finish = resolve;
    });
    const fetch = vi.fn(() => response);
    vi.stubGlobal("fetch", fetch);
    const controller = new SessionController();
    controller.getSnapshot().login(admin);
    const old = controller.getSnapshot();
    const pending = old.api.GET("/api/v2/seats", { baseUrl });
    const rejected = expect(pending).rejects.toMatchObject({
      name: "AbortError",
    });
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());
    old.logout();
    controller.getSnapshot().login({ ...admin, role: "viewer" });
    const current = controller.getSnapshot();
    finish(new Response("{}", { status: 401 }));
    await rejected;
    expect(controller.getSnapshot()).toBe(current);
    expect(current.identity?.role).toBe("viewer");
  });

  it("rejects a retired upload before sending it, including after local file reading", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const controller = new SessionController();
    controller.getSnapshot().login(admin);
    const old = controller.getSnapshot();
    old.logout();
    controller.getSnapshot().login(admin);
    await expect(
      old.api.POST("/api/v2/imports", { baseUrl, body: "old XLSX" }),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(fetch).not.toHaveBeenCalled();
  });

  it("cannot restore a token when body parsing finishes after the session ends", async () => {
    let finish!: (data: unknown) => void;
    const body = new Promise((resolve) => {
      finish = resolve;
    });
    const response = new Response("{}", { status: 201 });
    const parse = vi.spyOn(response, "json").mockImplementation(() => body);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => response),
    );
    const controller = new SessionController();
    controller.getSnapshot().login(admin);
    const old = controller.getSnapshot();
    const pending = old.api.POST("/api/v2/imports", { baseUrl, body: "" });
    await vi.waitFor(() => expect(parse).toHaveBeenCalledOnce());
    old.logout();
    controller.getSnapshot().login(admin);
    const current = controller.getSnapshot();
    current.preparation.set({ ...preview, preview_token: "new-token" });
    finish(preview);
    const result = await pending;
    expect(() =>
      old.preparation.set({ ...result.data!, file: preview.file }),
    ).toThrow();
    expect(current.preparation.get()?.preview_token).toBe("new-token");
  });
});
