# BugLogin 3.0 public website deployment

The 3.0 web service runs independently at `/opt/buglogin3-web`, behind the `buglogin3-prod` Cloudflare tunnel. The website deployment does not alter BugIdea services, databases, buckets, or the legacy 1.0 API.

## Build and deploy

Use a clean worktree at a commit already pushed to its upstream branch. Keep build output on the release drive, outside the checkout:

```powershell
pwsh -NoProfile -File scripts/release/package-web-release.ps1 -OutputDirectory G:\buglogin-target\web-artifacts\<commit>
pwsh -NoProfile -File scripts/release/deploy-web-systemd.ps1 -ArtifactDirectory G:\buglogin-target\web-artifacts\<commit> -DryRun
pwsh -NoProfile -File scripts/release/deploy-web-systemd.ps1 -ArtifactDirectory G:\buglogin-target\web-artifacts\<commit>
```

The package step runs the production standalone Next.js build with the 3.0 API at `127.0.0.1:12343`, hashes the bundle, and records the pushed commit. Deployment refuses dirty/unpushed source, verifies the host's exact 3.0 systemd target and bundle hash, stages into a new immutable release directory, atomically switches `current`, and checks the rendered landing page. On a failed check it restores the previous symlink and restarts the same web service.

The public landing page is served from `public/index.html`; its layout and styles are the supplied BugLogin HTML. Download buttons resolve the current Windows/macOS artifacts through `/api/downloads/latest`, which reads the stable v3 release catalog and disables unavailable platforms.

## Public hostname cutover

After `https://app3.buglogin.com/` and `/api/downloads/latest` pass their checks, route `buglogin.com` and `www.buglogin.com` to the existing `buglogin3-prod` tunnel. Keep `api.buglogin.com` on its legacy route until its clients are explicitly migrated; BugLogin 3.0 uses `api3.buglogin.com`. The old service/data stay available for rollback and compatibility.

Do not publish platform download buttons until the stable v3 feed contains both verified Windows and macOS installer URLs. The site reads `latest.json` without caching and reports no stable release while that feed is unavailable.
