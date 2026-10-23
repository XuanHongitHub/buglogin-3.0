param(
    [Parameter(Mandatory = $true)]
    [string]$Binary,
    [Parameter(Mandatory = $true)]
    [string]$ExpectedSha256,
    [Parameter(Mandatory = $true)]
    [string]$Output,
    [int]$TimeoutSeconds = 20
)

$ErrorActionPreference = 'Stop'
$binaryPath = [IO.Path]::GetFullPath($Binary)
if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
    throw "Missing Bugmini binary: $binaryPath"
}
$expected = $ExpectedSha256.Trim().ToLowerInvariant()
$actual = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) {
    throw "Bugmini checksum mismatch"
}

$tempBase = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) 'BugLogin2-Runtime-Smoke'))
$profileRoot = [IO.Path]::GetFullPath((Join-Path $tempBase ([guid]::NewGuid().ToString('N'))))
if (-not $profileRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe smoke profile root"
}
New-Item -ItemType Directory -Path $profileRoot -Force | Out-Null

$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = ([Net.IPEndPoint]$listener.LocalEndpoint).Port
$listener.Stop()
$stdout = Join-Path $profileRoot 'stdout.log'
$stderr = Join-Path $profileRoot 'stderr.log'
$argumentLine = "--headless=new --disable-gpu --no-first-run --no-default-browser-check --user-data-dir=`"$profileRoot`" --remote-debugging-address=127.0.0.1 --remote-debugging-port=$port about:blank"
$launcher = Start-Process -FilePath $binaryPath -ArgumentList $argumentLine -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
$version = $null
$observed = @()
$stopped = $false
try {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $observed = @(Get-CimInstance Win32_Process | Where-Object {
            $_.CommandLine -and $_.CommandLine.Contains($profileRoot, [StringComparison]::OrdinalIgnoreCase)
        })
        try {
            $version = Invoke-RestMethod -Uri "http://127.0.0.1:$port/json/version" -TimeoutSec 1
            break
        }
        catch {
            Start-Sleep -Milliseconds 250
        }
    }
}
finally {
    $targets = @(Get-CimInstance Win32_Process | Where-Object {
        $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine.Contains($profileRoot, [StringComparison]::OrdinalIgnoreCase)
    })
    foreach ($target in $targets) {
        Stop-Process -Id $target.ProcessId -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 700
    $remaining = @(Get-CimInstance Win32_Process | Where-Object {
        $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine.Contains($profileRoot, [StringComparison]::OrdinalIgnoreCase)
    })
    $stopped = $remaining.Count -eq 0
}

$evidence = [ordered]@{
    schema = 'buglogin.runtime-native-smoke/1'
    generatedAt = [DateTime]::UtcNow.ToString('o')
    runtime = 'bugmini'
    platform = 'windows'
    architecture = 'x86_64'
    binarySha256 = $actual
    launchObserved = $observed.Count -gt 0
    runtimeProcessesObserved = $observed.Count
    cdpObserved = $null -ne $version
    cdpBrowser = $version.Browser
    stopObserved = $stopped
    sourceProfileUsed = $false
}
$outputPath = [IO.Path]::GetFullPath($Output)
$outputParent = Split-Path -Parent $outputPath
if ($outputParent) {
    New-Item -ItemType Directory -Path $outputParent -Force | Out-Null
}
$evidence | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $outputPath -Encoding utf8
Remove-Item -LiteralPath $profileRoot -Recurse -Force
$evidence | ConvertTo-Json -Depth 4
if (-not $evidence.launchObserved -or -not $evidence.cdpObserved -or -not $evidence.stopObserved) {
    exit 1
}
