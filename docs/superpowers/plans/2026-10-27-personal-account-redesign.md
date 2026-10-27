# Personal Account Page Redesign Plan

Status: implementation in progress; code changes are local and not released. Visual QA against an authenticated desktop session remains pending.

## Goal

Redesign the desktop **Tài khoản của tôi** page so identity, password security, and session management are easy to scan and operate. Keep the existing BugLogin/Donut visual language and backend permissions. Do not redesign the organization account directory as part of this task.

## Evidence and current problems

- The screenshot shows identity, password, and all sessions stacked together on one long page.
- The current-password field appears twice: once in the identity form and again in the password form.
- Session history is rendered inline with active and revoked sessions mixed together; 21 rows make the page long and obscure the current session.
- Session rows use only `kind`, `createdAt`, `expiresAt`, `revokedAt`, `current`, and an optional `deviceId`. The UI must not turn an ID into a device/person name or invent location/IP details.
- The canonical personal-account specification already calls for separate **Thông tin cá nhân / Bảo mật / Phiên đăng nhập** views. The current implementation does not follow that composition.

## Binding references

- `docs/design/ui-authority.md`
- `docs/design/information-architecture.md` — ACC-07, ACC-08, ACC-09
- `docs/design/screen-state-matrix.md` — account profile, security, sessions/device states
- `docs/superpowers/specs/2026-10-26-management-ui-ux-design.md` — §13, “Tài khoản của tôi”
- `DESIGN.md` and the pinned Donut source/theme manifest for desktop typography, controls, spacing, icons, focus, and density

The existing specification is the design authority; this plan does not invent a new shell or visual system.

## Proposed desktop wireframe

Keep the current desktop shell and content width. The account page is a focused settings surface with a compact header and three separate views; it does not place every form and session row into one long page.

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ BugLogin / Tài khoản                                                        │
│ Tài khoản của tôi                                          [Làm mới]        │
│ Quản lý thông tin đăng nhập và các phiên của bạn.                           │
├──────────────────────────────────────────────────────────────────────────────┤
│ [Thông tin cá nhân]  [Bảo mật]  [Phiên đăng nhập  ·  1 đang hoạt động]       │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  THÔNG TIN CÁ NHÂN                                                           │
│  Cập nhật định danh của bạn. Quyền và trạng thái tài khoản do policy quản lý.│
│                                                                              │
│  Tên đăng nhập                         Tên hiển thị                           │
│  [admin........................]       [...............................]     │
│                                                                              │
│  Email                                                                        │
│  [admin@buglogin.com........................................] [Đã xác minh]  │
│                                                                              │
│  Nếu thay đổi cần xác thực lại, yêu cầu xác thực tại bước lưu.               │
│  Lỗi theo trường / trạng thái lưu                                            │
│  [Lưu thay đổi]                                                              │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ BugLogin / Tài khoản                                                        │
│ Tài khoản của tôi                                          [Làm mới]        │
│ [Thông tin cá nhân]  [Bảo mật]  [Phiên đăng nhập]                            │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  BẢO MẬT                                                                     │
│  Đổi mật khẩu                                                                │
│                                                                              │
│  Mật khẩu hiện tại                                                           │
│  [................................................................]         │
│                                                                              │
│  Mật khẩu mới                            Xác nhận mật khẩu mới                │
│  [................................]      [................................]  │
│  Dùng mật khẩu đủ mạnh theo policy.                                          │
│                                                                              │
│  Đổi mật khẩu sẽ đăng xuất các phiên và thu hồi token theo kết quả API.      │
│  [Đổi mật khẩu]                                                              │
│                                                                              │
│  Đăng xuất tài khoản khỏi ứng dụng này                              [Đăng xuất]│
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ BugLogin / Tài khoản                                                        │
│ Tài khoản của tôi                                          [Làm mới]        │
│ [Thông tin cá nhân]  [Bảo mật]  [Phiên đăng nhập  ·  1 đang hoạt động]       │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  PHIÊN ĐĂNG NHẬP                                    [Đăng xuất phiên khác]* │
│  Chỉ hiện dữ liệu thiết bị mà hệ thống xác minh được.                       │
│                                                                              │
│  ĐANG HOẠT ĐỘNG                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │ Phiên hiện tại · Ứng dụng desktop       Tạo lúc … · Hết hạn …          │  │
│  │ Thiết bị chưa có tên xác minh                         [Phiên hiện tại] │  │
│  ├──────────────────────────────────────────────────────────────────────┤  │
│  │ Ứng dụng desktop                       Tạo lúc … · Hết hạn …          │  │
│  │ Thiết bị chưa có tên xác minh                  [Đăng xuất phiên này] │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
│  ĐÃ THU HỒI  (20)                                         [Hiện lịch sử ▾]  │
│  Không chiếm chỗ trong danh sách phiên đang hoạt động.                      │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

`*` Render “Đăng xuất phiên khác” only if an authorized backend command exists and returns a real result. Otherwise omit it; do not simulate the action with a frontend loop.

Session history expanded state:

```text
│  ĐÃ THU HỒI  (20)                                         [Ẩn lịch sử ▴]   │
│  Ứng dụng desktop             Thu hồi lúc …                                  │
│  Ứng dụng desktop             Thu hồi lúc …                                  │
│  …                                                                            │
│  [Các phiên cũ hơn]  (only if the API supports bounded paging)               │
```

Compact-width behavior: tabs may scroll horizontally or wrap using the existing tab primitive; fields become one column; action buttons stay reachable; the session action may move to a row menu. Do not shrink text or introduce a second card grid to force the wide layout onto a narrow window.

Error/loading behavior stays inside the selected view: keep the page title and tabs stable, show shape-matched skeletons while loading, preserve unsaved values on error, and put retry beside the failed data/action. A failed session refresh must not be shown as “no sessions.”

## Planned code locations

### Primary desktop UI

- `src/components/management/personal-account.tsx`
  - Replace the single stacked form/list with three distinct account views/tabs matching §13 of the canonical spec.
  - Keep one responsive, internally scrolling content region inside the desktop shell.
  - Remove the always-visible duplicate current-password input from profile editing. If backend policy requires re-authentication for a specific identity change, request it only in that sensitive-change flow and preserve the server as authority.
  - Keep password change as one focused form with current password, new password, confirmation, validation, pending state, and the actual session-revocation consequence.
  - Put the current session first. Show active sessions by default and place revoked history behind a clear filter/disclosure so old sessions do not dominate the page. Use only verified display fields; otherwise use a localized generic type label.
  - Make single-session revoke and sign-out actions explicit and prevent duplicate submissions. Keep “sign out this app” distinct from revoking other sessions.

- `src/app/desktop-home.tsx`
  - Keep the existing account route/navigation entry and integrate the redesigned view without changing unrelated navigation or the account-management directory.

- `src/components/management/accounts-i18n.ts`
  - Add/update Vietnamese and English labels for the three views, active/revoked grouping, device-unavailable state, last-used metadata if available, sign-out-current/other-session actions, confirmation, and error/retry messages.
  - Preserve keys still used by `accounts.tsx`; do not rename shared strings without migrating every consumer.

### Existing desktop transport and conditional contract work

- `apps/desktop/src/bridge.ts`
  - Reuse `getOwnAccountIdentity`, `updateOwnAccountIdentity`, `changeOwnAccountPassword`, `listAccountSessions`, and `revokeAccountSession` where they satisfy the designed flows.

- `apps/desktop/src-tauri/src/lib.rs` and `apps/desktop/src-tauri/src/remote_api.rs`
  - Change only if implementation proves the existing session/identity commands cannot support the approved interactions.

- `crates/api/src/http/mod.rs`
  - Review the account identity and session list/revoke handlers before changing any UI assumptions.
  - Add server-side session pagination or verified device metadata only if needed for real account volumes and only when the source data is trustworthy. Never infer a device name from `deviceId` or IP.

- `crates/api/src/commands/registry.rs` and the command-contract source/generator
  - Update only if a genuine API contract change is approved. Regenerate `frontend/src/generated/command-registry.ts`; never hand-edit generated output.

## Interaction decisions

1. **Thông tin cá nhân:** username, display name, email and verification state; editable fields follow backend policy. Save result stays visible in this view.
2. **Bảo mật:** one password-change flow. Explain accurately which sessions/tokens are revoked after success; refresh session state from the API rather than fabricating a result.
3. **Phiên đăng nhập:** current session is unmistakable; active sessions are the primary list; revoked sessions are available as history. Support a single revoke and, if the existing API authorizes it, revoke-other-sessions with a consequence summary. Do not expose raw IDs, unverified device labels, or guessed location/IP.
4. **Sign out:** preserve the existing logout operation and make its progress/failure visible. Signing out must not be presented as the same action as revoking a different session.

## Required UI states and behavior

- Initial loading and refresh without a layout jump.
- Profile/session empty states with a useful explanation.
- Per-field validation and server-side rejection while preserving entered values.
- Saving/revoking progress with duplicate-submit protection.
- Success only after the API confirms persistence/revocation.
- Partial or failed session revocation with the exact affected-session result and retry path.
- Offline/degraded state that retains safe last-known display data but disables unsupported remote mutations with a clear reason.
- Forbidden/auth-expired state with a recovery action; no blank panel.
- Keyboard navigation, visible focus, accessible tab semantics, and Escape behavior for any confirmation dialog.
- Responsive desktop window sizes; no page-wide scrollbar escaping the app shell.

## Verification plan for implementation phase

- Inspect the pinned Donut account/settings patterns and shared semantic tokens before editing.
- Use representative account data with one current session, multiple active sessions, many revoked sessions, missing verified device names, and long email/display-name values.
- Verify profile save, password change, one-session revoke, revoke-other-sessions if supported, and logout against the real API commands.
- Capture screenshots and interaction evidence at the supported desktop sizes; check keyboard-only operation and accessibility labels.
- Run the account component/bridge checks plus desktop typecheck/build and targeted Rust/API checks only if backend contracts change.
- Do not release until the visual/state evidence matches this spec and existing account/security behaviors still pass.

## Scope boundary

- This is the **desktop self-service account page** shown in the supplied screenshot.
- `src/components/management/accounts.tsx` is the organization/workspace account directory and is out of scope unless implementation finds a concrete shared-component regression.
- `frontend/src/apps/web/account/index.tsx` is a separate web account surface and is out of scope for this desktop redesign; parity can be planned separately.
- No account/security API changes are presumed. Any required contract addition must be justified by the existing backend data and permissions first.

## Release sequencing

- Wait until the currently active release finishes.
- Reconcile the exact current `main` commit and dirty worktree before implementation; preserve other session work.
- Implement and verify the desktop account page in the next release cycle. Keep any contract/API change in the same reviewed vertical slice only if the approved UX cannot work with the existing contract.

## Implementation progress — 2026-10-27

- Replaced the stacked account form/session list with the approved profile, security, and sessions tabs.
- Removed the always-visible duplicate current-password field. It appears only after username/email changes, matching the server's re-authentication rule; display-name edits do not request it.
- Active sessions are listed first with the current session sorted to the top. Revoked sessions are behind a native, keyboard-operable disclosure.
- Added bilingual tab/session labels and kept load errors retryable without attaching a reload action to mutation errors (so unsaved form values are not discarded).
- `pnpm --dir apps/desktop typecheck`, `pnpm --dir apps/desktop test`, `pnpm --dir apps/desktop build`, and focused Biome checks passed. The repository-wide `pnpm exec tsc --noEmit` still reports existing errors in unrelated Cookie Bot, sync, bookmarks, and profile modules.
- Re-ran desktop typecheck and Vite production build on the current combined worktree; both pass. The build continues to emit the known `use client`, source-map, mixed-import, and large-chunk warnings.
- Added bridge regression coverage that asserts identity-save uses `account_identity_update` and emits a UUID-shaped idempotency key. Added a Rust HTTP transport parity case that asserts the identity PATCH preserves its body and sends the idempotency header. These checks validate the current source path; they do not establish the cause of the `3.0.19` screenshot error or replace authenticated desktop/API interaction QA.
- Identity and session loading now settle independently. An identity endpoint failure no longer suppresses a successful session snapshot, and a session endpoint failure is rendered with a retry instead of appearing as an authoritative zero-session empty state. Session count stays unavailable until the endpoint returns a successful list. Password changes refresh the server-confirmed session snapshot rather than clearing it locally.
- The Rust identity transport regression test passes on the current source. The `Valid idempotency key required` screenshot has not been reproduced against the current source/API path, so its original runtime cause remains unproven; authenticated QA on the rebuilt app is still required.
- A desktop dev server rendered the sign-in page; authenticated account interactions/screenshots have not yet been verified. Keep this item in progress until that QA is done.
- Device-session issuance now commits a hashed account/device head revision with the session and prior-family revocations. This CAS closes the multi-API-instance race where simultaneous sign-ins could both observe no active session for the same stable device ID. Conflicts caused by another device-head update are retried up to three attempts; unrelated stale authorization/device-code conflicts retain their original error. The focused head and session-replacement tests pass, and the full API library suite passes 279/279. This does not prove the origin of historical production rows; installed-app and live multi-instance evidence remain pending.
