# Lanvox 0.3.0

**Esta versão usa o protocolo 7: atualize o host e todos os clientes juntos. Clientes de versões anteriores não são compatíveis com o novo host.**

## Vídeo e compartilhamento de tela

- Bitrate adaptável para câmera e compartilhamento de tela, com feedback autenticado dos espectadores e ajuste conforme perdas e filas de vídeo.
- Compartilhamento em 480p e 720p com alvo de 3 Mb/s. Em resoluções maiores, a tela começa em 6 Mb/s; câmera começa em 1 Mb/s. Conteúdo estático pode consumir menos banda.
- Ajustes de bitrate H.264 durante a transmissão, tanto no encoder de GPU quanto no de CPU, com fallback para CPU caso o driver rejeite a mudança.
- Orçamento de vídeo do host configurável por `LANVOX_VIDEO_BUDGET_MBPS`, com padrão de 72 Mb/s, considerando cada cópia enviada aos espectadores.
- Diagnósticos mostram os alvos de bitrate e avisam quando o orçamento é insuficiente para os pisos das transmissões. O orçamento distribui alvos médios; não é um limitador rígido de tráfego.

## Desempenho e interface

- Redução do uso de CPU no envio UDP e melhorias nas verificações contra replay e na remontagem de pacotes.
- Captura de tela evita bloquear enquanto cópias da GPU estão pendentes.
- Correção da exibição de fotos de perfil nas salas de voz, nos participantes de chamadas e nas configurações da conta.
- Avatares acompanham a alteração e remoção da foto de perfil, com fallback para imagens inválidas e durante o carregamento da câmera.

O instalador preserva os dados e a identidade. A instalação encerra o aplicativo e qualquer host executado por ele; coordene a atualização com os participantes.
