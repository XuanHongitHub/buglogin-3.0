# Duplicate Source ID Policy

- Source IDs must be unique within each entity type.
- Duplicate proxy IDs are detected after deterministic row parsing and before package creation.
- Every conflicting row receives `migration_proxy_duplicate_source_id`.
- Different content never selects a winner; the batch remains atomic and destination writes stay zero.
- Reports contain source paths and stable codes only, never proxy payloads or credentials.
