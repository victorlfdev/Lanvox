# Lanvox

> Antes chamado Huddle. Convites `huddle://`, variáveis `HUDDLE_*` e as pastas de dados antigas continuam aceitos: na primeira execução, as pastas `Huddle` e `HuddleHost` são renomeadas para `Lanvox` e `LanvoxHost`.

Conversas, voz, câmera e compartilhamento de tela em um host próprio, acessível pela LAN ou VPN. Cliente desktop em Rust/egui e host independente.

## Aplicativo

```powershell
cd app
cargo run --locked
```

Escolha seu nome e entre pelo endereço do host ou convite `lanvox://`. O convite contém endereço, token de entrada e impressão digital. Confirme a identidade com o administrador por um canal confiável. Depois da primeira conexão, a chave do host fica fixada; mudanças não são aceitas silenciosamente.

Para hospedar pelo desktop, selecione a interface que aceitará conexões e inicie o host privado. Nas preferências da conta, copie o convite para os amigos. Sair da conversa preserva o host local, mas encerrar o aplicativo encerra a hospedagem. Para disponibilidade permanente, use o host independente.

## Recursos

- DMs e grupos privados com histórico, respostas, edição, exclusão, reações, menções, mensagens fixadas e busca.
- SQLite transacional, histórico paginado, confirmação após gravação, reenvio idempotente, rascunhos e leitura por conversa. Mensagens antigas não são apagadas ao chegar a 5.000. Anexos sem referência após exclusão são removidos; uploads concluídos e não publicados expiram após 24 horas.
- Anexos autorizados, limites, imagens e download por seletor de arquivos. Arquivos não são executados automaticamente.
- Comunidades com proprietário, administradores, membros, convites, remoção/banimento, categorias, ordenação e restrição de canais.
- Perfil, avatar e presença; notificações e bandeja no Windows.
- Chamadas e salas de voz, mute/deafen, volume individual, PTT, dispositivos e teste de microfone.
- Opus, jitter adaptável, recuperação de perdas, cancelamento de eco AEC3 e supressão de ruído via Sonora.
- H.264 para movimento (GPU pelo Media Foundation quando disponível, senão CPU), regiões JPEG para texto, qualidade adaptável e diagnóstico de FPS/filas/bitrate. O H.264 usa o binário OpenH264 oficial da Cisco, baixado da Cisco na primeira execução e verificado por SHA-256; pode ser desativado em Configurações › Vídeo, e sem ele o vídeo usa JPEG.
- Áudio estéreo do aplicativo compartilhado no Windows com suporte a process loopback. Capturar uma tela exclui o próprio Lanvox para não retransmitir a chamada. É uma opção explícita no seletor.
- Conexão assíncrona, reconexão com backoff e dados locais separados por identidade e host.

### Experiência desktop

- Busca indexada com filtros, ordenação, paginação e contexto; painéis de fixados e caixa de entrada.
- Histórico contínuo, datas, não lidas, posição de leitura e seletor rápido `Ctrl+K`.
- Prévia de anexos, arrastar arquivos, colar imagens, galeria com zoom e áudio/vídeo no Windows.
- Emojis pesquisáveis e personalizados, encaminhamento e enquetes.
- Cargos personalizados, permissões por canal/categoria, timeout, modo lento, auditoria, convites e eventos com confirmação de interesse.
- Threads, fóruns com tags, biografia/apelido e imagens de comunidades/grupos.
- Temas escuros, densidade, escala, painéis redimensionáveis e qualidade de câmera configurável.
- Sessões simultâneas de hosts com rascunhos e mídia isolados. Trocar de host encerra a participação de voz no anterior.
- Notificações por destino, silenciamento temporário e bloqueio local. O instalador Windows registra convites e links de mensagem `lanvox://`.
- Reconexão dos hosts em segundo plano e caixa de entrada paginada diretamente do histórico autorizado.
- Mensagens de voz gravadas no microfone configurado, com prévia e descarte antes do envio. Anexos preparados são recuperados localmente; envios grandes podem retomar os blocos confirmados.
- Permissões básicas editáveis para a comunidade, prévia por membro e pedidos de fala em canais de palco.

O bloqueio local oculta conteúdo e silencia a pessoa neste dispositivo; as permissões de acesso continuam sendo aplicadas pelo host. Arquivos preparados ficam armazenados neste dispositivo até serem removidos ou enviados. A retomada após reiniciar o cliente acontece quando o anexo recuperado é enviado novamente; envios parciais antigos expiram após sete dias e são limpos na inicialização do host ou durante a manutenção.

Interface em português. O desktop Windows é o alvo de empacotamento; o host sem interface é preparado também para Linux. Navegador, celular e federação entre hosts não são fornecidos por esta versão. Nas demais plataformas, notificações dependem das capacidades do sistema.

## Host independente

No desktop, copie **a chave pública da sua identidade** nas preferências. Ela tem 64 caracteres hexadecimais e difere da impressão digital `SHA256:...` do host. Substitua `CHAVE_PUBLICA_HEX` no comando:

```powershell
cd app
cargo run --locked --no-default-features -- --init-host --data ./data --owner-key CHAVE_PUBLICA_HEX
```

São criados `host-config.json` e `registration-token.secret`, sem imprimir o segredo. Ajuste o bind para o IP da LAN/VPN:

```powershell
cargo run --locked --release --no-default-features -- --server --data ./data --bind 192.168.1.20
cargo run --locked --no-default-features -- --check 192.168.1.20:47800
cargo run --locked --no-default-features -- --help
```

O padrão é `127.0.0.1`, recusando novas identidades sem convite, allowlist ou chave de proprietário. O processo precisa escrever na pasta de dados. TCP e UDP usam a porta 47800. TCP transporta comandos/dados por Noise; UDP transporta mídia autenticada/cifrada com chaves de sessão e proteção contra replay. Se UDP estiver indisponível, existe fallback pelo TCP cifrado, com menor isolamento entre voz e vídeo. O cliente mostra o caminho e as métricas.

Para obter o convite a compartilhar (o segredo de entrada fica protegido pelo Windows e nunca é impresso por outros comandos), rode no mesmo usuário que criou a pasta, depois de iniciar o host ao menos uma vez:

```powershell
cargo run --locked --no-default-features -- --print-invite --data ./data --address 192.168.1.20 [--copy]
```

> Não execute `--print-invite` onde a saída seja registrada ou publicada (workflow de CI, log público, transmissão de tela). Prefira `--copy`.

Isso imprime (ou, com `--copy`, copia no Windows) `lanvox://ENDERECO:PORTA?token=...&fingerprint=...`. O convite contém o segredo de entrada, portanto equivale a uma senha de acesso: envie por canal confiável. `--address` é obrigatório se o bind for `0.0.0.0`. Nada além do segredo de entrada e da impressão digital pública do host é exposto.

`--check` verifica apenas alcance TCP. Em VPN roteada, use endereço/DNS ou convite: broadcast local não é garantido. Configure as duas portas no firewall e na VPN. Não é necessário publicar diretamente na internet.

Variáveis: `LANVOX_NAME`, `LANVOX_BIND`, `LANVOX_PORT`, `LANVOX_DATA`, `LANVOX_CONFIG`, `LANVOX_OWNER_KEY`, `LANVOX_REGISTRATION_TOKEN_FILE`, `LANVOX_MAX_CONNECTIONS`. Segredos de entrada são lidos de arquivo. Veja [Docker](app/DOCKER.md).

## Identidade e privacidade

> **O administrador do host pode ler tudo o que está armazenado nele**: mensagens, anexos e conversas privadas (DMs e grupos). Não há criptografia de ponta a ponta que o exclua. O Noise protege apenas o caminho entre cliente e host. Só entre em um host cuja pessoa administradora você conheça e em quem confie. O app avisa isso na tela de entrada, nas conversas privadas e nas configurações.

> **Convites e backups são segredos.** Um convite `lanvox://…?token=…` equivale a uma senha de acesso; um backup contém chaves do host e conversas. Nunca os coloque no Git, em Releases, em issues, em logs de CI ou em capturas de tela públicas. O `.gitignore` já ignora `*.lanvox-backup`, bancos `*.sqlite3`, `host-config.json`, `*.secret`, `*.key` e `invite*.txt`.

A identidade do usuário usa Ed25519. Sua chave privada fica na configuração do usuário em `Lanvox/identity.key`; exporte pelas preferências e guarde uma cópia privada. Importar exige desconexão e mantém backup da identidade anterior.

O host mantém chave própria de transporte e token junto aos dados. Conhecer um nome ou ID não concede acesso a uma conversa. Noise protege o canal cliente–host; o administrador controla os dados armazenados. Não há criptografia das mensagens de ponta a ponta que exclua o administrador.

Protocolo 6 requer atualizar host e clientes juntos. O JSON anterior é importado para SQLite e preservado como `pre-sqlite.json`. A migração local de identidades anteriores mantém a proteção contra apropriação de nome; não apague chaves para tentar recuperar uma conta.

## Backup e restauração

Guarde backups fora da pasta do projeto, em local privado. Nunca os publique nem os anexe a Releases ou issues.

```powershell
cargo run --locked --no-default-features -- --backup ./amigos.lanvox-backup --data ./data
cargo run --locked --no-default-features -- --restore ./amigos.lanvox-backup --data ./data-restaurados
```

O bundle inclui banco consistente, chaves do host e arquivos padrão de configuração/entrada na pasta de dados. Contém segredos: guarde-o como uma credencial. Configurações apontando para arquivos externos exigem preservar esses arquivos também.

A restauração valida o banco, recusa caminhos inesperados e não sobrescreve dados. Use uma pasta nova e aponte `--data` para ela. Extensão `.sqlite3` no destino seleciona backup somente do banco. O backup administrativo dentro da comunidade também copia apenas o banco.

## Distribuição Windows

O desktop precisa de C/C++ e CMake para codecs nativos. O script detecta CMake do Visual Studio ou aceita `CMAKE` apontando para o executável.

```powershell
cd app
./scripts/package-windows.ps1 -RequireInstaller
```

O instalador usa Inno Setup (`LANVOX_ISCC` pode indicar `ISCC.exe`), instala por usuário e preserva os dados na desinstalação. Sem Inno Setup, omita `-RequireInstaller` para gerar somente o ZIP. Artefatos, licenças e hashes ficam em `app/target/distribution/`.

Binários locais não recebem assinatura Authenticode automaticamente; distribuição assinada exige o certificado do responsável.

O app atualiza a si mesmo a partir das Releases do GitHub, com manifesto assinado (Configurações › Minha conta › Atualizações). Como publicar versões: [RELEASING.md](RELEASING.md).

## Validação

```powershell
cd app
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo clippy --locked --no-default-features --all-targets -- -D warnings
cargo test --locked --no-default-features
```

Câmera e captura reais ficam ignoradas na suíte determinística. Executar os testes ignorados acessa dispositivos:

```powershell
cargo test --locked -- --ignored --nocapture --test-threads=1
```

QA visual sem dispositivos nem preferências pessoais:

```powershell
cargo run --locked -- --screenshot ./target/qa/chat.png --screenshot-view chat
cargo run --locked -- --screenshot ./target/qa/settings.png --screenshot-view settings --screenshot-size 940x500
```

Também existem modos `home`, `friends`, `dm`, `call`, `voice-settings`, `video-settings`, `stream-settings`, `manage`, `profile`, `group`, `share`, `appearance`, `search`, `inbox`, `roles`, `events`, `expressions`, `forum` e `poll`. Dados e métricas da demonstração não são medições.

Qualidade em VPN real, dispositivos e uso prolongado precisam de ensaios nas máquinas dos participantes.

## Licença

Distribuído sob a [Licença Apache 2.0](LICENSE). Consulte também o arquivo [NOTICE](NOTICE).

O Lanvox é um projeto independente, sem vínculo com o Discord Inc. ou com qualquer outra plataforma de conversa ou voz.
