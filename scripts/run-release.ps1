param(
    [int] $AudioProcessId = 0
)

$ErrorActionPreference = "Stop"
$apiBinary = (Resolve-Path "$PSScriptRoot\..\api\target\release\seafriend-api.exe").Path
$desktopBinary = (Resolve-Path "$PSScriptRoot\..\desktop\target\release\seafriend-desktop.exe").Path
$apiWorkingDirectory = (Resolve-Path "$PSScriptRoot\..\api").Path
$desktopWorkingDirectory = (Resolve-Path "$PSScriptRoot\..\desktop").Path

$existingListener = Get-NetTCPConnection -LocalPort 3000 -State Listen -ErrorAction SilentlyContinue
if ($existingListener) {
    throw "A porta 3000 já está em uso. Encerre o processo existente antes de iniciar o SeaFriend."
}

$oldApiUrl = $env:SEAFRIEND_API_URL
$oldAudioProcessId = $env:SEAFRIEND_AUDIO_PROCESS_ID
$apiProcess = $null
$desktopProcess = $null

try {
    $apiProcess = Start-Process -FilePath $apiBinary `
        -WorkingDirectory $apiWorkingDirectory `
        -PassThru -WindowStyle Hidden

    $healthy = $false
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        Start-Sleep -Milliseconds 250
        if ($apiProcess.HasExited) {
            throw "A API encerrou antes de responder ao healthcheck."
        }
        try {
            $response = Invoke-WebRequest -Uri "http://127.0.0.1:3000/health" -UseBasicParsing -TimeoutSec 2
            if ($response.StatusCode -eq 200) {
                $healthy = $true
                break
            }
        } catch {
        }
    }
    if (-not $healthy) {
        throw "A API não respondeu ao healthcheck em 10 segundos."
    }

    $env:SEAFRIEND_API_URL = "http://127.0.0.1:3000"
    if ($AudioProcessId -gt 0) {
        $env:SEAFRIEND_AUDIO_PROCESS_ID = [string]$AudioProcessId
    } else {
        Remove-Item Env:SEAFRIEND_AUDIO_PROCESS_ID -ErrorAction SilentlyContinue
    }

    Write-Host "API disponível em http://127.0.0.1:3000"
    Write-Host "Iniciando SeaFriend Desktop..."
    $desktopProcess = Start-Process -FilePath $desktopBinary `
        -WorkingDirectory $desktopWorkingDirectory `
        -PassThru
    Wait-Process -Id $desktopProcess.Id
} finally {
    if ($desktopProcess -and -not $desktopProcess.HasExited) {
        Stop-Process -Id $desktopProcess.Id -Force -ErrorAction SilentlyContinue
    }
    if ($apiProcess -and -not $apiProcess.HasExited) {
        Stop-Process -Id $apiProcess.Id -Force -ErrorAction SilentlyContinue
    }
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
