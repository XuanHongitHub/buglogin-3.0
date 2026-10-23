# MIG-028 large-generated

Deterministic fixed-seed scale fixture. The integration test generates 10,000 profile metadata files and 100,000 logical objects packed into 100 JSONL shards at runtime. No generated source tree is committed.

`fixture.json` pins scale, resource ceilings, exact counts, and stable hashes. The test exercises inspection, profile mapping, encrypted package creation, package verification, and source immutability.
