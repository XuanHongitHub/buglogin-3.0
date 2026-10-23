param(
    [string]$BuildRoot = 'G:\buglogin-target',
    [int]$WaitSeconds = 8
)

$ErrorActionPreference = 'Stop'
$resolvedRoot = [IO.Path]::GetFullPath($BuildRoot)
if ($resolvedRoot.TrimEnd('\') -ine 'G:\buglogin-target') {
    throw 'BugLogin Windows smoke must use the single build target G:\buglogin-target.'
}
$releaseRoot = Join-Path $resolvedRoot 'x86_64-pc-windows-msvc\release'
$executable = Join-Path $releaseRoot 'buglogin-desktop-app.exe'
$artifacts = @(
    $executable,
    (Join-Path $releaseRoot 'bundle\nsis\BugLogin_3.0.0_x64-setup.exe')
)
foreach ($artifact in $artifacts) {
    if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) {
        throw "Missing desktop artifact: $artifact"
    }
}

$process = Start-Process -FilePath $executable -PassThru -WindowStyle Hidden
try {
    Start-Sleep -Seconds $WaitSeconds
    if ($process.HasExited) {
        throw "Desktop exited before smoke window; exit code $($process.ExitCode)"
    }
    $evidence = [ordered]@{
        schema = 'buglogin.windows-binary-smoke/1'
        aliveAfterSeconds = $WaitSeconds
        processId = $process.Id
        artifacts = @($artifacts | ForEach-Object {
            $item = Get-Item -LiteralPath $_
            $hash = Get-FileHash -LiteralPath $_ -Algorithm SHA256
            [ordered]@{
                path = $item.FullName
                bytes = $item.Length
                sha256 = $hash.Hash.ToLowerInvariant()
            }
        })
    }
    $evidence | ConvertTo-Json -Depth 4
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id
        $process.WaitForExit()
    }
}
