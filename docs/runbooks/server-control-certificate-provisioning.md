# Server trust and issuing-certificate provisioning

1. Verify the immutable site `fleet_namespace_uuid`, public Control Root and public Local Origin Root against deployment records.
2. Install the Server package and verify ownership/modes under `/var/lib/natsume-server`.
3. Run `natsume-server init --server-ip <canonical-ip>`; retain the Server-control CSR, Origin-Intermediate CSR and SPKI fingerprints, never a private-key export.
4. Sign the Server CSR with the offline site Control Trust Root using the frozen Server profile and exact IP SAN.
5. Sign the Origin-Intermediate CSR with the offline site Local Origin Root using the frozen CA profile.
6. Import both chains; require chain, EKU/BasicConstraints, KeyUsage, IP SAN where applicable and SPKI match.
7. Confirm the per-instance Device Issuing CA exists inside the encrypted Server vault.
8. Start the service and verify HTTPS over TCP and QUIC over UDP on the configured numeric port.
9. Confirm a Client package containing only the public roots validates Server and Gateway chains without TOFU or a dangerous verifier.
