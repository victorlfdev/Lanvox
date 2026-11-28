# Lanvox 0.3.4

- Adaptação de vídeo também reduz a resolução quando quem assiste reporta fila ou decodificação congestionada, mantendo o FPS solicitado.
- Prévia local mais nítida e adaptação do compartilhamento para sustentar 60 FPS.

# Lanvox 0.3.3

## Compartilhamento de tela

- Adaptação automática passa a reduzir a resolução quando a captura ou a codificação ficam abaixo da taxa solicitada, preservando 60 FPS até 480p.
- Prévia local passa de 720×405 para 1024×576 com cadência de 30 FPS, mantendo baixo o custo de escala.

Esta atualização mantém o protocolo 7. O instalador preserva os dados e a identidade. A instalação encerra o aplicativo e qualquer host executado por ele.

# Lanvox 0.3.2

## Compartilhamento de tela

- Ajustes na captura, conversão de pixels e codificação H.264 para reduzir o custo do compartilhamento de tela.
- Cadência de captura e pré-visualização alinhadas à taxa solicitada, incluindo 60 FPS.
- Adaptação de resolução mais gradual quando a máquina não consegue acompanhar a codificação.
- Melhorias na leitura de quadros do Windows Graphics Capture e nas métricas de captura e codificação.

Esta atualização mantém o protocolo 7. O instalador preserva os dados e a identidade. A instalação encerra o aplicativo e qualquer host executado por ele.

# Lanvox 0.3.1

## Inicialização e conexão

- Conexão automática à última comunidade ao abrir o aplicativo, com opção de escolher uma comunidade fixa ou desativar a conexão inicial em Preferências › Minha conta › Inicialização.
- Restauração do último canal acessível, sem ativar voz, microfone ou câmera automaticamente.
- Novas tentativas com intervalo progressivo em falhas temporárias de rede, inclusive quando DNS ou VPN ainda não estão disponíveis ao entrar no Windows. A opção Reconectar automaticamente controla as tentativas; Cancelar as interrompe.
- Falhas de autenticação ou dados inválidos interrompem a reconexão automática.

## Integração com o Windows

- Opção Iniciar com o Windows nas preferências e no instalador, ativada por padrão em novas instalações e sem exigir administrador.
- Atualizações automáticas preservam o registro de inicialização; a desinstalação o remove.
- Proteção contra instâncias duplicadas: abrir o Lanvox novamente traz a instância existente para frente, e convites são encaminhados a ela.

Esta atualização mantém o protocolo 7, compatível com a versão 0.3.0. O instalador preserva os dados e a identidade. A instalação encerra o aplicativo e qualquer host executado por ele.
