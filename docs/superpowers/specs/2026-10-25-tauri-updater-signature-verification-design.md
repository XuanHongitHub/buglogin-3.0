# Tauri updater artifact signature verification

## Problem

The Windows release workflow only checked that the NSIS `.exe.sig` file existed, and the macOS workflow only checked that the app archive had a neighboring `.sig` file. A malformed or mismatched Tauri updater signature could therefore pass artifact packaging and provenance checks.

## Design

- Verify each Windows and macOS updater payload cryptographically before artifact upload.
- Use the configured Tauri minisign public key only. The private signing key remains confined to the protected signing environment.
- Decode Tauri's base64-wrapped minisign signature, bind its key ID to the configured public key, and verify the Ed25519 signature over the minisign BLAKE2b-512 prehash.
- Keep the check usable on both pinned Windows and macOS runners with the repository's existing Node runtime and built-in crypto module.
- Reject malformed armor, unsupported algorithms, mismatched key IDs, invalid signatures, and modified payload bytes.

## Acceptance

- Unit tests verify a valid signature and reject payload tampering, a different public key, and malformed signature data.
- The downloaded 3.0.8 Windows installer and macOS updater archive verify against the configured public updater key.
- The signing workflow runs the verifier for both platform artifacts before upload.
