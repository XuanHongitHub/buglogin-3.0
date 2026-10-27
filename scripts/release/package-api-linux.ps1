[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BuildDirectory,

    [Parameter(Mandatory = $true)]
    [string]$ArtifactDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$buildRoot = (Resolve-Path $BuildDirectory).Path
$buildManifestPath = Join-Path $buildRoot "release-manifest.json"
$buildSignaturePath = Join-Path $buildRoot "release-manifest.sigstore.json"
$artifactFullPath = [IO.Path]::GetFullPath($ArtifactDirectory)
if (-not (Test-Path -LiteralPath $artifactFullPath)) {
    New-Item -ItemType Directory -Path $artifactFullPath | Out-Null
}
$artifactRoot = (Resolve-Path $artifactFullPath).Path
$repoPrefix = $repoRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if ($artifactRoot.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Release artifacts must be stored outside the repository, preferably on a non-F: volume."
}
if (Get-ChildItem -LiteralPath $artifactRoot -Force | Select-Object -First 1) {
    throw "Artifact directory must be empty: $artifactRoot"
}
if (-not (Test-Path -LiteralPath $buildManifestPath -PathType Leaf)) {
    throw "CI release-manifest.json is missing from the downloaded GitHub artifact."
}

function Get-CommandOutput([string]$FilePath, [string[]]$Arguments) {
    $output = & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed: $([IO.Path]::GetFileName($FilePath))"
    }
    return (($output | Out-String).Trim())
}

$commit = Get-CommandOutput "git.exe" @("-C", $repoRoot, "rev-parse", "HEAD")
$status = Get-CommandOutput "git.exe" @("-C", $repoRoot, "status", "--porcelain", "--untracked-files=all")
if ($status) {
    throw "Refusing to package artifacts from a dirty worktree."
}
$upstream = Get-CommandOutput "git.exe" @("-C", $repoRoot, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}")
$aheadBehind = Get-CommandOutput "git.exe" @("-C", $repoRoot, "rev-list", "--left-right", "--count", "$upstream...HEAD")
if ($aheadBehind -notmatch "^0\s+0$") {
    throw "Refusing to package artifacts: current commit is not exactly synchronized with $upstream."
}
$buildManifest = Get-Content -LiteralPath $buildManifestPath -Raw | ConvertFrom-Json
if ($buildManifest.commit -ne $commit -or $buildManifest.target -ne "x86_64-unknown-linux-gnu") {
    throw "Downloaded CI artifacts do not match the current commit and x86-64 Linux target."
}
if (-not (Test-Path -LiteralPath $buildSignaturePath -PathType Leaf)) {
    throw "Sigstore signature bundle is missing from the downloaded GitHub artifact."
}
if (-not (Get-Command "cosign.exe" -ErrorAction SilentlyContinue)) {
    throw "Cosign is required to verify native API build provenance."
}
& cosign.exe verify-blob $buildManifestPath --bundle $buildSignaturePath --certificate-identity-regexp '^https://github\.com/(?:keyduc91/Malvanut-Login|XuanHongitHub/buglogin-3\.0)/\.github/workflows/native-api-artifact\.yml@refs/heads/main$' --certificate-oidc-issuer https://token.actions.githubusercontent.com | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Sigstore build provenance verification failed for release-manifest.json."
}

$inputs = @{
    bugloginApi = Join-Path $buildRoot "buglogin-api"
    adminPasswordReset = Join-Path $buildRoot "buglogin-admin-password-reset"
}
foreach ($entry in $inputs.GetEnumerator()) {
    if (-not (Test-Path -LiteralPath $entry.Value -PathType Leaf)) {
        throw "Linux release executable is missing: $($entry.Value)"
    }
    $stream = [IO.File]::OpenRead($entry.Value)
    try {
        $header = [byte[]]::new(20)
        if ($stream.Read($header, 0, 20) -ne 20 -or $header[0] -ne 0x7f -or $header[1] -ne 0x45 -or $header[2] -ne 0x4c -or $header[3] -ne 0x46 -or $header[4] -ne 2 -or $header[5] -ne 1 -or $header[18] -ne 0x3e -or $header[19] -ne 0) {
            throw "Release input is not a 64-bit little-endian Linux x86-64 ELF executable: $($entry.Value)"
        }
    } finally {
        $stream.Dispose()
    }
}

$apiTarget = Join-Path $artifactRoot "buglogin-api"
$adminTarget = Join-Path $artifactRoot "buglogin-admin-password-reset"
$apiHash = (Get-FileHash -LiteralPath $inputs.bugloginApi -Algorithm SHA256).Hash.ToLowerInvariant()
$adminHash = (Get-FileHash -LiteralPath $inputs.adminPasswordReset -Algorithm SHA256).Hash.ToLowerInvariant()
if ($apiHash -ne $buildManifest.artifacts.bugloginApiSha256 -or $adminHash -ne $buildManifest.artifacts.adminPasswordResetSha256) {
    throw "Downloaded binaries do not match the CI release manifest."
}
$openApiSource = Join-Path $buildRoot "openapi-v1.yaml"
$openApiExpected = Join-Path $repoRoot "crates/api/openapi/openapi-v1.yaml"
if (-not (Test-Path -LiteralPath $openApiSource -PathType Leaf)) {
    throw "Downloaded OpenAPI contract is missing from the CI artifact."
}
$openApiSourceHash = (Get-FileHash -LiteralPath $openApiSource -Algorithm SHA256).Hash.ToLowerInvariant()
$openApiSourceText = [IO.File]::ReadAllText($openApiSource)
$openApiExpectedText = [IO.File]::ReadAllText($openApiExpected)
$normalizedOpenApiSource = $openApiSourceText -replace "`r`n?", "`n"
$normalizedOpenApiExpected = $openApiExpectedText -replace "`r`n?", "`n"
if ($openApiSourceHash -ne $buildManifest.artifacts.openApiSha256 -or $normalizedOpenApiSource -cne $normalizedOpenApiExpected) {
    throw "Downloaded OpenAPI contract does not match this checkout."
}

Copy-Item -LiteralPath $inputs.bugloginApi -Destination $apiTarget
Copy-Item -LiteralPath $inputs.adminPasswordReset -Destination $adminTarget
$openApiTarget = Join-Path $artifactRoot "openapi-v1.yaml"
Copy-Item -LiteralPath $openApiSource -Destination $openApiTarget
Copy-Item -LiteralPath $buildManifestPath -Destination (Join-Path $artifactRoot "release-manifest.json")
Copy-Item -LiteralPath $buildSignaturePath -Destination (Join-Path $artifactRoot "release-manifest.sigstore.json")
Get-Content -LiteralPath (Join-Path $artifactRoot "release-manifest.json") -Raw
