ALTER TABLE devices ADD COLUMN client_ip TEXT;
ALTER TABLE devices ADD COLUMN server_observed_ip TEXT;
ALTER TABLE devices ADD COLUMN ip_observed_at_unix_ms INTEGER;
