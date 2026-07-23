# Reset between contests

1. Export non-secret readiness/operation reports and the required audit archive.
2. Issue explicit local-secret clear/unbind workflows and verify terminal results where required.
3. Stop Server writes and take a final encrypted DB backup if retention policy requires it.
4. Destroy or archive the current Server database and Server root key as a pair under policy.
5. Preserve the site `fleet_namespace_uuid`, public/private Control Root custody and public/private Local Origin Root custody.
6. Initialize a new empty database, Server vault root key, per-instance Device Issuing CA and Origin Issuing Intermediate key/CSR.
7. Provision the Server IP-SAN leaf and Origin Intermediate from the two offline site roots, then re-enroll Clients for the new Device CA as required.
8. Import the new single CSV; the first commit freezes the new instance's Seat set.
9. Bind Devices, run explicit state sync, then human-triggered secret sync.

This is a deployment runbook, not an `Event` lifecycle API.
