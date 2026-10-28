# Pin prepared desktop updates to their checked release

## Problem

The desktop stores only the downloaded bytes and version. On restart it checks
the mutable stable `latest.json` again, then requires that feed to still name
the prepared version. A stable promotion between download and restart therefore
rejects a valid package with `app_update_release_changed`.

## Decision

Keep the Tauri `Update` value that supplied the downloaded bytes alongside those
bytes in process memory. Tauri's value carries the exact immutable artifact URL,
version, and detached signature. On restart, verify and install through that
same value; do not query the mutable latest feed again. The updater's pinned
public key and signature verification remain mandatory, and app shutdown still
happens only after verification succeeds.

## Boundaries

- Do not weaken or bypass Tauri signature or signed-version verification.
- Do not change the feed format, release URLs, or stable promotion process.
- Keep prepared update state in memory, matching current behavior.
- Preserve the prepared update if shutdown or installation fails.
- Keep this fix scoped to a release built from the current 3.0.23 source; it
  cannot alter an already installed 3.0.20 binary.

## Acceptance

- Restart installs the exact release object used to download the prepared
  bytes, even if stable `latest.json` advances afterward.
- Signature verification still occurs before shutdown and again at install.
- Existing prepared-version mismatch and shutdown-order behavior remain intact.
