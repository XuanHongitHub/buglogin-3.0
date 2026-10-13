# Sync Lease Heartbeat Test Stability

## Scope

Make the API sync-import lease-heartbeat test independent of wall-clock scheduler timing while preserving the behavior it verifies.

## Behavior

- The test waits for two actual renewal callbacks using synchronization, not a short fixed sleep.
- It stops the heartbeat, records the renewal count, and verifies no callback occurs after shutdown.
- Production heartbeat interval, failure handling, and lease semantics do not change.

## Evidence

Run the focused heartbeat test repeatedly, then run the full `buglogin-api` test suite.
