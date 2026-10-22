param(
    [string]$BuildRoot = 'E:\buglogin-target',
    [int]$WaitSeconds = 8
)

$ErrorActionPreference = 'Stop'
$resolvedRoot = [IO.Path]::GetFullPath($BuildRoot)
if ($resolvedRoot.TrimEnd('\') -ine 'E:\buglogin-target') {
    throw 'BugLogin Windows smoke must use the single build target E:\buglogin-target.'
}
$executable = Join-Path $resolvedRoot 'release\buglogin-desktop-app.exe'
$artifacts = @(
    $executable,
    (Join-Path $resolvedRoot 'release\bundle\msi\BugLogin_0.1.0_x64_en-US.msi'),
    (Join-Path $resolvedRoot 'release\bundle\nsis\BugLogin_0.1.0_x64-setup.exe')
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
