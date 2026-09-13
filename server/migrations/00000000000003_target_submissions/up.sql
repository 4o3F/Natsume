-- HTTP submission deduplication only; no delivery or Client execution state.
-- No cascading foreign keys: logout/revocation must never make an ID executable again.
CREATE TABLE target_submission_receipts (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operator_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    results_json TEXT NOT NULL
) STRICT;
