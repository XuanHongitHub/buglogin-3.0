# Synthetic E2E Fixtures

`manifest.json` is deterministic, sanitized, and synthetic. It contains no production exports, `.tmp` data, real domains, or reusable credentials.

```text
node tests/e2e/support/verify-fixtures.mjs
node tests/e2e/support/verify-fixtures.mjs --services
pnpm --filter @buglogin/e2e smoke:core
```

The `--services` command fails explicitly until the E2E stack is running.
`smoke:core` uses Node's built-in test runner. It validates the synthetic account/workspace/profile/proxy/migration/MCP/audit vertical contract without a browser or live services.
