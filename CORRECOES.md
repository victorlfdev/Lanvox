# Correções da análise técnica

Implementação em 16/11/2026, a partir de `cbde5d6`. Os itens foram implementados em sequência; os testes e a revisão consolidada foram executados na etapa final.

## Bugs corrigidos

| Item | Alteração |
|---|---|
| B01 — revogação de voz | Mudanças de estado administrativo revogam a participação antes da sincronização. As rotas também verificam VIEW, CONNECT e timeout dos dois participantes. |
| B02 — sobrescrita de downloads | Downloads escrevem em temporários exclusivos no diretório escolhido. O destino só é substituído depois da conclusão e sincronização; cancelamentos removem apenas o temporário. |
| B03 — cache entre hosts | Diretório por instância e nomes derivados da identidade, host e anexo. A limpeza respeita arquivos das sessões em segundo plano. |
| B04 — limite de downloads | Blocos têm orçamento separado dos comandos de controle. O host informa uma espera limitada e o cliente agenda a repetição. Erros de transferência identificam o anexo. |
| B05 — histórico e leitura | A janela registra a existência de mensagens posteriores e não insere mensagens ao vivo no meio de uma consulta ao passado. A leitura automática usa o fim apresentado; marcar todas é uma ação separada. |
| B06 — oito anexos | Constante compartilhada, aplicada na seleção, preparação, restauração e validação do host. Reservas e tarefas pendentes participam dos mesmos limites. |
| B07 — retenção de anexos | Referências mensagem–anexo persistidas. A última exclusão remove o conteúdo e sua cota; exclusão de canal também coleta seu histórico e arquivos. Uploads concluídos sem publicação expiram após 24 horas. O cliente conserva sua cópia local até o ACK e permite recuperar texto, resposta e anexos pelo botão “Voltar ao rascunho”. A migração coleta resíduos de canais excluídos em bancos antigos. |
| B08 — uploads vencidos | Expiração persistida antes da admissão e manutenção periódica, preservando transferências ativas. |
| B09 — não lidas e menções | Edição e exclusão publicam novos resumos autoritativos; os indicadores e o primeiro marcador de não lidas são reconciliados. |
| B10 — credenciais por host | Reconexões consultam o registro do endereço correto. A ativação de uma sessão restaura suas preferências. A entrada manual mantém as credenciais digitadas e só confirma o registro depois de conectar. |
| B11 — rascunhos e rotação | Migração idempotente dos arquivos, verificação de conteúdo e preservação da origem até o commit local. A preparação de anexos precisa terminar antes da rotação. |
| B12 — formatação | Código formatado e `cargo fmt --check` aprovado. |

## Desempenho e robustez

- **O01 — persistência:** mensagem, ACK idempotente, detalhes e contador são confirmados em uma transação pequena. Leitura, edição, reação e fixação também usam atualizações específicas, sem serializar todos os avatares. Criação de enquete mantém a alteração administrativa na mesma transação. Os estados normalizados são recuperados ao abrir o banco.
- **O02 — arquivos e preferências:** orçamento reservado antes de ler lotes, leitura limitada, preparação/hash/gravação/recuperação fora da UI e estado de preparação visível. Preferências comuns usam um worker que agrupa atualizações e elimina gravações idênticas; envios e mudanças críticas de identidade mantêm gravação síncrona.
- **O03 — sincronização:** snapshots têm um orçamento próprio limitado ao protocolo, preservando a ordem dos comandos. Uma atualização de perfil agora usa um delta, evitando retransmitir todo o workspace. Um snapshot de 20 MiB foi verificado sem consumir o orçamento comum de controle.
- **O04 — renderização:** janela de apresentação compartilhada por revisão, hash de avatar calculado ao receber alterações, cache de datas e descarte de caches de apresentação obsoletos. Os vetores pequenos existentes foram preservados.
- **O05 — áudio:** mixagem, decodificação e resampling saíram do callback do dispositivo e passaram a um worker. O callback consome um ring buffer com operações atômicas, sem mutex ou alocação, e registra underruns. O ciclo de vida do consumidor encerra o produtor.
- **O06 — consultas:** envelopes tipados e não recursivos carregam o identificador da requisição. Respostas antigas são descartadas e os erros encerram apenas a consulta correspondente, inclusive quando há rate limit. Canais de tarefas locais desconectados deixam de manter indicadores presos.

As alterações preservam ACK após commit, transporte cifrado, proteção contra replay, filas limitadas e separação das rotas de mídia.

## Validação final

| Verificação | Resultado |
|---|---|
| Testes sem GUI | **132 aprovados** |
| Testes desktop | **213 aprovados; 7 ignorados** |
| Clippy sem GUI, todos os targets, `-D warnings` | Aprovado |
| Clippy desktop, todos os targets, `-D warnings` | Aprovado |
| Formatação e verificação de whitespace do diff | Aprovadas |
| Captura desktop 940×500 | Gerada e inspecionada |

São 26 verificações adicionais em relação à base, incluindo um ensaio de desempenho. A cobertura acrescentada inclui revogação de áudio, substituição atômica de arquivos, orçamento de download, requisições correlacionadas, histórico, persistência após reinício, rollback de idempotência, retenção de anexos compartilhados, coleta de órfãos, migração do banco anterior, rotação de rascunhos, recuperação de anexos pendentes e ordem/limites do ring buffer.

No ensaio sintético original — vinte mensagens, SQLite em memória, 4 MiB de metadados de avatares — o tempo caiu de **2.155 ms para 1 ms**. A última execução sem avatares também registrou 1 ms. É uma medição local em perfil dev, com resolução em milissegundos, sem rede, disco ou dispositivos; não representa um fator geral de aceleração nem percentis de produção.

Código de regressão: [host](app/src/net/server/workspace/tests/regressions.rs), [cliente](app/src/state_regressions.rs), [armazenamento](app/src/storage/tests.rs), [arquivos](app/src/file_transfer.rs) e [áudio](app/src/media/audio/output_ring.rs). Captura: [interface em tamanho mínimo](app/target/qa/correcoes-chat-940.png).

## Compatibilidade

O protocolo passou para **6** e o esquema SQLite para **5**. Atualize host e clientes juntos. O banco é migrado para a frente; um binário anterior recusa o esquema novo. A migração e a restauração de backups foram cobertas pela suíte.

Câmera/microfone reais, VPN, uso prolongado, Linux e medição acústica do novo caminho de áudio permanecem ensaios de ambiente. Os sete testes de dispositivos/mídia ignorados não foram executados. Não foi realizada uma consulta atualizada de advisories de dependências nesta alteração.
