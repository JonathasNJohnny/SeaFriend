# SeaFriend

Aplicação de salas temporárias para comunicação entre participantes.

## Arquitetura

- API: Rust + Axum
- Runtime: Tokio
- Containerização: Docker
- Comunicação P2P: iroh / QUIC
- Relay: suporte do endpoint iroh, ainda pendente de validação externa

## Desenvolvimento

A API está em `api/`.

### Executar

```bash
cd api
docker compose -p seafriend-api up -d --build
```

O cliente desktop está em `desktop/` e pode criar ou acessar salas pela API:

```powershell
$env:SEAFRIEND_API_URL = "http://127.0.0.1:3100"
cargo run --manifest-path desktop/Cargo.toml
```

Para executar os binários release no Windows, use o Developer PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-release.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\run-release.ps1
```

Para iniciar captura de áudio automaticamente, informe o PID:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\run-release.ps1 -AudioProcessId 1234
```

O cliente mantém a captura Windows existente, abre a sinalização WebSocket após
entrar na sala, envia heartbeats periódicos e reconecta a sinalização com backoff.
O transporte P2P integrado cobre vídeo RawRgba8/DeflateRgba8 e áudio PCM float32; a validação
entre máquinas, relay real e hardware de áudio continuam pendentes.

## CI

O workflow `.github/workflows/ci.yml` executa formatação, testes e clippy da API
e do desktop em cada push para `main` e em pull requests. Ele não executa deploy
nem altera a VPS.

O deploy está separado em `.github/workflows/deploy.yml` e só pode ser iniciado
manualmente no environment `production`, com host, chave SSH e known-hosts
fornecidos por secrets. O workflow mantém o Compose isolado do projeto e não
foi executado neste desenvolvimento.
Os requisitos dos secrets estão em [docs/deploy.md](docs/deploy.md).
Para clientes fora da VPS, a API precisa estar disponível por HTTPS com proxy
WebSocket; o Compose mantém a porta do container vinculada localmente à VPS.
