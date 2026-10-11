# Large Generated Migration Fixture

## Contract

- MIG-028 uses fixed seed `2800280028`; repeated runs generate byte-identical paths and content.
- Runtime generation creates exactly 10,000 legacy profiles and 100,000 logical object records.
- Logical objects are stored in 100 deterministic JSONL shards, not 100,000 committed or runtime files.
- The generated tree stays within the file, byte, generation-time, and migration-pipeline budgets pinned by the fixture contract.
- Inspection reports exactly 10,000 profiles and stable source counts/hash.
- Profile mapping and encrypted package output contain exactly 10,000 profile records with a stable JSONL hash.
- Package verification succeeds; source snapshot remains identical before and after the full test pipeline.
- The committed fixture contains only the contract and documentation. Generated data lives under an isolated temporary directory and is removed after the test.

## Scope Ceiling

`ponytail:` MIG-028 proves one deterministic large source shape on the executing OS; add separate soak matrices only when production telemetry defines larger profile payload, proxy, extension, sync, or control-plane distributions.
