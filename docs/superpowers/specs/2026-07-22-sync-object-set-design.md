# Sync Object Set Reconciliation

## Contract

- Sync export metadata, manifest, revisions, blobs, and tombstones validate as one set.
- Every live manifest object has exactly one matching blob with equal size and SHA-256.
- Extra, missing, duplicate, or tampered objects block packaging.
- Reconciliation reports exact object and tombstone counts plus canonical checksums.
- Source remains unchanged after successful validation or failed packaging.
