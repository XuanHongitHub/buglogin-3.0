# Owner Bootstrap Environment Guard

## Scope

The one-shot production owner bootstrap CLI must refuse to connect unless the operator explicitly selects `BUGLOGIN_ENV=production`. It must also reject `BUGLOGIN_DATA_ROOT`, which denotes local repository mode. This prevents an operator from creating credentials in a development or accidental local store while believing production was initialized.

## Contract

- The CLI accepts only the exact environment value `BUGLOGIN_ENV=production`.
- The CLI refuses to proceed if `BUGLOGIN_DATA_ROOT` is present, even when empty.
- Validation happens before prompting for a password or opening PostgreSQL.
- Existing PostgreSQL TLS validation, hidden password input, one-time bootstrap transaction, and output redaction remain authoritative.
- This guard does not reset, delete, or migrate existing identities and does not make bootstrap available through HTTP.

## Verification

Add unit coverage for the accepted production configuration and rejected missing, local, unknown, and local-data-root configurations. Execute this coverage during the requested consolidated verification pass.
