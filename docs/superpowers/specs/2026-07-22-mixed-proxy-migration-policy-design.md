# Mixed Proxy Migration Policy

## Scope

Classify every legacy proxy file independently while preserving an atomic migration commit boundary.

## Contract

- Proxy files are processed in normalized path order.
- Each JSON file receives one redacted result: `valid` or `invalid`, source path, optional source ID, optional stable error code.
- Results never contain host credentials, URLs, passwords, tokens, or raw invalid payloads.
- The only approved migration policy is `atomic`: one invalid row blocks package creation and destination activation; valid rows are reported but never partially committed.
- Filesystem, symlink, and unreadable-source failures remain batch-fatal.
- Inspect reports include semantic proxy row failures, not only malformed JSON.
- Source remains immutable after inspect, failed export, and failed import.

## Non-goals

- No `valid-only` migration commit until exclusion-ledger, reconciliation, UI confirmation, and rollback contracts exist.
- No changes to the interactive proxy parser API in this wave.
