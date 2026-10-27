# Server-managed profile keys

## Decision

Replace client-carried recovery-code and per-recipient-device envelope distribution with server-managed envelope encryption. Keep AES-GCM payload encryption and S3 ciphertext storage. The service wraps resource data keys under a separately configured KEK, scoped with workspace/resource/key-version authenticated context. Current implementation injects that KEK using `BUGLOGIN_PROFILE_KEY_ENCRYPTION_KEY_HEX`; it is not a remote KMS adapter. Rust obtains a key only after the API validates the current profile capability and desktop device session, then stores it only in the OS vault.

This deliberately gives up zero-knowledge confidentiality: the service can unwrap keys for authorized operations. Authorization remains profile scoped; a bearer token alone is insufficient. Revocation blocks subsequent key fetches but cannot erase plaintext already downloaded or data keys currently held by an authorized running client.

## Legacy migration

An authorized client that can still decrypt a legacy profile can enroll its existing resource key under the service KEK without changing payload ciphertext when the key version and manifest format permit it. Migration must stage and verify the wrapped envelope before activating the new manifest. If every legacy device key is lost, the ciphertext is unrecoverable. Preserve legacy reads during rollout; never silently mint a replacement key for old ciphertext.

## Rollout gates

1. Configure the deployment KEK from a protected secret store; replace the environment-key adapter with a remote KMS/HSM before production promotion if required by the deployment threat model.
2. Enforce current read/launch ACL, device session, scope binding, rate limits, and redacted audit on key delivery.
3. Migrate desktop Rust and remove recovery-code/fingerprint prompts from normal access.
4. Test legacy compatibility, revoked access, key rotation, restart recovery, and failure rollback.
5. Rehearse against isolated staging; production migration requires snapshot, dry-run, rehearsal, verification, and rollback proof.

## Current constraint

The existing deployment uses S3-compatible object storage and has no verified KMS provider configuration in source. Provider choice and key custody must be explicit; a local environment variable is not evidence of production key management. No production-ready claim is allowed until the hosted provider and rotation procedure are configured and verified.
