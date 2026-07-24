
# ADR-0011: Application-encrypted SQLite vaults

## Decision

Server and Client store confidential persistent payloads as per-record AEAD ciphertext in their SQLite databases. Each side has a random 32-byte file root key protected by owner/mode and never injected through systemd credentials, environment variables or command-line arguments.

Client derives its vault master key with HKDF using the immutable Machine Hardware ID as salt. The Machine ID is binding material, not entropy. A separate identity file is validated before any decryption attempt.
