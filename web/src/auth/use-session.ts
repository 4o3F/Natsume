import { useMutation, useQuery } from "@tanstack/react-query";

import { unwrap } from "@/api/errors";
import type { SessionRequest, SessionResponse } from "@/api/generated/types";
import { SESSION_POLL_MS } from "@/api/polling";
import { SESSION_KEY } from "./session";
import { useSessionScope } from "./session-context";

export function useSession() {
  const scope = useSessionScope();
  return useQuery({
    queryKey: SESSION_KEY,
    queryFn: async ({ signal }): Promise<SessionResponse | null> => {
      const result = await scope.api.getSession({ signal });
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
      unwrap<SessionResponse>(await scope.api.createSession(body)),
    onSuccess: scope.login,
  });
}

export function useLogout() {
  const scope = useSessionScope();

  return useMutation({
    mutationFn: async () => unwrap<void>(await scope.api.deleteSession()),
    onSuccess: scope.logout,
  });
}
