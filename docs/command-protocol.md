# Command Protocol

Gaman exposes one portable command contract for CLI resolution, WASM, and
other host integrations. The protocol carries resolved lifecycle intent; it does
not carry paths, environment variables, database connections, terminal state,
or prompt callbacks.

## Layers

```text
argh tokens
  -> host resolution
  -> CommandEnvelope
  -> MigrationRunner::run_command(&Command)
  -> CommandResponse | CommandFailure
```

`CommandArgs` remains the authoritative textual grammar and help source.
`Command` is the portable typed lifecycle request. `MigrationRunner` executes
that request through caller-owned storage, tracking, execution, and inspection
adapters.

## Versioning

`COMMAND_PROTOCOL_VERSION` identifies the request and response shape. Protocol
version 2 is Gaman's frozen host boundary. Hosts must reject versions they do
not support before executing a command.

```json
{
  "protocol_version": 2,
  "command": {
    "command": "status",
    "arguments": {
      "reverse": false,
      "search": null
    }
  }
}
```

Schema and low-level engine APIs may still change before Gaman 0.5. The version
2 command envelope, response, failure, diagnostic codes, and clarification
contract are stable: an incompatible transport change requires a new protocol
version.

## Results

`CommandResponse` contains a typed `CommandResult`. Hosts decide how to present
it. Terminal wording, JSON layout convenience, process exit status, and output
destinations are not runner responsibilities.

Migration results include the filename-derived migration ID even though normal
migration YAML intentionally omits that field.

## Failures

`CommandFailure` contains:

- the protocol version;
- a stable `DiagnosticCode`;
- a concise summary;
- ordered, sanitized details and an optional actionable hint;
- whether host input can make the request retryable;
- typed clarification requests when decisions are required.

Storage, tracking, execution, inspection, parsing, invalid-command, and
migration lifecycle failures remain distinguishable. Native driver and
filesystem error types never appear in the protocol.

The normal diagnostic must identify the failed Gaman action and retain useful
context such as a migration ID, operation ordinal, entity identity, or SQL
source location. Hosts may present `details` and `hint` directly. Native CLI
verbose mode may additionally show a sanitized internal cause chain; that
chain is not part of the protocol payload.

## Clarification Retry

The runner never prompts. When migration generation requires input:

```text
run &command
-> CommandError::NeedsInput
-> host collects Decision values
-> command.with_decisions(decisions)
-> run &retry
```

Decisions cannot be attached to commands that do not support clarification.

## WASM

WASM exposes structured command requests and exact token arrays. Token arrays
preserve spaces and quoting already resolved by the JavaScript host. Gaman does
not split command-line strings in browser bindings.
