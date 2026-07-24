
# ADR-0015: Fixed-user Home transaction with deployment-selected backend

## Decision

Use a fixed contest user and versioned immutable Home Template. OverlayFS is the default reset backend; target systems that fail the compatibility probe use a deployment-time fixed staged-copy backend. Runtime failure never silently changes backend.

Every destructive step has a durable transaction journal and boot recovery; uncertain state blocks contest login.
