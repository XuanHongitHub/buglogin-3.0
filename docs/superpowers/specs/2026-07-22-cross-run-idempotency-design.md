# Cross-Run Migration Idempotency

- Importing the exact package under a new run ID reuses matching destination entities and profile bytes.
- The second run creates zero entities, zero profile roots, and no revision changes.
- Rolling back the reuse-only run removes nothing owned by the first run.
- Rolling back the creating run quarantines its owned profile exactly once.
