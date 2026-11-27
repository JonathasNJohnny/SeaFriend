param(
    [Parameter(Mandatory = $false)]
    [string] $ApiUrl = "http://127.0.0.1:3100",
    [int] $AudioProcessId = 0
)

$ErrorActionPreference = "Stop"
$bundledBinary = Join-Path $PSScriptRoot "seafriend-desktop.exe"
if (Test-Path -LiteralPath $bundledBinary) {
    $desktopBinary = $bundledBinary
} else {
    $desktopBinary = (Resolve-Path "$PSScriptRoot\..\desktop\target\release\seafriend-desktop.exe").Path
}
$oldApiUrl = $env:SEAFRIEND_API_URL
$oldAudioProcessId = $env:SEAFRIEND_AUDIO_PROCESS_ID

try {
    $env:SEAFRIEND_API_URL = $ApiUrl.TrimEnd('/')
    if ($AudioProcessId -gt 0) {
        $env:SEAFRIEND_AUDIO_PROCESS_ID = [string]$AudioProcessId
    } else {
        Remove-Item Env:SEAFRIEND_AUDIO_PROCESS_ID -ErrorAction SilentlyContinue
    }
    Write-Host "Iniciando SeaFriend Desktop com API em $env:SEAFRIEND_API_URL"
    & $desktopBinary
    exit $LASTEXITCODE
} finally {
    if ($null -eq $oldApiUrl) {
        Remove-Item Env:SEAFRIEND_API_URL -ErrorAction SilentlyContinue
    } else {
        $env:SEAFRIEND_API_URL = $oldApiUrl
    }
    if ($null -eq $oldAudioProcessId) {
        Remove-Item Env:SEAFRIEND_AUDIO_PROCESS_ID -ErrorAction SilentlyContinue
    } else {
        $env:SEAFRIEND_AUDIO_PROCESS_ID = $oldAudioProcessId
    }
}
