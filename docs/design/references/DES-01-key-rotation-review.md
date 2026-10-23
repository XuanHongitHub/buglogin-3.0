# DES-01: Review recipients before profile key rotation

Status: draft reference package; owner approval and visual evidence are pending.
Authority: `docs/design/ui-authority.md`, `docs/design/information-architecture.md`, `docs/design/screen-state-matrix.md`, and the read-only pinned Donut source.

## Screen contract

- **Screen:** DES-01 Profiles home, profile action drawer/dialog.
- **Role:** active workspace member who has server-authorized `profile.update` for the selected profile.
- **Primary job:** inspect the exact recipient devices and fingerprints that will receive replacement profile-key envelopes, confirm that snapshot, and run the existing Rust rotation operation.
- **Success:** Rust returns the committed generation and `keyRotated: true`; only then show success and refresh the profile state. A local click or queued request is not success.
- **Authority:** fetch `profile.keyrotation.recipients.list`; submit `profile.keyrotation.execute` with the exact `(memberId, deviceId, fingerprint)` set returned by that snapshot. Rust revalidates the set at commit.

## Donut component references

Read-only sources under `.tmp/donutbrowser-upstream`:

- `src/components/profile-sync-dialog.tsx`: centered `Dialog`, bounded content, descriptive header, scrollable body, persistent footer.
- `src/components/group-management-dialog.tsx`: dense table for member/group records, explicit loading and error states, row semantics.
- `src/components/ui/dialog.tsx`: focus-managed modal primitives and responsive overlay behavior.
- `src/components/ui/table.tsx`, `button.tsx`, and `checkbox.tsx`: shared control geometry and accessible table controls.

BugLogin implementation must reuse the existing typed design-system controls and Donut semantic tokens. Do not copy Donut entitlement checks, sync toggles, account notices, or paid-plan language. Do not introduce new colors, typography, radii, or a second modal system.

## Composition and behavior

Use a bounded centered dialog consistent with Donut's profile operation dialogs. Keep the title, selected profile identity, short security explanation, recipient table, error summary, and footer in that order. The table is read-only: member and device identifiers plus the public-key fingerprint are shown; private keys, tokens, profile data, and envelope bytes are never rendered. Fingerprints use a wrapping monospace field with a copy action only if keyboard and screen-reader labels are present. Require one explicit confirmation for the complete snapshot. The primary action names the operation (for example, “Rotate profile key”); cancel/close is secondary. The footer remains visible while the recipient list scrolls.

The selected profile's owner/permission state comes from API responses. The dialog does not infer ownership from local labels. A changed recipient set invalidates confirmation and requires reloading the snapshot. No recipient is silently skipped. More than one page of recipients is not expected under the API's current 256-member and 64-device-per-member caps; if those caps change, the list must page without truncation.

## State and error mapping

| State | Source | Display behavior |
|---|---|---|
| Loading | recipient-list request pending | Preserve dialog shell; show Donut loading treatment and prevent confirmation. |
| Ready | API recipient snapshot | Show every recipient device and exact fingerprint; require full-set confirmation. |
| No recipients | API returns an empty list | Explain that rotation cannot produce recipient envelopes; do not offer a misleading success action. |
| Forbidden | API `profile_forbidden` / authorization problem | Explain access is unavailable; hide recipient details and close/retry only after permissions change. |
| Rotation not pending | API `sync_key_rotation_not_pending` | Refresh profile data and close the stale flow; no rotation mutation. |
| Snapshot changed | Rust `profile_key_rotation_snapshot_stale` | Clear confirmation, fetch a fresh snapshot, and require review again. |
| Missing device/key | typed recipient/key error | Keep the profile fenced; show the affected member/device identifier if authorized and direct the owner to device enrollment. |
| Offline/API unavailable | canonical retryable problem | Preserve no false progress; keep the snapshot timestamp, disable execution, and offer retry. |
| Rotation running | operation accepted / existing operation status | Show status only from the returned operation; allow dismissal if the durable operation survives. |
| Success | terminal API result | Show committed generation and refresh the profile; never infer success from request completion alone. |

Use the canonical Problem `code`, title, detail, retryability, and correlation ID. Do not show raw backend/debug text or fabricate recipient counts.

## Data and accessibility fixture

Use a sanitized API fixture with a long workspace member name, long device name, and full SHA-256 fingerprint formatting. Include both a single-device member and a multi-device member, plus a list at the current recipient limit in stress layout review. Fixture values are design-only and must never be wired into the production component as fallback data.

The dialog has one `h2`, `aria-labelledby`, labelled recipient table headers, a live status region for loading/result, a programmatic error summary, visible keyboard focus, focus trapping, Escape/cancel handling before submission, and focus restoration to the invoking profile action. Confirmation is a labelled checkbox/button state, never color-only. At 200% zoom, the table may scroll horizontally but member/device identity and the full fingerprint remain reachable.

## Screenshot and interaction gates

Required desktop viewports: V05 (1024×768), V06 (1280×800), V07 (1440×900), V09 (1920×1080), V10 (2560×1440), and V12 (1440×900 reduced motion), per `approved-reference-checklist.md`. Capture ready, long-label, maximum-count, forbidden, offline, stale-snapshot, and success states from real/sanitized API data. Complete keyboard-only, focus-return, 200%-zoom, and screen-reader-label checks before marking this package approved.

## Deviations and approval

- Reuses Donut dialog/table/control behavior while replacing Donut sync/entitlement logic with BugLogin API authorization and device-key rotation contracts.
- Adds a security-sensitive complete-recipient confirmation because the Rust API binds rotation to an exact reviewed snapshot.
- No new shell, visual tokens, or decorative treatment.
- **Approval owner:** product owner. **Approval date/version:** pending.
- **Unresolved:** capture the assigned screenshots and interaction evidence against the actual BugLogin desktop shell, then record owner approval here before production UI implementation.
