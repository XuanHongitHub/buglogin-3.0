# Profile key management delta

## New requirements

### Requirement: Authorized service-managed profile key delivery
The system SHALL keep profile payload data encrypted in object storage and wrap each resource data key using a configured service key-management provider. Rust SHALL deliver a data key only after validating the bearer session and current profile-scoped capability. A valid account session alone SHALL NOT grant access to every profile.

#### Scenario: Authorized profile member opens on a new device
- GIVEN the account has current read and launch access to a profile
- AND the service can unwrap that profile's active data key
- WHEN the desktop requests the key using its authenticated session
- THEN the API returns the key only to the Rust desktop transport
- AND Rust stores it in the operating-system vault
- AND no recovery code or recipient fingerprint is requested

#### Scenario: Access was revoked
- GIVEN the account no longer has current profile read/launch access
- WHEN the desktop requests the profile key
- THEN the API denies delivery and writes a redacted audit event

#### Scenario: Legacy key unavailable
- GIVEN a profile uses a legacy client-managed envelope
- AND no authorized device can provide the legacy data key
- WHEN migration is attempted
- THEN the system reports that legacy ciphertext cannot be recovered
- AND does not replace it with a new key or empty profile

### Requirement: Safe key-provider configuration
The system SHALL fail readiness for hosted profile-key operations when the key provider is absent or invalid. It SHALL NOT use hard-coded, default, or plaintext keys. Key bytes SHALL NOT appear in logs, audit, React, or MCP output.
