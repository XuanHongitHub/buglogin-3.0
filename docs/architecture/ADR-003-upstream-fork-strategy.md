# ADR-003: Thin Upstream Fork

- Status: Accepted for v1
- Date: 2026-07-20

## Decision

Use DonutBrowser as the upstream browser core. Track the exact upstream commit and release in a manifest. Prefer upstream modules and behavior; fork only for BugLogin branding, product defaults, supported runtime policy, UX, migration, and explicitly approved business capabilities.

Maintain a small, reviewable patch queue. Each upstream update records source commit, license/attribution review, changed files, compatibility impact, and migration impact. Do not claim feature parity between runtimes; adapters declare capabilities.

AGPL, attribution, trademark, source-distribution, and written-permission obligations remain release gates. BugLogin-specific safety controls must not be removed while removing or changing commercial gates.

## Consequences

- Upstream security and runtime fixes remain adoptable.
- Divergence is visible and bounded.
- Product-specific behavior lives behind domain contracts, not broad engine rewrites.
- Licensing and branding are explicit release dependencies.

## Rejected

- Wholesale port of the legacy application.
- Permanent vendor fork without upstream update discipline.
- Removing upstream attribution or assuming commercial freedom.
