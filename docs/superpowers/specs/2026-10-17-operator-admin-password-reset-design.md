# Operator Platform Admin Password Reset

## Scope

Provide a host-only recovery path for a known existing 3.0 platform administrator when browser recovery delivery is unavailable. The operation changes only the selected account's password and revokes its active sessions and API tokens. It does not create an account, change roles, or erase imported control data.

## Contract

- Expose the operation only through a separate operator CLI binary; never add an HTTP, MCP, or desktop command.
- Provide a read-only CLI mode that lists every durable platform-admin account ID, optional username/email, status, and whether its identity record exists, so an operator can identify the intended account without querying the database or exposing credentials.
- Require `BUGLOGIN_ENV=production`, reject `BUGLOGIN_DATA_ROOT`, require an explicit account ID confirmation, and obtain the new password through two hidden terminal prompts.
- Require the target account to exist, be active, have an identity and password record, and be present in the current durable platform-admin aggregate.
- In one durable control transaction, replace the password with Argon2id at the next credential version, revoke all existing sessions/session families and API tokens, and append an audit record containing identifiers/counts only.
- Use revision preconditions for the account, control aggregate, password, sessions, and API tokens. Concurrent changes fail closed without partial mutation.
- Print no password or token; the command is not run as part of this source change.

## Verification

Source coverage proves a successful reset persists the new password, rejects the old password after reopening hosted state, and invalidates an existing session and API token together. The durable-admin, active-account, hidden-prompt, production-environment, and revision-CAS paths have source guards; rejection and conflict cases remain in the consolidated verification pass. No build or test was run as part of this source change.
