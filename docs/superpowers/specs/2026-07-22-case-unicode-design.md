# Case and Unicode Path Policy

## Contract

- Snapshot paths preserve original UTF-8 spelling for evidence and hashes.
- Collision comparison uses Unicode NFC plus lowercase canonical keys.
- Two distinct source paths with the same canonical key block migration.
- Filesystems that alias the pair satisfy the same safety outcome by preventing two entries.
- Vietnamese profile names remain unchanged; deterministic IDs never derive from display-name casing.
