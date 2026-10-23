[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
if ($outputRoot.StartsWith($repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Web release artifacts must be stored outside the repository."
}
if (Test-Path -LiteralPath $outputRoot) {
    if ((Get-ChildItem -LiteralPath $outputRoot -Force | Measure-Object).Count -gt 0) { throw "Output directory must be empty." }
} else { New-Item -ItemType Directory -Path $outputRoot | Out-Null }

$status = (& git.exe -C $repoRoot status --porcelain --untracked-files=all | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $status) { throw "Build only from a clean worktree." }
$commit = (& git.exe -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw "Could not resolve current commit." }
$upstream = (& git.exe -C $repoRoot rev-parse --abbrev-ref --symbolic-full-name '@{u}').Trim()
if ($LASTEXITCODE -ne 0) { throw "Current branch must track a remote branch." }
$aheadBehind = (& git.exe -C $repoRoot rev-list --left-right --count "$upstream...HEAD").Trim()
if ($LASTEXITCODE -ne 0 -or $aheadBehind -notmatch '^0\s+0$') { throw "Build only from a commit pushed to upstream." }

$env:BUGLOGIN_APP_TARGET = "web"
$env:NEXT_PUBLIC_BUGLOGIN_APP_TARGET = "web"
$env:BUGLOGIN_ENV = "production"
$env:BUGLOGIN_API_BASE_URL = "http://127.0.0.1:12343"
$env:BUGLOGIN_BUILD_CPUS = "2"
$env:NEXT_TELEMETRY_DISABLED = "1"
Push-Location $repoRoot
$nextEnvPath = Join-Path $repoRoot "next-env.d.ts"
$nextEnvBytes = [IO.File]::ReadAllBytes($nextEnvPath)
try {
    & pnpm.cmd build:web
    if ($LASTEXITCODE -ne 0) { throw "Next.js production web build failed." }
} finally {
    [IO.File]::WriteAllBytes($nextEnvPath, $nextEnvBytes)
    Pop-Location
}

$standalone = Join-Path $repoRoot ".next-web/standalone"
$static = Join-Path $repoRoot ".next-web/static"
$public = Join-Path $repoRoot "public"
foreach ($path in @($standalone, $static, $public)) {
    if (-not (Test-Path -LiteralPath $path -PathType Container)) { throw "Build output is missing: $path" }
}
Copy-Item -LiteralPath $static -Destination (Join-Path $standalone ".next-web/static") -Recurse -Force
Copy-Item -LiteralPath $public -Destination (Join-Path $standalone "public") -Recurse -Force
if (-not (Test-Path -LiteralPath (Join-Path $standalone "server.js") -PathType Leaf)) { throw "Standalone server.js is missing." }

# pnpm's standalone trace can leave dependency aliases under node_modules/.pnpm/node_modules.
# Node does not search that directory when resolving from a flattened standalone package, so
# materialize only aliases that are absent at the standalone node_modules root.
$standaloneModules = Join-Path $standalone "node_modules"
$pnpmAliases = Join-Path $standaloneModules ".pnpm/node_modules"
if (Test-Path -LiteralPath $pnpmAliases -PathType Container) {
    foreach ($entry in Get-ChildItem -LiteralPath $pnpmAliases -Force) {
        $destination = Join-Path $standaloneModules $entry.Name
        if ($entry.Name.StartsWith("@")) {
            New-Item -ItemType Directory -Path $destination -Force | Out-Null
            foreach ($package in Get-ChildItem -LiteralPath $entry.FullName -Force) {
                $packageDestination = Join-Path $destination $package.Name
                if (-not (Test-Path -LiteralPath $packageDestination)) {
                    Copy-Item -LiteralPath $package.FullName -Destination $destination -Recurse -Force
                }
            }
        } elseif (-not (Test-Path -LiteralPath $destination)) {
            Copy-Item -LiteralPath $entry.FullName -Destination $standaloneModules -Recurse -Force
        }
    }
}

$bundlePath = Join-Path $outputRoot "buglogin3-web.tar.gz"
$tar = (Get-Command tar.exe).Source
& $tar -czf $bundlePath -C $standalone .
if ($LASTEXITCODE -ne 0) { throw "Could not package standalone web output." }
$bundleHash = (Get-FileHash -LiteralPath $bundlePath -Algorithm SHA256).Hash.ToLowerInvariant()
$manifest = [ordered]@{
    commit = $commit
    target = "buglogin3-web-linux-x64"
    bundleSha256 = $bundleHash
    builtUtc = [DateTime]::UtcNow.ToString("o")
}
$manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $outputRoot "release-manifest.json") -Encoding utf8
[pscustomobject]@{ result = "web_bundle_ready"; commit = $commit; bundle = $bundlePath; bundleSha256 = $bundleHash } | ConvertTo-Json -Depth 3
