# DES-ADM-01 Account management

Status: implementation reference approved by the product owner on 2026-10-23

## Screen contract

- Route: Platform Admin → Accounts.
- Roles: only principals whose server-side grants authorize `CAP-ADM-001` read/create/manage. The UI never grants access by role label alone.
- Primary job: find an account, inspect its identity and verification state, create one or many pending accounts, or change an account status.
- Success condition: server confirms each mutation; every row shows the resulting account and verification state.
- Data: real `account` and `account_identity` records. Identity is optional for legacy records and shown as unavailable when absent.

## Composition

Reuse the current Platform Admin shell, Donut-compatible table, existing form controls, badges, buttons, and semantic tokens. Use a compact page header, search/filter toolbar, dense account table, and a single primary “Create accounts” action. The creation panel has Single and Bulk modes. Bulk accepts one `username,email` pair per line, previews the number of valid rows, rejects duplicate emails/usernames in the input before submission, and reports each row as created or failed. Mutations are independent and may partially complete; retry reuses each row's idempotency key.

Rows show username/email, identity verification, account status, and created date. Account IDs stay out of normal labels. Status changes require the current revision and use the existing server authorization/audit path. Pending verification is explicit: no password/session is created and the new account cannot sign in yet.

## States and interactions

- Loading: preserve shell and show table skeleton.
- Empty: explain no accounts exist and offer create if server permits it.
- Filter empty: show a clear-filter action.
- Error/offline: retain current rows, identify that refresh failed, and offer retry.
- Forbidden: show server denial without account data.
- Create validation: associate field errors; bulk line errors include line number and never echo secrets (none are collected).
- Partial bulk: summarize succeeded/failed counts and keep failed rows available for retry.
- Success: refresh from the API; do not infer success from local form state.
- Keyboard: all tabs, filters, rows, and actions are reachable; dialogs return focus to their trigger.

## Deviations and approval

No new shell, palette, component grammar, or motion system. Bulk is a sequence of individually audited/idempotent create operations, not an atomic transaction. The API contract only provisions pending identities; login-ready credentials and email verification remain a separate backend capability and must not be implied by this screen.
