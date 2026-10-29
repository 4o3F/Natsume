CREATE TABLE journal_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    occurred_at_ms INTEGER NOT NULL,
    boot_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    category TEXT NOT NULL,
    severity TEXT NOT NULL,
    device_id TEXT,
    session_id TEXT,
    actor_operator_id TEXT,
    request_id TEXT,
    operation_id TEXT,
    trace_id TEXT,
    seat_code TEXT,
    details TEXT NOT NULL
) STRICT;
CREATE INDEX journal_events_device_id ON journal_events(device_id, id) WHERE device_id IS NOT NULL;
CREATE INDEX journal_events_kind_id ON journal_events(kind, id);
CREATE INDEX journal_events_actor_id ON journal_events(actor_operator_id, id) WHERE actor_operator_id IS NOT NULL;
CREATE INDEX journal_events_operation_id ON journal_events(operation_id) WHERE operation_id IS NOT NULL;
CREATE INDEX journal_events_request_id ON journal_events(request_id) WHERE request_id IS NOT NULL;
CREATE INDEX journal_events_occurred_at ON journal_events(occurred_at_ms);
