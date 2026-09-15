CREATE TABLE device_power_targets (
    device_id TEXT PRIMARY KEY REFERENCES devices(device_id) ON DELETE CASCADE,
    shutdown_epoch INTEGER,
    expires_at_unix_ms INTEGER
) STRICT;
