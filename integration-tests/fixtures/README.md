
# Fixtures

Fixtures must be non-secret and reproducible: sanitized hardware evidence, CSV edge cases, Protobuf golden frames, certificates from test-only CAs, DOMjudge HTTP traces, journal crash points and package/OS snapshots. `client.preseed` uses an IANA documentation endpoint and must be replaced with the registered lab endpoint for target-VM evidence. Never commit real passwords, root keys, private keys or raw hardware serials.
