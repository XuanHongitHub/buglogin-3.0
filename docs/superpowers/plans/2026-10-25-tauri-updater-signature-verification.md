# Tauri updater signature verification plan

Status: implementation and local candidate verification complete; signed workflow rerun and OS install/update acceptance remain open.

- [x] Reproduce the gap: workflow checked `.sig` existence; downloaded 3.0.8 artifacts passed checksum/provenance but had no cryptographic signature gate.
- [x] Add a small cross-platform minisign verifier using the configured public key only; it verifies both the artifact signature and trusted-comment signature.
- [x] Add regression tests for valid signatures, changed artifact bytes, mismatched key IDs/public keys, changed trusted comments, and malformed input.
- [x] Run the verifier against the Windows and macOS 3.0.8 candidate artifacts and add it to both signing jobs.
- [x] Run focused release tests and `pnpm release:verify`; downloaded candidate bundles pass checksum, SBOM, and provenance verification.
- [ ] Push the reviewed source and rerun Windows/macOS signing so the new CI verifier is exercised on the next immutable candidate.
- [ ] Complete installed-app launch/update acceptance on native Windows and macOS devices before stable promotion.

The existing 3.0.8 candidate's notarization, SBOM, checksum, provenance, and attestation evidence belongs to source SHA `a250ec7b0ba49f6822053c0b3e1e45db1e7b4c7a`. The current worktree contains later changes, so that candidate must not be represented as containing this verifier or the uncommitted source changes. Stable feed remains on 3.0.7.
