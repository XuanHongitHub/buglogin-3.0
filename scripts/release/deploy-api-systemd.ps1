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
$apiPath = Join-Path $artifactRoot "buglogin-api"
$adminPath = Join-Path $artifactRoot "buglogin-admin-password-reset"
$manifestPath = Join-Path $artifactRoot "release-manifest.json"
$manifestSignaturePath = Join-Path $artifactRoot "release-manifest.sigstore.json"
$openApiPath = Join-Path $artifactRoot "openapi-v1.yaml"
$remoteHelper = Join-Path $PSScriptRoot "deploy-api-systemd-remote.sh"

function Get-CommandOutput([string]$FilePath, [string[]]$Arguments) {
    $output = & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed: $([IO.Path]::GetFileName($FilePath))"
    }
    return (($output | Out-String).Trim())
}

function Invoke-Checked([string]$FilePath, [string[]]$Arguments) {
    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed: $([IO.Path]::GetFileName($FilePath))"
    }
}

foreach ($commandName in @("git.exe", "ssh.exe", "scp.exe")) {
    if (-not (Get-Command $commandName -ErrorAction SilentlyContinue)) {
        throw "Required command is unavailable: $commandName"
    }
}

$commit = Get-CommandOutput "git.exe" @("-C", $repoRoot, "rev-parse", "HEAD")
$status = Get-CommandOutput "git.exe" @("-C", $repoRoot, "status", "--porcelain", "--untracked-files=all")
if ($status) {
    throw "Refusing deployment from a dirty worktree. Commit and push the reviewed release first."
}
$upstream = Get-CommandOutput "git.exe" @("-C", $repoRoot, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}")
$aheadBehind = Get-CommandOutput "git.exe" @("-C", $repoRoot, "rev-list", "--left-right", "--count", "$upstream...HEAD")
if ($aheadBehind -notmatch "^0\s+0$") {
    throw "Refusing deployment: current commit is not exactly synchronized with $upstream."
}

foreach ($path in @($apiPath, $adminPath, $manifestPath, $manifestSignaturePath, $openApiPath, $remoteHelper)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required release input is missing: $path"
    }
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.commit -ne $commit -or $manifest.target -ne "x86_64-unknown-linux-gnu") {
    throw "Release manifest must identify the current commit and x86_64-unknown-linux-gnu target."
}
if (-not (Get-Command "cosign.exe" -ErrorAction SilentlyContinue)) {
    throw "Cosign is required to verify native API build provenance."
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-LinuxExecutable([string]$Path, [string]$ManifestHash) {
    if ((Get-Item -LiteralPath $Path).Length -lt 4) {
        throw "Release executable is empty or truncated: $Path"
    }
    $stream = [IO.File]::OpenRead($Path)
    try {
        $header = [byte[]]::new(20)
        if ($stream.Read($header, 0, 20) -ne 20 -or $header[0] -ne 0x7f -or $header[1] -ne 0x45 -or $header[2] -ne 0x4c -or $header[3] -ne 0x46 -or $header[4] -ne 2 -or $header[5] -ne 1 -or $header[18] -ne 0x3e -or $header[19] -ne 0) {
            throw "Release input is not a 64-bit little-endian Linux x86-64 ELF executable: $Path"
        }
    } finally {
        $stream.Dispose()
    }
    if ((Get-Sha256 $Path) -ne $ManifestHash.ToLowerInvariant()) {
        throw "Release executable hash does not match release-manifest.json: $Path"
    }
}

Assert-LinuxExecutable $apiPath $manifest.artifacts.bugloginApiSha256
Assert-LinuxExecutable $adminPath $manifest.artifacts.adminPasswordResetSha256
$openApiHash = Get-Sha256 $openApiPath
if ($manifest.artifacts.openApiSha256.ToLowerInvariant() -ne $openApiHash) {
    throw "Release OpenAPI hash does not match the CI artifact."
}
& cosign.exe verify-blob $manifestPath --bundle $manifestSignaturePath --certificate-identity-regexp '^https://github\.com/XuanHongitHub/buglogin-3\.0/\.github/workflows/native-api-artifact\.yml@refs/heads/main$' --certificate-oidc-issuer https://token.actions.githubusercontent.com | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Sigstore build provenance verification failed for release-manifest.json."
}

$ssh = (Get-Command "ssh.exe").Source
$scp = (Get-Command "scp.exe").Source
$remoteReadOnly = "sudo -n systemctl is-active buglogin3-api && sudo -n systemctl show buglogin3-api -p FragmentPath -p ExecStart --no-pager"
$remoteInfo = Get-CommandOutput $ssh @("-n", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", $HostAlias, $remoteReadOnly)
if ($remoteInfo -notmatch "/etc/systemd/system/buglogin3-api\.service" -or $remoteInfo -notmatch "/opt/buglogin3/buglogin-api") {
    throw "Remote unit does not match the isolated BugLogin 3.0 API target."
}

$apiHash = Get-Sha256 $apiPath
$adminHash = Get-Sha256 $adminPath
if ($DryRun) {
    [pscustomobject]@{
        result = "preflight_passed_no_changes"
        commit = $commit
        host = $HostAlias
        apiSha256 = $apiHash
        adminResetSha256 = $adminHash
        openApiSha256 = $openApiHash
    } | ConvertTo-Json -Depth 3
    return
}

$nonce = [Guid]::NewGuid().ToString("N")
$remoteStage = "/tmp/buglogin3-api-$commit-$nonce"
$stageCommand = "umask 077 && mkdir -m 700 -- $remoteStage"
Invoke-Checked $ssh @("-n", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", $HostAlias, $stageCommand)

try {
    Invoke-Checked $scp @("-q", $apiPath, "${HostAlias}:$remoteStage/buglogin-api")
    Invoke-Checked $scp @("-q", $adminPath, "${HostAlias}:$remoteStage/buglogin-admin-password-reset")
    Invoke-Checked $scp @("-q", $openApiPath, "${HostAlias}:$remoteStage/openapi-v1.yaml")
    Invoke-Checked $scp @("-q", $remoteHelper, "${HostAlias}:$remoteStage/deploy-api-systemd-remote.sh")
    $remoteDeploy = "sudo -n /bin/bash $remoteStage/deploy-api-systemd-remote.sh activate $remoteStage $commit $apiHash $adminHash $openApiHash"
    $deploymentOutput = Get-CommandOutput $ssh @("-n", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", $HostAlias, $remoteDeploy)
    if ($deploymentOutput -notmatch '(?m)^deployment_id=([0-9]{8}T[0-9]{6}Z-[0-9a-f]{40}-[0-9a-f]{16})\r?$') {
        throw "Remote activation did not return a valid deployment ID. Remote output: $deploymentOutput"
    }
    $deploymentId = $Matches[1]
} catch {
    Write-Error "Staging deployment did not complete. Remote stage retained for diagnosis: $remoteStage"
    throw
}

try {
    Invoke-Checked "node.exe" @((Join-Path $repoRoot "scripts/release/verify-staging-api.mjs"))
} catch {
    $verificationError = $_
    $rollbackStage = "/tmp/buglogin3-rollback-$nonce"
    try {
        $rollbackStageCommand = "umask 077 && mkdir -m 700 -- $rollbackStage"
        Invoke-Checked $ssh @("-n", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", $HostAlias, $rollbackStageCommand)
        Invoke-Checked $scp @("-q", $remoteHelper, "${HostAlias}:$rollbackStage/deploy-api-systemd-remote.sh")
        $remoteRollback = "sudo -n /bin/bash $rollbackStage/deploy-api-systemd-remote.sh rollback $deploymentId $apiHash $adminHash"
        Invoke-Checked $ssh @("-n", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", $HostAlias, $remoteRollback)
        Write-Warning "Public staging verification failed; deployment $deploymentId was rolled back to its recorded predecessor."
    } catch {
        Write-Warning "Public staging verification failed and automatic rollback was refused or failed. Deployment $deploymentId remains for diagnosis; inspect the BugLogin-only API and rollback receipt."
        throw
    } finally {
        $rollbackCleanup = "rm -f -- $rollbackStage/deploy-api-systemd-remote.sh && rmdir -- $rollbackStage 2>/dev/null || true"
        & $ssh -n -T -o BatchMode=yes -o ConnectTimeout=8 $HostAlias $rollbackCleanup | Out-Null
    }
    throw $verificationError
}
