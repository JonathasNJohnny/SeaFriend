$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $projectRoot "desktop\Cargo.toml"
$vsDevCmd = "C:\Program Files\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat"
$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"

if (-not (Test-Path -LiteralPath $vsDevCmd)) {
    throw "Visual Studio Developer Command file not found: $vsDevCmd"
}
if (-not (Test-Path -LiteralPath $cargo)) {
    throw "Cargo not found: $cargo"
}

$command = "call `"$vsDevCmd`" -arch=x64 && `"$cargo`" run --manifest-path `"$manifest`""
cmd.exe /d /c $command
exit $LASTEXITCODE
