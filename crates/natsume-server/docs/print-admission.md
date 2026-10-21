# Print admission implementation notes

The I1 behavior and wire authority is [architecture §23.2.1](../../../docs/architecture.md#2321-i1-受理协议与独立交付); Rust Print HTTP DTOs define the JSON fields and generate TypeScript bindings through Specta; actual HTTP tests cover routing, authentication and responses. This component admits documents, not physical printing, and creates no printing attempts or automatic cleanup.

## PDF validation boundary

The Server pins `lopdf` 0.45.0 with default features disabled and enables strict loading. Admission checks the parsed cross-reference objects, reference targets, stream lengths, unencrypted status, catalog, complete page tree with parent and count consistency, and inherited media/crop boxes. A4 dimensions allow 0.1 PDF point of numeric rounding and either orientation. Page content streams are decompressed and syntax-parsed; a conservative passive profile rejects actions, annotations, forms, attached files, multimedia and external-file references, including indirect dictionary values and objects inside object streams.

This is structural admission, not complete PDF interpretation, rendering, font or image-codec validation, printer-driver compatibility, black-and-white output enforcement, or physical printing proof. The passive profile can reject PDF features that general-purpose viewers accept; the supported producer is the frozen wrapper template. The parser has its own implementation constraints, including a 100-level direct-object nesting budget. No application task-count or page-count cap is introduced.

The 8 MiB limit bounds the uploaded compressed PDF only. Parsing and decompression run in-process on a blocking thread, with no independent memory or CPU isolation and no Server body-read timeout. Compressed streams can consume substantially more memory than the upload size. Dedicated source credentials reduce who can reach this boundary but do not turn parsing into a sandbox. Parser diagnostics, PDF bytes and source credentials are not included in HTTP error responses or the configured Server log targets.

## Persistence and recovery

Set the required absolute `[storage].print_documents` directory in the deployment configuration. The package provisions `/var/lib/natsume-server/print-documents` as `natsume-server:natsume-server` mode `0700`. For another location, provision a protected parent directory, grant the service user write/traversal access, and adjust the systemd write allowlist if necessary. The store creates its leaf directory when absent and rejects a symlink, unsafe owner, or group/world-writable leaf directory. Protect ancestor directories against replacement; the leaf check is not a filesystem sandbox.

Admission parses the PDF and writes it outside the SQLite write transaction. A mode-`0600` temporary file in the document directory is synced, published without replacing an existing `<sha256>.pdf`, and followed by a directory sync. Existing files must match the exact bytes. Only then does a short SQLite transaction store digest, byte count, page count, global ready sequence and first admission counts. The `00000000000007_print` migration creates the document metadata table without a PDF BLOB column. Identical documents may share a file, but requests remain independently queued; this is not request deduplication.

A file failure leaves the request preparing. Database rollback can leave a durable file without a database reference; retries reuse it, and startup never turns it into a task. Process termination may also leave temporary files. There is no online deletion or automatic cleanup job: removing unreferenced files requires a separate approved maintenance procedure with the Server stopped. A cancelled caller must query its original source/request key rather than assume rollback. Ready results and display snapshots survive roster removal; a preparing upload must still match its original account identity.

Startup checks the size and SHA-256 of every referenced file before exposing HTTP, and ready queries or registration replays also verify the file outside database transactions. Missing, symlinked or corrupt files are errors, not successful ready responses. Startup verification reads the full retained document set, so its duration grows with stored PDF volume.

## Backup and recovery

Stop the only Server process for the entire backup window. Capture a consistent SQLite backup including committed WAL state, the complete configured document directory, configuration and existing required keys/secrets before restarting. Copying only the live database file is not a consistent backup, and a database-only backup no longer contains the PDFs. Restore the database and document directory from the same backup set; startup verification does not repair missing documents. See the [deployment backup procedure](../../../docs/operations-deployment.zh-CN.md#82-server-冷备份).

Capacity monitoring, physical printing, document retention and backup/restore performance acceptance remain deployment responsibilities. File-backed document storage does not bound PDF parser memory or remove the current metadata aggregation cost when taking FIFO snapshots.

## Verification

- `cargo test --frozen -p natsume-server --lib component::print`: 20 tests passed, including file-write failure, database rollback, concurrent admission, cancellation, missing/corrupt files, and database-plus-document restore.
- `cargo test --frozen -p natsume-server --test schema_contract`: 14 tests passed, covering metadata-only storage, preserved constraints and migration round trips.
- After `deno task prepare rust print-domjudge`, `cargo test --frozen -p natsume-server --lib http::tests::print_domjudge -- --ignored`: the real Typst/HTTPS test passed, including replay, hidden responses, database reopen and source revocation; it reads the admitted PDF from the file store.
- `deno task schema:check`: passed; `diesel/schema.rs` is generated from migrations 01–07 and contains only document metadata, not PDF bytes.
- `prek run --all-files`: passed with no hooks skipped, including workspace Clippy and the full non-ignored workspace Rust tests. The builder used the prepared pinned Skia archive.

These checks do not simulate power loss, exercise production backup tooling on a deployed Server, or establish sustained admission throughput and disk-capacity limits.
