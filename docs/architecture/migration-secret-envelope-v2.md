# Migration Secret Envelope v2

Status: implemented, 2026-07-22.

- Proxy credential index: `buglogin.encrypted-proxy-credentials/2`.
- VPN config index: `buglogin.encrypted-vpn-configs/2`.
- Profile/cookie data index: `buglogin.encrypted-profile-data/2`.
- Indexes expose only non-secret binding metadata, ciphertext byte lengths, ciphertext SHA-256, and opaque deterministic object paths.
- Plaintext values, plaintext SHA-256 fingerprints, config SHA-256 fingerprints, and plaintext-derived object-key suffixes are forbidden outside authenticated ciphertext.
- AES-256-GCM authenticates plaintext and binding metadata through object-key AAD. Ciphertext hashes detect package/object substitution before decryption.
- Readers require exact object sets, canonical index ordering/bindings, expected lengths, successful AEAD authentication, and decrypted domain validation.
- Schema v1 secret-bearing indexes are rejected. Production is not released; no compatibility waiver exists for insecure plaintext-fingerprint metadata.
