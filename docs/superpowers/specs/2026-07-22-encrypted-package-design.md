# Encrypted Migration Package

## Contract

- Production export reads exactly 32 key bytes from the native OS vault; plaintext passwords and environment keys remain unsupported.
- Profile payloads use authenticated AES-256-GCM chunks bound to deterministic object paths.
- Package files, indexes, reports, errors, and debug output contain no plaintext profile payload.
- A wrong key or modified ciphertext fails authentication before destination creation or mutation.
- A correct key restores exact profile bytes; source bytes remain unchanged.
