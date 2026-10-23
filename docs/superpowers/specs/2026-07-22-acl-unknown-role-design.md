# ACL Unknown Role Policy

## Contract

- Unknown membership roles stop control-plane mapping.
- Unknown `resourceAccessMode` values stop control-plane mapping.
- Neither case may downgrade to a known role or access mode.
- Invalid ACL data produces no package and no destination mutation.
- Source bytes remain unchanged; errors expose only stable field metadata.
