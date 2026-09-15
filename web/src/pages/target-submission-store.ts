import type { QueryClient } from "@tanstack/react-query";
import { z } from "zod";

import type { createApiClient } from "../api/client";
import { ApiError, unwrap } from "../api/errors";
import type { TargetRequest, TargetResponse } from "./target-operations";

const requestSchema: z.ZodType<TargetRequest> = z
  .object({
    operation_id: z.string().uuid(),
    scope: z.discriminatedUnion("kind", [
      z.object({ kind: z.literal("all_enabled") }).strict(),
      z.object({ kind: z.literal("all_online_enabled") }).strict(),
      z
        .object({
          kind: z.literal("devices"),
          device_ids: z.array(z.string().uuid()).min(1),
        })
        .strict(),
    ]),
    action: z.discriminatedUnion("kind", [
      z
        .object({
          kind: z.literal("set_foreground"),
          foreground_target: z.enum(["waiting", "contest"]),
        })
        .strict(),
      z.object({ kind: z.literal("terminate_session") }).strict(),
      z.object({ kind: z.literal("reset_home") }).strict(),
      z.object({ kind: z.literal("power_off") }).strict(),
    ]),
  })
  .strict()
  .refine(
    ({ action, scope }) =>
      (action.kind === "power_off") === (scope.kind === "all_online_enabled"),
    "Power-off requires the online enabled scope",
  );

export interface TargetSubmissionState {
  pending: TargetRequest | null;
  sending: boolean;
  completed: { request: TargetRequest; response: TargetResponse } | null;
  error: string | null;
  storageReady: boolean;
}

// A single unresolved request per authenticated operator and browser tab.
// Receipt recovery is separate from the session's disposable query caches.
export function createTargetSubmissionStore(
  api: ReturnType<typeof createApiClient>,
  signal: AbortSignal,
  operatorId: string | null,
  queryClient: QueryClient,
) {
  const key = operatorId ? `natsume.target-submission.${operatorId}` : null;
  let state: TargetSubmissionState = {
    pending: null,
    sending: false,
    completed: null,
    error: null,
    storageReady: true,
  };
  const listeners = new Set<() => void>();
  if (key && typeof window !== "undefined") {
    try {
      const saved = window.sessionStorage.getItem(key);
      if (saved !== null)
        state.pending = requestSchema.parse(JSON.parse(saved));
    } catch {
      state = {
        ...state,
        storageReady: false,
        error:
          "Cannot restore the saved target request. New submissions are paused.",
      };
    }
  }

  function update(next: TargetSubmissionState) {
    if (signal.aborted) return;
    state = next;
    listeners.forEach((listener) => listener());
  }

  function clearSaved(request: TargetRequest) {
    signal.throwIfAborted();
    if (!key) throw new Error("No target submission owner");
    const saved = window.sessionStorage.getItem(key);
    // Never erase a request installed by another scope.
    if (saved !== JSON.stringify(request))
      throw new Error("Saved request changed");
    window.sessionStorage.removeItem(key);
  }

  async function send(request: TargetRequest) {
    if (signal.aborted || state.sending) return;
    update({ ...state, sending: true, error: null });
    try {
      const response = await unwrap<TargetResponse>(
        await api.POST("/api/v2/target-submissions", { body: request }),
      );
      signal.throwIfAborted();
      if (response.operation_id !== request.operation_id)
        throw new Error("Unexpected operation ID");
      clearSaved(request);
      update({
        ...state,
        pending: null,
        sending: false,
        completed: { request, response },
        error: null,
      });
      await queryClient.invalidateQueries({ queryKey: ["devices"] });
    } catch (error) {
      if (
        signal.aborted ||
        state.pending?.operation_id !== request.operation_id
      )
        return;
      // A malformed/conflicting request is definitively rejected. Transport,
      // 5xx and auth failures cannot establish the result of a prior attempt.
      if (
        error instanceof ApiError &&
        (error.status === 400 || error.status === 409)
      ) {
        try {
          clearSaved(request);
          update({
            ...state,
            pending: null,
            sending: false,
            error: error.title,
          });
          return;
        } catch {
          // Keep the request until its storage and submission result are resolved.
        }
      }
      update({
        ...state,
        sending: false,
        error:
          "Submission result not confirmed. Retry the original request to confirm its result.",
      });
    }
  }

  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    async submit(
      action: TargetRequest["action"],
      scope: TargetRequest["scope"],
    ) {
      if (signal.aborted || !key || state.pending || !state.storageReady)
        return;
      let request: TargetRequest;
      try {
        request = requestSchema.parse({
          operation_id: crypto.randomUUID(),
          action,
          scope,
        });
        window.sessionStorage.setItem(key, JSON.stringify(request));
      } catch {
        update({
          ...state,
          error: "Cannot save the target request. No submission was sent.",
        });
        return;
      }
      update({ ...state, pending: request, completed: null, error: null });
      await send(request);
    },
    async retry() {
      if (state.pending && state.storageReady) await send(state.pending);
    },
  };
}
