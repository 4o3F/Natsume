import { QueryClient } from "@tanstack/react-query";

import { createApiClient } from "../api/client";
import type { components } from "../api/generated/schema";
import { createPreparationStore } from "../pages/preparation-store";

type Identity = components["schemas"]["SessionResponse"];
export const SESSION_KEY = ["session"] as const;

export interface SessionScope {
  readonly generation: number;
  readonly identity: Identity | null | undefined;
  readonly api: ReturnType<typeof createApiClient>;
  readonly queryClient: QueryClient;
  readonly preparation: ReturnType<typeof createPreparationStore>;
  observe(identity: Identity | null): void;
  login(identity: Identity): void;
  logout(): void;
}

export class SessionController {
  private scope: SessionScope;
  private disposeScope: () => void = () => {};
  private readonly listeners = new Set<() => void>();

  constructor() {
    this.scope = this.createScope(0, undefined);
  }

  getSnapshot = () => this.scope;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  private change(
    expected: SessionScope,
    identity: Identity | null,
    login = false,
  ) {
    if (this.scope !== expected) return;
    if (
      !login &&
      expected.identity?.operator_id === identity?.operator_id &&
      expected.identity?.role === identity?.role
    )
      return;

    const dispose = this.disposeScope;
    this.scope = this.createScope(expected.generation + 1, identity);
    dispose();
    this.listeners.forEach((listener) => listener());
  }

  private createScope(
    generation: number,
    identity: Identity | null | undefined,
  ): SessionScope {
    const abort = new AbortController();
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    if (identity !== undefined) queryClient.setQueryData(SESSION_KEY, identity);
    const preparation = createPreparationStore(abort.signal);
    const scope: SessionScope = {
      generation,
      identity,
      queryClient,
      preparation,
      api: createApiClient(abort.signal, () => scope.logout()),
      observe: (identity) => this.change(scope, identity),
      login: (identity) => this.change(scope, identity, true),
      logout: () => this.change(scope, null),
    };
    this.disposeScope = () => {
      abort.abort();
      preparation.clear();
      queryClient.clear();
    };
    return scope;
  }
}
