$ErrorActionPreference = "Stop"

Write-Host "Compilando API release..."
cargo build --manifest-path api/Cargo.toml --release --locked
if ($LASTEXITCODE -ne 0) { throw "Falha ao compilar a API." }

Write-Host "Compilando desktop release..."
cargo build --manifest-path desktop/Cargo.toml --release --locked
if ($LASTEXITCODE -ne 0) { throw "Falha ao compilar o desktop." }

Write-Host "Build concluído."
