# SeaFriend

Aplicação de salas temporárias para comunicação entre participantes.

## Arquitetura

- API: Rust + Axum
- Runtime: Tokio
- Containerização: Docker
- Comunicação P2P: iroh / QUIC (planejado)
- Relay: iroh Relay (planejado)

## Desenvolvimento

A API está em `api/`.

### Executar

```bash
cd api
docker compose -p seafriend-api up -d --build
