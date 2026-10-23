# Native BugLogin API Staging Deployment

## Scope

Provide one guarded path to publish the Rust API worktree to the isolated BugLogin 3.0 systemd service on `bugideas-prod`. The deployment target remains `api3.buglogin.com`; it must not edit `api.buglogin.com`, DNS, Cloudflare, BugIdea services, application data, or secret files.

## Release inputs

- Require a clean Git worktree whose current commit is exactly synced with its upstream branch. Deployment cannot publish uncommitted source.
- Require Linux x86-64 release artifacts for `buglogin-api` and `buglogin-admin-password-reset`, plus the canonical `openapi-v1.yaml` copied from the same checkout.
- Build artifacts in a GitHub Actions workflow only from a pushed commit reachable from `main`; generate `release-manifest.json` there and attest the binaries, manifest, and OpenAPI file. Locally verify each GitHub provenance attestation and reject a manifest whose commit differs from the clean, pushed checkout.
- Compare the supplied OpenAPI bytes with the checked-in API contract before transfer. Record SHA-256 for both binaries and the contract without printing environment or credential values.
- Transfer to a unique staging directory; validate all staged file types and hashes before touching the service.

## Activation and rollback

- Target only `/opt/buglogin3/buglogin-api`, `/opt/buglogin3/buglogin-admin-password-reset`, `/opt/buglogin3/rollback`, and `buglogin3-api.service`.
- Keep timestamped, checksum-identified copies of the current API and operator binary before replacing them. Preserve the prior API until health and contract verification succeeds.
- Restart only `buglogin3-api.service`. Require active systemd state, `GET /v1/readyz` with `status=ready`, and exact live OpenAPI SHA-256 parity with the supplied contract.
- On any activation or verification failure, restore the previous API and operator binary state, restart only the BugLogin API unit, and verify readiness. Preserve the new artifact bundle and deployment log for diagnosis.
- Report runtime-catalog availability from the readiness header. API success with an unavailable runtime catalog is explicitly not browser-launch readiness.

## Security boundaries

- No database, S3, Redis, environment-file, DNS, ingress, or BugIdea-owned resource mutations.
- No application credential or password in arguments, logs, environment output, or deployment report.
- The password-reset CLI is shipped for a separately authorized interactive operation; deployment never runs it.
- Production cutover from `api3` to `api.buglogin.com` remains outside this deployment path and requires its own approval.
