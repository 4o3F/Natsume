import { useMutation, useQuery } from "@tanstack/react-query";

import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { SESSION_POLL_MS } from "@/api/polling";
import { SESSION_KEY } from "./session";
import { useSessionScope } from "./session-context";

type SessionRequest = components["schemas"]["SessionRequest"];
type SessionResponse = components["schemas"]["SessionResponse"];

export function useSession() {
  const scope = useSessionScope();
  return useQuery({
    queryKey: SESSION_KEY,
    queryFn: async ({ signal }): Promise<SessionResponse | null> => {
      const result = await scope.api.GET("/api/v2/session", { signal });
      const identity =
        result.response.status === 401
          ? null
          : await unwrap<SessionResponse>(result);
      scope.observe(identity);
      return identity;
    },
    refetchInterval: SESSION_POLL_MS,
    retry: false,
  });
}

export function useLogin() {
  const scope = useSessionScope();

  return useMutation({
    mutationFn: async (body: SessionRequest) =>
      unwrap<SessionResponse>(
        await scope.api.POST("/api/v2/session", {
          body,
        }),
      ),
    onSuccess: scope.login,
  });
}

export function useLogout() {
  const scope = useSessionScope();

  return useMutation({
    mutationFn: async () =>
      unwrap<void>(await scope.api.DELETE("/api/v2/session")),
    onSuccess: scope.logout,
  });
}
