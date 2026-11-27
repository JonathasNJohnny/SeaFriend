param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet("listen", "connect")]
    [string] $Mode,
    [Parameter(Position = 1)]
    [string] $EndpointId
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $projectRoot "desktop\Cargo.toml"
$vsDevCmd = "C:\Program Files\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat"
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"

if (-not (Test-Path -LiteralPath $vsDevCmd)) { throw "Visual Studio Developer Command file not found: $vsDevCmd" }
if (-not (Test-Path -LiteralPath $cargo)) { throw "Cargo not found: $cargo" }
if ($Mode -eq "connect" -and [string]::IsNullOrWhiteSpace($EndpointId)) {
    throw "EndpointId is required in connect mode"
}

$arguments = "--manifest-path `"$manifest`" --example p2p_echo -- $Mode"
if ($Mode -eq "connect") { $arguments += " $EndpointId" }
$command = "call `"$vsDevCmd`" -arch=x64 && `"$cargo`" run $arguments"
cmd.exe /d /c $command
exit $LASTEXITCODE
