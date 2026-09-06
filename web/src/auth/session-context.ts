import { createContext, useContext } from "react";

import type { SessionScope } from "./session";

export const SessionContext = createContext<SessionScope | null>(null);

export function useSessionScope() {
  const scope = useContext(SessionContext);
  if (!scope) throw new Error("SessionProvider is required");
  return scope;
}
