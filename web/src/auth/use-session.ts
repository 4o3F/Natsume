import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "@/api/client";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { SESSION_POLL_MS } from "@/api/polling";

type SessionRequest = components["schemas"]["SessionRequest"];
type SessionResponse = components["schemas"]["SessionResponse"];

export const SESSION_KEY = ["session"] as const;

export function useSession() {
  return useQuery({
    queryKey: SESSION_KEY,
    queryFn: async (): Promise<SessionResponse | null> => {
      const result = await api.GET("/api/v2/session");

      if (result.response.status === 401) {
        return null;
      }

      return unwrap<SessionResponse>(result);
    },
    refetchInterval: SESSION_POLL_MS,
    retry: false,
  });
}

export function useLogin() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (body: SessionRequest) =>
      unwrap<SessionResponse>(
        await api.POST("/api/v2/session", {
          body,
        }),
      ),
    onSuccess: async (session) => {
      await queryClient.cancelQueries({ queryKey: SESSION_KEY });
      queryClient.setQueryData(SESSION_KEY, session);
    },
  });
}

export function useLogout() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async () => unwrap<void>(await api.DELETE("/api/v2/session")),
    onSuccess: async () => {
      await queryClient.cancelQueries();
      queryClient.setQueryData(SESSION_KEY, null);
      queryClient.removeQueries({
        predicate: (query) => query.queryKey[0] !== "session",
      });
    },
  });
}
