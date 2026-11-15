# Lanvox 0.1.0 — implementação integrada

Atualizado em 13/11/2026. O código entregue amplia o desktop Windows e o host LAN/VPN.

## Implementado

- Busca FTS com filtros, ordenação, paginação, contexto e continuação do histórico.
- Leitura contínua, datas, não lidas, posição preservada, caixa de entrada e fixados laterais.
- Compositor com emojis, imagens da área de transferência, arquivos arrastados, prévias, galeria, áudio/vídeo anexado e envio serializado com ACK.
- Encaminhamento idempotente, threads, fóruns, tags, enquetes e emojis próprios da comunidade.
- Cargos múltiplos, hierarquia, capacidades, sobrescritas por canal/categoria, timeout, modo lento, auditoria, convites e eventos com confirmação de interesse.
- Perfis com biografia/apelido, imagens de comunidades/grupos e administração organizada por assunto.
- Temas escuros, escala, densidade, painéis redimensionáveis e controle de qualidade da câmera.
- Notificações por destino, silenciamento e bloqueio local; convites/links de mensagem registrados no instalador Windows.
- Múltiplos hosts em uma sessão, com estado, mixer e decoder isolados. Ao trocar de host, o usuário sai da voz anterior. Hosts desconectados reconectam ao serem selecionados.

## Compatibilidade

O protocolo atual é **5**. Atualize host e clientes juntos. As extensões persistidas usam valores padrão para preservar bancos e configurações existentes. Não foram apagadas identidades nem dados do usuário.

## Validação integrada

- Desktop: **154 testes aprovados**, 4 testes de dispositivos/mídia ignorados.
- Host: **95 testes aprovados**.
- Clippy estrito passou nas configurações desktop e host.
- Formatação aplicada ao projeto.
- Executável compilado: `app/target/debug/lanvox.exe`.
- 24 capturas demonstrativas conferidas em 1280×720 e 940×500: `app/target/qa-integration/`.
- Logs em `app/target/integration-*.log`.

Os testes que iniciam servidores locais agora escolhem uma porta disponível para TCP e UDP, evitando as faixas de exclusão distintas do Windows.

## Pendências explícitas

- Ensaios de gravação de mensagens de voz com dispositivos reais.
- Anexos de até 100 MiB usam transferência em blocos acima de 8 MiB, com progresso, cancelamento e retomada dos blocos confirmados.
- Os anexos preparados são guardados por identidade e host para recuperação após reiniciar. Transferências não confirmadas voltam ao preparo; mensagens confirmadas usam a fila persistente.
- Criptografia de mídia ponta a ponta que exclua o host; permanece a proteção de transporte cliente–host.
- Clientes web/mobile, sincronização entre dispositivos, integrações de bots/webhooks e GIFs/stickers completos.
- Atualização assinada implementada (`updater.rs`); falta gerar a chave do projeto, preencher `PUBLIC_KEY_HEX` e testar com uma release real (veja RELEASING.md).
- Ensaios de dispositivos, acessibilidade com leitor de tela, instalação limpa e chamadas prolongadas em VPN real.

As capturas demonstrativas servem para conferir layout; seus dados e métricas não são medições de chamadas reais.
