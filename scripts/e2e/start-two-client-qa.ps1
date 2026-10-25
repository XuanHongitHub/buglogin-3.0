[CmdletBinding()]
param(
    [string]$Executable = 'G:\buglogin-target\debug\buglogin-desktop-app.exe',
    [string]$ApiBaseUrl = 'http://127.0.0.1:12345',
    [string]$InstanceA = 'qa-a-20261025',
    [string]$InstanceB = 'qa-b-20261025',
    [switch]$WebViewDebug
)

$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw 'Run this launcher with PowerShell 7.4 or newer (pwsh).'
}
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw 'Build the debug BugLogin executable before launching QA clients.'
}
$api = [Uri]$ApiBaseUrl
if ($api.Scheme -ne 'http' -or $api.Host -notin @('127.0.0.1', 'localhost', '[::1]') -or $api.IsDefaultPort -or $api.UserInfo -or $api.Query -or $api.Fragment -or $api.AbsolutePath -ne '/') {
    throw 'This QA launcher requires a loopback HTTP API with an explicit port.'
}
if ($InstanceA -eq $InstanceB) {
    throw 'QA clients must use distinct instance IDs.'
}
foreach ($instanceId in @($InstanceA, $InstanceB)) {
    if ($instanceId -cnotmatch '^[a-z0-9][a-z0-9_-]{0,31}$') {
        throw 'Invalid QA instance ID.'
    }
}
if ($env:BUGLOGIN_API_TOKEN -or $env:BUGLOGIN_DATA_ROOT) {
    throw 'Use a shell without BUGLOGIN_API_TOKEN or BUGLOGIN_DATA_ROOT; QA clients sign in separately.'
}
$ready = Invoke-RestMethod -Uri "$($ApiBaseUrl.TrimEnd('/'))/v1/readyz" -TimeoutSec 5
if ($ready.status -ne 'ready' -or $ready.service -ne 'buglogin-api') {
    throw 'Start the isolated local-qa-api-session.mjs service first.'
}
# Launch a verified immutable copy so Windows does not lock Cargo's build output.
$sourceHash = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash
$snapshotRoot = Join-Path $env:TEMP ('buglogin-two-client-qa-' + $sourceHash.Substring(0, 16).ToLowerInvariant())
[void](New-Item -ItemType Directory -Path $snapshotRoot -Force)
$snapshotExecutable = Join-Path $snapshotRoot 'buglogin-desktop-app.exe'
# Instance and generation suffixes can exceed Windows LevelDB path limits under
# AppData. Keep the disposable QA cache short; native device/key isolation stays
# scoped by BUGLOGIN_INSTANCE_ID.
$qaProfileCacheRoot = Join-Path $env:TEMP 'blq'
if (-not (Test-Path -LiteralPath $snapshotExecutable -PathType Leaf)) {
    Copy-Item -LiteralPath $Executable -Destination $snapshotExecutable
}
if ((Get-FileHash -LiteralPath $snapshotExecutable -Algorithm SHA256).Hash -ne $sourceHash) {
    throw 'QA executable snapshot integrity check failed.'
}
foreach ($instanceId in @($InstanceA, $InstanceB)) {
    $existing = Get-Process buglogin-desktop-app -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowTitle.Contains("[$instanceId]") }
    if ($existing) {
        Write-Output "Already running: $instanceId"
        continue
    }
    $childEnvironment = @{
        BUGLOGIN_INSTANCE_ID = $instanceId
        BUGLOGIN_API_BASE_URL = $ApiBaseUrl.TrimEnd('/')
        BUGLOGIN_DEPLOYMENT_MODE = 'production'
        BUGLOGIN_PROFILE_CACHE_ROOT = $qaProfileCacheRoot
        WEBVIEW2_USER_DATA_FOLDER = (Join-Path $env:TEMP "buglogin-qa-webview\$instanceId")
    }
    $webViewPort = $null
    if ($WebViewDebug) {
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
        $listener.Start()
        $webViewPort = $listener.LocalEndpoint.Port
        $listener.Stop()
        $childEnvironment['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = "--remote-debugging-port=$webViewPort --remote-debugging-address=127.0.0.1"
    }
    $client = Start-Process -FilePath $snapshotExecutable -Environment $childEnvironment -PassThru
    [pscustomobject]@{ instance = $instanceId; processId = $client.Id; api = $ApiBaseUrl; webViewPort = $webViewPort }
}
