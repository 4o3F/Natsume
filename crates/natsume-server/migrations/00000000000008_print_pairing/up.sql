CREATE TABLE print_printers (
    printer_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    pairing_code_hash BLOB UNIQUE,
    pairing_expires_at_unix_ms INTEGER,
    client_id TEXT UNIQUE,
    client_token_hash BLOB,
    client_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (client_confirmed IN (0, 1)),
    CHECK ((pairing_code_hash IS NULL) = (pairing_expires_at_unix_ms IS NULL)),
    CHECK ((client_id IS NULL) = (client_token_hash IS NULL)),
    CHECK (client_id IS NOT NULL OR client_confirmed = 0),
    CHECK (client_id IS NULL OR pairing_code_hash IS NULL)
) STRICT;
