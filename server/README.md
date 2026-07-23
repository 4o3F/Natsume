# Server

`natsume-server` owns the single-contest domain, SQLite migrations, application-encrypted vault, HTTPS management/Device-only Enrollment, Device and Origin PKI authorities, mandatory-mTLS QUIC gateway, target calculator, dispatcher, audit and SSE.

The Server has no Event/phase model. Import is one `seat,account,password` CSV. Target state is inert; Device effects require explicit Commands, and password distribution requires a human-triggered `SYNC_SECRET`.

Enrollment signs only the Daemon Device Identity certificate. Gateway certificate issuance is accepted only as a request on an authenticated QUIC session while the same Device is executing a matching, unexpired `SYNC_STATE`; SAN/profile/validity are derived from the frozen command snapshot.
