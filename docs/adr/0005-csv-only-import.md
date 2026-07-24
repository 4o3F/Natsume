
# ADR-0005: One authoritative UTF-8 CSV

## Decision

Each import accepts exactly one UTF-8 CSV (optional BOM) with exact header `seat,account,password`. The file is a complete snapshot keyed by immutable Seat label. The first successful commit creates and freezes the Seat universe; every later file must contain exactly the same Seat set. Re-import creates assignment/password revisions or no-ops; commit is atomic and never triggers Device synchronization.

## Consequences

There is no `ImportSource`, multi-file workspace, column mapping, XLSX/ODS adapter, delimiter guessing, legacy encoding or DOMjudge credential-file export. Password staging is AEAD-encrypted and preview is masked.
