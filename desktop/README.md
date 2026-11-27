# SeaFriend Desktop — Cliente Windows

Esta prova técnica cria uma janela nativa com Slint e permite selecionar uma janela do Windows pelo `GraphicsCapturePicker`.

Após a seleção, o `windows-capture` recebe frames do Windows Graphics Capture. Os frames RGBA são enviados por um canal limitado diretamente para a UI Slint, descartando frames atrasados para reduzir latência e evitar escrita em disco.

## Executar

No Developer PowerShell/Command Prompt do Visual Studio:

```powershell
cargo run --manifest-path desktop/Cargo.toml
```

Se o terminal comum não encontrar `msvcrt.lib`, execute pela raiz do projeto usando o script que carrega automaticamente o ambiente MSVC e o Windows SDK:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\run-desktop.ps1
```

A captura real depende de uma sessão Windows interativa, permissão do sistema e uma janela selecionável. O arquivo de saída é um artefato de prova local e não deve ser versionado.

## Prova de áudio seletivo

A Etapa 5 inclui um exemplo WASAPI process-loopback que captura o processo informado e sua árvore de processos por alguns segundos:

```powershell
cargo run --manifest-path desktop/Cargo.toml --example audio_loopback -- 1234 10 teste.raw
```

Ou, na raiz do projeto, sem depender do ambiente do terminal:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\run-audio-loopback.ps1 -ProcessId 1234 -Seconds 10 -Output teste.raw
```

O primeiro argumento é o PID do aplicativo. O arquivo é PCM bruto float32, 48 kHz, estéreo. A prova deve ser feita com um aplicativo emitindo áudio e comparada com outro aplicativo emitindo áudio simultaneamente; o arquivo deve conter apenas o processo selecionado e seus filhos.

Na aplicação integrada, o botão `Listar processos` mostra os processos locais
com nome e PID. O PID pode ser informado no campo `PID do aplicativo para áudio`
e iniciado pelo botão `Capturar áudio`. A variável
`SEAFRIEND_AUDIO_PROCESS_ID` continua disponível para iniciar a captura
automaticamente.

Para usar o desktop release contra uma API hospedada na VPS:

```powershell
powershell -ExecutionPolicy Bypass -File .\run-desktop-release.ps1 `
  -ApiUrl "https://api.exemplo.com" -AudioProcessId 1234
```

## Próxima prova: P2P iroh/QUIC

O exemplo `p2p_echo` será usado na Etapa 6 para validar endpoint, ALPN, conexão QUIC e troca de dados. O endereço impresso pelo modo `listen` deverá ser passado ao modo `connect` em outra máquina.

Para executar mesmo a partir de um PowerShell comum:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\run-p2p-echo.ps1 listen
powershell -ExecutionPolicy Bypass -File .\scripts\run-p2p-echo.ps1 connect ENDPOINT_ID
```

## Primeira prova de mídia

O exemplo `media_echo` envia três pacotes de vídeo sintéticos com sequência, timestamp, dimensões e payload limitado:

```powershell
cargo run --manifest-path desktop/Cargo.toml --example media_echo -- listen
cargo run --manifest-path desktop/Cargo.toml --example media_echo -- connect ENDPOINT_ADDR_B64
```

Com a captura Windows real:

```powershell
cargo run --manifest-path desktop/Cargo.toml --example video_stream -- listen
cargo run --manifest-path desktop/Cargo.toml --example video_stream -- connect ENDPOINT_ADDR_B64
```

## Integração com salas

Com a API executando em `http://127.0.0.1:3100`, o cliente inicia com:

```powershell
$env:SEAFRIEND_API_URL = "http://127.0.0.1:3100"
cargo run --manifest-path desktop/Cargo.toml
```

A tela permite criar uma sala ou entrar com código. Depois da resposta da API,
o cliente abre o WebSocket de sinalização e envia heartbeat a cada 15 segundos.
