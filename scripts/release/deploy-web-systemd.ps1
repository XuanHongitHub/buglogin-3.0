[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ArtifactDirectory,
    [string]$HostAlias = "bugideas-prod",
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$artifactRoot = (Resolve-Path $ArtifactDirectory).Path
$bundlePath = Join-Path $artifactRoot "buglogin3-web.tar.gz"
$manifestPath = Join-Path $artifactRoot "release-manifest.json"
$remoteHelper = Join-Path $PSScriptRoot "deploy-web-systemd-remote.sh"

foreach ($name in @("git.exe", "ssh.exe", "scp.exe", "tar.exe")) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) { throw "Required command is unavailable: $name" }
}
foreach ($path in @($bundlePath, $manifestPath, $remoteHelper)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Required deployment input is missing: $path" }
}

$commit = (& git.exe -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw "Could not resolve current commit." }
$status = (& git.exe -C $repoRoot status --porcelain --untracked-files=all | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $status) { throw "Refusing web deployment from a dirty worktree." }
$upstream = (& git.exe -C $repoRoot rev-parse --abbrev-ref --symbolic-full-name '@{u}').Trim()
if ($LASTEXITCODE -ne 0) { throw "Current branch must track a remote branch." }
$aheadBehind = (& git.exe -C $repoRoot rev-list --left-right --count "$upstream...HEAD").Trim()
if ($LASTEXITCODE -ne 0 -or $aheadBehind -notmatch '^0\s+0$') { throw "Current commit must exactly match its pushed upstream." }

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.commit -ne $commit -or $manifest.target -ne "buglogin3-web-linux-x64") { throw "Manifest must match this pushed commit and web target." }
$bundleHash = (Get-FileHash -LiteralPath $bundlePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($manifest.bundleSha256 -ne $bundleHash) { throw "Web bundle checksum does not match release-manifest.json." }

$ssh = (Get-Command ssh.exe).Source
$scp = (Get-Command scp.exe).Source
$remote = "sudo -n systemctl is-active buglogin3-web && sudo -n systemctl show buglogin3-web -p FragmentPath -p ExecStart --no-pager"
$remoteInfo = (& $ssh -n -T -o BatchMode=yes -o ConnectTimeout=8 $HostAlias $remote | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $remoteInfo -notmatch '/etc/systemd/system/buglogin3-web\.service' -or $remoteInfo -notmatch '/opt/buglogin3-web/current/server\.js') {
    throw "Remote unit does not match the isolated BugLogin 3.0 web target."
}

if ($DryRun) {
    [pscustomobject]@{ result = "preflight_passed_no_changes"; commit = $commit; host = $HostAlias; bundleSha256 = $bundleHash } | ConvertTo-Json -Depth 3
    return
}

$nonce = [Guid]::NewGuid().ToString("N")
$remoteStage = "/tmp/buglogin3-web-$commit-$nonce"
& $ssh -n -T -o BatchMode=yes -o ConnectTimeout=8 $HostAlias "umask 077 && mkdir -m 700 -- $remoteStage"
if ($LASTEXITCODE -ne 0) { throw "Could not create isolated remote staging directory." }
try {
    & $scp -q $bundlePath "${HostAlias}:$remoteStage/buglogin3-web.tar.gz"
    if ($LASTEXITCODE -ne 0) { throw "Could not upload the web bundle." }
    & $scp -q $manifestPath "${HostAlias}:$remoteStage/release-manifest.json"
    if ($LASTEXITCODE -ne 0) { throw "Could not upload the web manifest." }
    & $scp -q $remoteHelper "${HostAlias}:$remoteStage/deploy-web-systemd-remote.sh"
    if ($LASTEXITCODE -ne 0) { throw "Could not upload the web deployment helper." }
    $remoteDeploy = "sudo -n /bin/bash $remoteStage/deploy-web-systemd-remote.sh activate $remoteStage $commit $bundleHash"
    & $ssh -n -T -o BatchMode=yes -o ConnectTimeout=8 $HostAlias $remoteDeploy
    if ($LASTEXITCODE -ne 0) { throw "Remote web activation failed; inspect the isolated BugLogin 3.0 web unit." }
} finally {
    & $ssh -n -T -o BatchMode=yes -o ConnectTimeout=8 $HostAlias "rm -rf -- $remoteStage" | Out-Null
}
