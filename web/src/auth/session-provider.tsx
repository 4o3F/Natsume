import { useState, useSyncExternalStore, type ReactNode } from "react";
import { QueryClientProvider } from "@tanstack/react-query";

import { SessionController } from "./session";
import { SessionContext } from "./session-context";

export function SessionProvider({ children }: { children: ReactNode }) {
  const [controller] = useState(() => new SessionController());
  const scope = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
  );
  return (
    <SessionContext.Provider value={scope}>
      <QueryClientProvider key={scope.generation} client={scope.queryClient}>
        {children}
      </QueryClientProvider>
    </SessionContext.Provider>
  );
}
