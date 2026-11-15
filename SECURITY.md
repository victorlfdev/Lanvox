# Política de segurança

## Como reportar uma vulnerabilidade

**Não abra uma issue pública** para falhas de segurança. Use o relato privado do GitHub:

1. Abra a aba **Security** do repositório.
2. Clique em **Report a vulnerability** (Private vulnerability reporting).
3. Descreva o problema, a versão afetada e, se possível, os passos para reproduzir.

Você receberá uma resposta em até 7 dias. Corrigimos primeiro e divulgamos depois: pedimos que mantenha o relato privado até haver uma versão corrigida. Dê crédito se quiser: citamos quem reportou nas notas da versão, a menos que prefira anonimato.

## Não publique segredos ao reportar

Ao descrever um problema, **não cole** convites `lanvox://…?token=…`, tokens de entrada, chaves (`identity.key`, `hosted-server.host-key`), backups (`*.lanvox-backup`), bancos `*.sqlite3` nem logs de um host real. Se o relato precisar de dados, use um host de teste com segredos descartáveis. Se você expôs um convite ou token por engano, peça ao administrador que gere outro.

## Escopo

Interessam especialmente:

- Falhas no protocolo cliente–host (Noise sobre TCP, mídia cifrada sobre UDP, proteção contra replay).
- Contorno de autenticação, de convites, de permissões ou de banimentos no host.
- Vazamento de chaves de identidade, do token de entrada ou de backups.
- Leitura ou escrita de arquivos fora da pasta de dados (anexos, restauração de backup).
- Execução de código ou travamento remoto do cliente ou do host por mensagens ou mídia malformadas.

Fora do escopo:

- Ataques que exigem acesso ao computador ou à conta do sistema do usuário.
- O administrador do host ver as mensagens e anexos armazenados: não há criptografia de ponta a ponta, e isso é avisado no app e no README.
- Negação de serviço por volume de tráfego de quem já tem convite válido.
- Vulnerabilidades em dependências sem um caminho de exploração no Lanvox (reporte à dependência).

## Versões com suporte

Só a versão mais recente recebe correções de segurança. O protocolo exige host e clientes na mesma versão.

## Privacidade e atualizações

O Lanvox não envia telemetria. Conexões a terceiros acontecem apenas quando você ativa ou pede: o codec OpenH264 baixado da Cisco (verificado por SHA-256) e, se você ativar em Configurações › Vídeo, miniaturas de links de vídeo.
