# Donut 0.33 desktop source sync

## Goal

Bring the Donut 0.33 desktop UI and interaction baseline into BugLogin while keeping BugLogin's Rust/API/auth/workspace/sync authority and product branding intact.

## Source and boundaries

- Read-only source checkout: `F:\Herd\buglogin-3.0`, HEAD `89f44466a7d4e56150dabc54528dc6300c292eda` (`v0.33.0`, 2026-10-09).
- The source checkout has uncommitted changes. Treat its working tree as reference only; do not edit or clean it. Review source changes before importing them.
- The desktop entrypoint remains `apps/desktop/src/main.tsx` → `src/app/desktop-home.tsx`.
- Donut premium/trial and device-code flows are excluded. API-backed BugLogin flows and BugLogin identity assets remain authoritative.
- Android/iOS fingerprint choices stay visible but disabled until the runtime supports them; desktop OS choices remain available without a plan gate.

## Acceptance

- User-facing desktop profile, table, group, proxy, and settings surfaces use Donut 0.33 components/tokens or a reviewed BugLogin adaptation.
- Tauri commands and API contracts remain supported; no upstream UI action is imported with an unregistered command.
- English and Vietnamese cover newly introduced UI strings; other locales may fall back to English.
- Source differences and intentional exclusions are recorded before calling the sync complete.
