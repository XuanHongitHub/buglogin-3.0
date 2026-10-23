# Windows Unsigned Build Smoke — 2026-07-22

Status: development evidence only; not release acceptance.

## Environment

- OS: Windows x86_64 development host.
- Build target: `D:\buglogin-build`.
- Configuration: no `BUGLOGIN_DEPLOYMENT_MODE` or Wayfern runtime variables supplied.

## Result

- `cargo test -p buglogin-desktop-app --lib`: 38 passed.
- `pnpm --dir apps/desktop tauri:build`: MSI and NSIS bundles produced.
- Release executable remained alive after 8 seconds instead of exiting with code 101.
- Missing deployment/runtime configuration enters `DesktopState::Unavailable`; commands fail closed and no local or hosted authority is opened.

## Artifact Evidence

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `D:\buglogin-build\release\buglogin-desktop-app.exe` | 16,473,600 | `9571f58657cab6df1364a6c28ccd876fa6b445f52573c7cdea10b9b793f958bf` |
| `D:\buglogin-build\release\bundle\msi\BugLogin_0.1.0_x64_en-US.msi` | 5,873,664 | `82a857633aef135ec0278be0a1f738f54360e969ada427c346a90605bc6fe876` |
| `D:\buglogin-build\release\bundle\nsis\BugLogin_0.1.0_x64-setup.exe` | 4,037,681 | `f70637de2992d3ff142689c407b9a8c36f7d5a52aaa3b98b759b2b1bc869f3db` |

## Not Proven

- Authenticode, timestamp, SmartScreen, WebView2 install/update behavior.
- MSI/NSIS install, repair, upgrade, uninstall, and keep/delete-data choices.
- Real signed Wayfern launch, profile lifecycle, proxy egress, sync interruption, or migration cutover.
- macOS build, signing, notarization, stapling, or native interaction.

This evidence cannot satisfy `E2E-INST-001` or any signed RC gate.
