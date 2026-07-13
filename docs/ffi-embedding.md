# Future C And FFI Embedding

Gaman's future C ABI will expose direct schema construction and the same typed
command lifecycle used by Rust, WASM, and the CLI. It will not require consumers
to produce SQL, YAML, or JSON schema files.

## Intended Boundary

FFI consumers will use opaque handles for schema, table, column, and entity
builders. Finalizing a builder will produce a prepared Gaman schema or a
structured validation failure. Raw Rust references, generic types, and panics
will never cross the ABI.

Resolved lifecycle work will execute through `MigrationRunner`. A binding may
serialize the versioned command protocol for transport, but schema construction
will remain a direct builder workflow. Storage, tracking, execution, and
inspection will be supplied through opaque handles or callback tables.

Clarification is a suspension rather than a callback into user input. The ABI
will return structured clarification requests, accept explicit decisions, and
retry the same resolved command.

## Stability

Protocol version 2, runner commands, results, failures, diagnostics, and adapter
responsibilities are the implementation contract for a future `gaman-ffi`
crate. The C ABI itself is intentionally deferred until the remaining schema
builder surface is ready to freeze.
