# BugLogin Production Infrastructure Inventory

Inventory date: 2026-10-22. Initial host inspection was read-only. Release updater keys and Apple/S3 secret names were later configured in the private BugLogin 3.0 repository; no secret values are recorded here.

This file is the handoff source for BugLogin infrastructure and release wiring. Recheck live state before any release or deployment because DNS, credentials, and workflows can change.

## Access map

| SSH alias | Host | Role | Verified access/config |
|---|---|---|---|
| `kofserver` | `103.101.161.156` | Existing BugLogin 1.0 host | SSH resolves as `root`; key path is `~/.ssh/id_rsa_server`. Nginx currently serves `buglogin.com`, `www.buglogin.com`, and `api.buglogin.com`. |
| `bugideas-prod` | `152.42.243.109` | Shared BugIdea VPS with isolated BugLogin 3.0 services | SSH resolves as `root`; key path is `~/.ssh/bugideas_ed25519`. BugLogin services have their own `buglogin3-*` units and `/opt/buglogin3` deployment boundary. |

Private key material and environment secret values must stay on their hosts or in the approved secret manager. This inventory records paths and non-secret routing only.

## BugLogin 1.0 host

- Nginx site file: `/etc/nginx/sites-available/buglogin.com`.
- Configured names: `buglogin.com`, `www.buglogin.com`, and `api.buglogin.com`.
- Nginx proxies to local services on ports `3003` and `13242`.
- PM2 reports `buglogin-api` and `buglogin-web` online. The profile sync service source/config is under `/var/www/buglogin/app/buglogin-sync`.
- Environment file locations: `/var/www/buglogin/app/.env` and `/var/www/buglogin/app/buglogin-sync/.env`. Values are intentionally not recorded here.
- The sync service config identifies DigitalOcean Spaces endpoint `https://sgp1.digitaloceanspaces.com`, region `sgp1`, bucket `mavelnut-login`, and `S3_FORCE_PATH_STYLE=false`. It contains S3 access credentials; do not copy their values into this repo.
- The old sync config also has browser release settings. The variable names are `BROWSER_RELEASE_STORAGE` and `BROWSER_RELEASE_S3_KEY`; their values are not included in this inventory.

## Shared BugIdea VPS: BugLogin 3.0

- Active units checked read-only: `buglogin3-api.service`, `buglogin3-web.service`, `buglogin3-cloudflared.service`, and `buglogin3-redis.service`.
- API env files listed by systemd: `/etc/buglogin3/api.env`, `/etc/buglogin3/database.env`, `/etc/buglogin3/redis.env`, and `/etc/buglogin3/s3.env`.
- The API env sets production mode and binds `0.0.0.0:12343`; secrets and database/Redis values are not recorded.
- S3 config in `/etc/buglogin3/s3.env`: endpoint `https://sgp1.digitaloceanspaces.com`, bucket `mavelnut-login`, region `sgp1`, prefix `buglogin3/prod`, tenant `buglogin3-prod`, path-style disabled.
- Existing deployment docs state the 3.0 service temporarily shares legacy BugLogin Spaces credentials. The prefix and tenant are application namespace guards, not a separate S3 ACL/credential boundary.
- Cloudflare Tunnel config routes `api3.buglogin.com` to `http://127.0.0.1:12343`, and `app3.buglogin.com` plus `app.buglogin.com` to `http://127.0.0.1:12342`. The tunnel service is active. Do not print or copy its credential file/token.
- The documented API deployment helper is `scripts/release/deploy-api-systemd.ps1`; it only replaces the isolated BugLogin 3.0 binaries/unit, checks readiness and OpenAPI parity, and has rollback. It does not modify DNS, Cloudflare, env files, databases, or BugIdea services.

## Cloudflare and domain observations

- DNS lookups for `api.buglogin.com`, `api3.buglogin.com`, and `app3.buglogin.com` returned Cloudflare anycast addresses `104.21.7.207` and `172.67.188.4`; these answers do not expose the origin IP.
- The 1.0 Nginx origin was verified by SSH as `103.101.161.156`; the separate 3.0 host/tunnel was verified through alias `bugideas-prod` and its tunnel ingress config.
- No Cloudflare DNS records or deployment hooks were changed during this inventory.

## S3 desktop release path from the 1.0 release channel

- GitHub repository: `keyduc91/Malvanut-Login`, legacy branch `1.0version`; this is the separate 1.0/source repository, not the BugLogin 3.0 release target.
- The legacy macOS desktop release workflow references repository secrets `S3_ACCESS_KEY_ID` and `S3_SECRET_ACCESS_KEY`, uploads versioned DMGs to `s3://mavelnut-login/releases/desktop/v<version>/`, and publishes/updates `s3://mavelnut-login/releases/desktop/latest.json`.
- DigitalOcean Spaces public base: `https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/`.
- Read-only GET of `https://mavelnut-login.sgp1.digitaloceanspaces.com/releases/desktop/latest.json` returned HTTP 200 on 2026-10-22. It identifies version/tag `1.1.84` and lists `BugLogin_1.1.84_aarch64.dmg` and `BugLogin_1.1.84_x64-setup.exe`. Its entries have `signature: null` and `sha256: null`; this is a 1.0 download catalog, not the signed Tauri updater schema used by 2.0. Preserve this legacy feed and do not overwrite it with 2.0 metadata.
- The 1.0 macOS workflow also references Apple certificate, Apple ID/password, and team secrets and contains a signing/notarization path. This is evidence of an existing 1.0 workflow, not evidence that the current 2.0 workflow has completed signing/notarization.
- At the time of the initial VPS inventory, the 2.0 workflow did not publish to Spaces. Since then, the local worktree has been updated to publish immutable 2.0 RC artifacts and the RC updater feed under `releases/desktop/v2/`, and stable promotion publishes a separate stable feed. Static workflow tests and `actionlint` pass; no Actions upload has run, so the configured GitHub credential's write permission and public object behavior remain unverified.
- GitHub Actions secret names `S3_ACCESS_KEY_ID` and `S3_SECRET_ACCESS_KEY` are configured in the repository. Secret values were not read. The exact credentials, write scope, and successful 2.0 upload are therefore not verified.
- The current 2.0 workflows reference secret names `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `WINDOWS_CERTIFICATE_PFX_BASE64`, `WINDOWS_CERTIFICATE_PASSWORD`, `APPLE_CERTIFICATE_P12_BASE64`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_APP_PASSWORD`, and `APPLE_TEAM_ID`. Workflow references are not proof that every secret has a valid value or that signing/notarization succeeds. Never store secret values in this inventory.

## BugLogin 3.0 deployment credentials and updater keys

Target repository: `XuanHongitHub/buglogin-3.0` (private). Its `main` branch is the BugLogin 3.0 release target. The active GitHub CLI identity has `admin: true` on this repo. `keyduc91/Malvanut-Login` is a separate repository and is not the 3.0 backup target.

The following repository secret names were visible in GitHub Actions. GitHub does not expose saved secret values for readback, so their contents and validity have not been independently verified. Values must remain in GitHub Secrets or another approved secret manager; do not paste them into this Markdown file.

| Secret/variable | Observed state | Deployment use / remaining check |
|---|---|---|
| `APPLE_CERTIFICATE_P12_BASE64` | Configured in the BugLogin 3.0 repo from the supplied local certificate file. | macOS Developer ID signing input; import, signing, notarization, and Gatekeeper validation still need successful native Actions evidence. |
| `APPLE_CERTIFICATE_PASSWORD` | Configured in the BugLogin 3.0 repo. | Password for the P12 certificate; GitHub does not provide value readback. |
| `APPLE_ID` | Configured in the BugLogin 3.0 repo. | Apple account used by notarization. |
| `APPLE_APP_PASSWORD` | Configured in the BugLogin 3.0 repo. | App-specific password used by notarization. |
| `APPLE_TEAM_ID` | Configured in the BugLogin 3.0 repo. | Selects the Developer ID team. Workflow derives the signing identity from the imported certificate when the optional identity is empty. |
| `S3_ACCESS_KEY_ID` | Configured in the BugLogin 3.0 repo from `/etc/buglogin3/s3.env` over SSH. | Spaces release publisher. Actual write scope and public readback remain unverified until a release workflow upload succeeds. |
| `S3_SECRET_ACCESS_KEY` | Configured in the BugLogin 3.0 repo from `/etc/buglogin3/s3.env` over SSH. | Spaces release publisher. Actual write scope and public readback remain unverified until a release workflow upload succeeds. |
| `TAURI_SIGNING_PRIVATE_KEY` | Generated locally and configured in the BugLogin 3.0 repo. | Signs Windows/macOS updater archives. Never expose or commit the private key. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Not configured; generated key is unencrypted. | Not required for this key. GitHub stores the key as an encrypted repository secret. |
| `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY` | Configured as a repository variable from the paired Tauri key. | Embedded into release builds to verify Tauri updater signatures. |

Windows Authenticode certificate secrets are not required for this release policy: Windows users install the downloaded NSIS package manually. Tauri updater signatures remain required for in-app updating.

GitHub CLI identity `XuanHongitHub` has `admin: true` on `XuanHongitHub/buglogin-3.0`. GitHub Actions is enabled and hosted runners are available; the repo has zero self-hosted runners. Repo secrets and updater public key variable are now configured. Secret values were not read back.

The active release source is being moved to `releases/desktop/v3/` for BugLogin 3.0. Verify RC and stable feeds there, and leave `releases/desktop/latest.json` and the legacy 1.0 namespace untouched. Keep sync data private under `buglogin3/prod`.

The operator requested no credential rotation. Apple and S3 values are stored as GitHub Actions secrets; their values are not readable back from GitHub and are not copied into this inventory. The release workflow must not print secret values in logs.

## What this resolves and what remains open

The S3 bucket and established release path already exist in the 1.0 system; the prior statement that no release bucket/path was identified is superseded by this inventory. Do not provision another bucket just to proceed.

### Live read-only recheck — 2026-10-22

- `kofserver`: `buglogin-api` and `buglogin-web` both report `online` in PM2; `nginx -t` succeeds. Public `https://buglogin.com/`, `https://api.buglogin.com/`, and the API's `/health` endpoint returned HTTP 200. `/v1/readyz` returns 404 on this legacy 1.0 API, so it is not the correct health route for that service.
- `bugideas-prod`: `buglogin3-api`, `buglogin3-web`, `buglogin3-cloudflared`, and `buglogin3-redis` all report `active` in systemd. Public `https://api3.buglogin.com/v1/readyz` and `https://app3.buglogin.com/` returned HTTP 200.
- The public legacy Spaces manifest still returns HTTP 200 (600-byte response at recheck). This only verifies public read availability; it does not verify authenticated S3 writes, 2.0 publishing, or updater acceptance.
- Checks were read-only. No service was restarted; no DNS, environment file, database, bucket object, or secret value was changed or displayed.

The BugLogin 3.0 release layout uses `releases/desktop/v3/artifacts/v<version>/<run-id>/` for immutable files and separate `releases/desktop/v3/rc/latest.json` and `releases/desktop/v3/stable/latest.json` feeds. The local release source is being updated; successful publication and readback are still pending. Keep the existing 1.0 `releases/desktop/latest.json` unchanged.

Remaining release work:

1. Push the reviewed BugLogin 3.0 release source to `XuanHongitHub/buglogin-3.0` and validate the protected Tauri updater and Apple credentials in Actions.
2. Verify the configured S3 publisher can write the intended `v3` release paths and that public objects can be downloaded and checksum-verified.
3. Run Windows and macOS builds; verify macOS Developer ID signing/notarization, artifact checksums, download URLs, updater signatures, and installation/update behavior on both OSes. Windows Authenticode is not part of this release gate.
4. Keep sync objects under `buglogin3/prod`; do not mix profile-sync payloads with desktop-release artifacts under `releases/desktop/`.
5. Validate staging API/runtime catalog, MCP role permissions, two-client PostgreSQL/S3/Redis sync, checksums/restart recovery, and team-lock behavior before stable promotion.
6. Promote the API and desktop through their documented workflows. DNS/Cloudflare changes are a separate explicit cutover and are not part of the API binary deployment helper.

## Verification boundaries

Verified in this read-only inventory: host aliases/IPs, Nginx upstream ports and hostnames, active 3.0 units, 3.0 tunnel ingress, non-secret S3 endpoint/bucket/prefix settings, configured GitHub secret names, legacy public manifest response and listed artifact names.

Not verified: S3 write permission from GitHub Actions, current Windows/macOS signatures, updater acceptance, BugLogin 3.0 release artifacts, actual Cloudflare DNS record configuration through the Cloudflare API, or fresh cross-device sync/team-lock behavior.

## Recheck commands

- Inspect SSH aliases: `ssh -G kofserver` and `ssh -G bugideas-prod` (never print private key contents).
- On the 1.0 host, inspect only approved non-secret config fields from `/var/www/buglogin/app/buglogin-sync/.env`; redact every access key, secret, token, database URL, and Redis URL.
- On the 3.0 host, inspect non-secret fields from `/etc/buglogin3/s3.env`; keep credentials and tunnel secrets redacted.
- Check the public legacy manifest with a GET request; do not overwrite it during a test.
- Verify workflow/release status through GitHub Actions. Never run a production upload or deployment as an exploratory probe.

### Release execution update — 2026-10-22

- Release target confirmed as `XuanHongitHub/buglogin-3.0`; it is distinct from the 1.0 `keyduc91/Malvanut-Login` repository. The local release source is version `3.0.0` and has been configured for the isolated `releases/desktop/v3/` namespace.
- The 3.0 repo now has Apple signing, S3 publisher, and Tauri updater secrets configured. `BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY` is configured as a repository variable. The generated updater key is unencrypted; no password secret is required. Values are intentionally not recorded.
- A separate Ed25519 runtime-catalog signing key was generated for the v155 Wayfern entries and stored as `BUGLOGIN_RUNTIME_CATALOG_SIGNING_PRIVATE_KEY` in the 3.0 repo. Only the public key is checked into source and deployed to the API. The private key value is not recorded.
- A disposable Spaces preflight in `releases/desktop/v3/preflight/` passed upload, public download, SHA-256 comparison, delete, and post-delete access check. This proves the credentials can write/read/delete in the tested namespace; no desktop release artifact or feed has been published yet.
- The native Windows desktop check passed with the only active release target set to `G:\buglogin-target`. After stopping the old API/Cargo process tree, `cargo clean` removed 108.5 GiB from `E:\buglogin-target` and 8.7 GiB from `C:\buglogin-target`; both target directories are now empty. The build helper, smoke script, tests, and hosted Windows setup action now select G:. Do not create additional build targets on C:, E:, or F:.
- Current local `pnpm release:verify` passes 15/15 checks. The source edits remain uncommitted/unpushed at this point; GitHub signing, notarization, installer generation, API deployment, and release publication remain unverified.
- GitHub REST returned 403 for branch-protection settings on this private repository with the current plan. The desktop release workflow is therefore manual-only from `main`; it validates the version tag and uses the resolved commit SHA for the signed build. Do not push a tag expecting automatic release jobs.

### Runtime and platform support recheck — 2026-10-22

- The current official Wayfern manifest at `https://donutbrowser.com/wayfern.json` reports version `155.0.8059.79`, with Windows x64 and macOS arm64 packages. `macos-x64` is explicitly unavailable. The DMG directory listing confirms the app bundle and executable path `Wayfern.app/Contents/MacOS/Wayfern`.
- The release workflow now targets Windows x64 and Apple Silicon macOS only. Intel Mac is excluded until Wayfern publishes a compatible runtime; shipping an app with no matching browser would fail the profile-launch requirement.
- The v155 runtime archive checksums, signed catalog, production catalog/trust-file deployment, and real Mac install/launch checks are still pending. No private runtime-catalog key is recorded in this document.
