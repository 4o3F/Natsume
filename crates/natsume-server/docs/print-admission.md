# Print admission implementation notes

The I1 behavior and wire authority is [architecture §23.2.1](../../../docs/architecture.md#2321-i1-受理协议与独立交付); Rust Print DTOs and HTTP annotations generate the public OpenAPI contract. This component admits documents, not physical printing, and creates no printing attempts or automatic cleanup.

## PDF validation boundary

The Server pins `lopdf` 0.45.0 with default features disabled and enables strict loading. Admission checks the parsed cross-reference objects, reference targets, stream lengths, unencrypted status, catalog, complete page tree with parent and count consistency, and inherited media/crop boxes. A4 dimensions allow 0.1 PDF point of numeric rounding and either orientation. Page content streams are decompressed and syntax-parsed; a conservative passive profile rejects actions, annotations, forms, attached files, multimedia and external-file references, including indirect dictionary values and objects inside object streams.

This is structural admission, not complete PDF interpretation, rendering, font or image-codec validation, printer-driver compatibility, black-and-white output enforcement, or physical printing proof. The passive profile can reject PDF features that general-purpose viewers accept; the supported producer is the frozen wrapper template. The parser has its own implementation constraints, including a 100-level direct-object nesting budget. No application task-count or page-count cap is introduced.

The 8 MiB limit bounds the uploaded compressed PDF only. Parsing and decompression run in-process on a blocking thread, with no independent memory or CPU isolation and no Server body-read timeout. Compressed streams can consume substantially more memory than the upload size. Dedicated source credentials reduce who can reach this boundary but do not turn parsing into a sandbox. Parser diagnostics, PDF bytes and source credentials are not included in HTTP error responses or the configured Server log targets.

## Persistence and recovery

The additive `00000000000007_print` migration preserves existing tables. Final PDF bytes, digest, page count, global ready sequence and first admission counts commit in one SQLite write transaction. A caller cancelled after database work starts must query its original source/request key rather than assume rollback. Ready results and registration display snapshots survive roster removal; a preparing upload must still match its original account identity. There is no separate document directory, filesystem configuration or cleanup job.

Backups must use a consistent SQLite backup procedure that includes committed WAL state, together with the existing deployment's other required state and secrets. Copying only the live database file is not a consistent backup. Database storage grows with admitted PDFs; capacity monitoring and an operator-approved retention policy remain deployment responsibilities.
