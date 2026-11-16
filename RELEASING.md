# Publicar uma versão com atualização assinada

O app só aceita uma atualização cujo manifesto esteja assinado com a **chave privada do projeto**. O GitHub apenas hospeda os arquivos: quem invadir a conta ou o workflow consegue subir um arquivo, mas não consegue fazê-lo ser aceito.

## Uma vez só: criar a chave

```powershell
cd app
cargo run --locked --release -- --update-keygen C:\caminho\privado\lanvox-update.key
```

- O comando grava a chave privada nesse arquivo (recusa sobrescrever) e imprime a chave pública.
- **Guarde o arquivo fora deste repositório**, com backup em local privado (gerenciador de senhas ou pendrive cifrado). Nunca o coloque no Git, em Releases nem em secrets do CI. Se perder a chave, os apps instalados não aceitarão mais atualizações e será preciso distribuir uma nova versão manualmente com outra chave pública.
- Cole a chave pública em `PUBLIC_KEY_HEX` em `app/src/updater.rs` e publique essa versão manualmente. Só a partir dela os apps conseguem se atualizar.
- Confirme também `REPO` em `app/src/updater.rs` (`dono/repositório` do GitHub).

Se a chave privada vazar: gere outra, troque `PUBLIC_KEY_HEX`, distribua a nova versão manualmente e avise os usuários.

## Política de versões

Formato `x.y.z`, só números (o app ignora versões com sufixo, como `0.2.0-beta`) e sempre maior que a instalada.

- **Primeira release: `0.1.0`.** É a versão atual do `Cargo.toml` e não muda antes de ser publicada. Como ela é instalada manualmente, só a versão seguinte consegue testar a atualização automática.
- **Atualizações pequenas: aumente só o último número (`0.1.1`, `0.1.2`, …).** Vale para correções de bugs, ajustes de interface, pequenas melhorias e atualizações de dependências.
- **Recursos novos de tamanho relevante: aumente o do meio e zere o último (`0.2.0`).**
- **Mudou o protocolo (`PROTOCOL_VERSION`): aumente pelo menos o do meio**, porque host e clientes precisam atualizar juntos. Escreva isso nas notas da versão.
- Não pule números sem motivo: os saltos devem ser os menores possíveis. O primeiro número (`1.0.0`) fica para quando o projeto se considerar estável.
- O número vem de um só lugar, `version` em `app/Cargo.toml`. A tag `vX.Y.Z` precisa ser igual a ele, senão o workflow falha. O nome do instalador e o endereço no manifesto derivam desse número.
- Nunca reutilize nem republique um número já lançado com outro conteúdo. Corrigiu algo depois de publicar? Lance o número seguinte.

## A cada versão

1. Aumente `version` em `app/Cargo.toml` seguindo a política acima e faça commit.
2. Crie e envie a tag (exemplo para uma correção): `git tag v0.1.1 && git push origin v0.1.1`.
3. O workflow *Release build* testa, gera o instalador e cria uma release **rascunho** com ele.
4. Assine e anexe com o script (baixa o instalador do rascunho, gera `update-manifest.json` e `update-manifest.json.sig` com a sua chave e os anexa ao rascunho):

   ```powershell
   cd app
   $env:LANVOX_UPDATE_KEY = 'C:\caminho\privado\lanvox-update.key'   # ou passe -KeyPath
   ./scripts/sign-release.ps1 -Version 0.1.1 -Notes "O que mudou"
   ```

   O script recusa rodar se `Cargo.toml` não bater com a versão, se a release já estiver publicada ou se já tiver manifesto anexado.
5. Revise o rascunho no GitHub e publique (ou rode o script com `-Publish` para publicar no mesmo passo). O app procura os dois arquivos em `releases/latest/download/`.
6. Teste num cliente com a versão anterior: Configurações › Minha conta › Atualizações › *Procurar atualizações*.

## Notas

- Se a versão mudar o protocolo (`PROTOCOL_VERSION`), o app avisa que host e clientes precisam atualizar juntos.
- A atualização usa o instalador por usuário do Inno Setup; os dados e a identidade são preservados.
- Instalar uma atualização encerra o app (e o host que ele estiver executando).
- Por padrão o app procura atualizações ao abrir e a cada 5 minutos enquanto está em execução. Baixa e verifica a versão nova automaticamente; quando não houver conexão, reconexão ou host ativo, instala e reabre o app. Isso pode ser desligado nas configurações, mantendo os botões manuais.
- A publicação não envia um aviso instantâneo ao app: a detecção acontece na próxima consulta. Releases em rascunho ou sem manifesto e assinatura válidos não são instaladas.
- Versões antigas mantêm o fluxo manual até receberem uma versão com esse comportamento automático.
- Sem certificado Authenticode o Windows SmartScreen ainda avisará no primeiro uso do instalador; a assinatura do projeto protege a cadeia de atualização, não substitui o certificado.
