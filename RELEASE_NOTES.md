# Lanvox 0.2.0

**Esta versão usa o protocolo 6: atualize o host e todos os clientes juntos. Clientes de versões anteriores não são compatíveis com o novo host.**

## Atualizações

- Com a opção de atualização automática ativada, o app consulta novas versões ao abrir e a cada 5 minutos, baixa e verifica o instalador assinado e instala quando não há conexão, reconexão ou host ativo. Depois da instalação, o app reabre automaticamente.
- Os controles manuais de atualização continuam disponíveis nas configurações. Versões anteriores mantêm seu comportamento de atualização até instalar esta versão.

## Interface e conversas

- Correção ao colar imagens com Ctrl+V no campo de mensagem.
- Ajustes de navegação e de layouts para janelas pequenas, escala ampliada e nomes longos.
- Ao selecionar um canal de voz, a sala é exibida antes de entrar; a participação começa pelo botão **Entrar na voz**.
- Recuperação de rascunhos e anexos preparados. Ao trocar de conversa durante a gravação de uma mensagem de voz, o áudio é preservado como rascunho local.
- Correções na paginação do histórico, nas buscas e na correlação de respostas ao trocar de conversa ou host.

## Confiabilidade e permissões

- Correções na revogação de acesso, nas permissões e na proteção de anexos no host.
- Melhorias na retomada de uploads, na limpeza de anexos e na gravação de downloads.
- Correções no isolamento de credenciais por host, na persistência e na recuperação de dados.
- Redução de trabalho na interface e na persistência; mixagem de áudio movida para fora do callback do dispositivo.
- Banco SQLite atualizado para o esquema 5, com migração automática.

O instalador preserva os dados e a identidade. A instalação encerra o aplicativo e qualquer host executado por ele; coordene a atualização com os participantes.
