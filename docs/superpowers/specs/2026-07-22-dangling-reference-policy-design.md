# Dangling Reference Policy

## Contract

- Profile `group_id`, `proxy_id`, `vpn_id`, and `extension_group_id` are optional.
- Missing optional targets are nulled in the neutral record, retained as redacted migration warnings, and do not write a dangling active reference.
- Extension-group member assignments are required; missing extension targets block inspect eligibility and package creation.
- Warnings serialize only source path/field and stable code; no raw payload or secret values.
- Required dangling references produce zero destination writes.
