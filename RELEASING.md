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

## A cada versão

1. Aumente `version` em `app/Cargo.toml` (formato `x.y.z`; o app nunca aceita versão menor ou igual à instalada) e faça commit.
2. Crie e envie a tag: `git tag v0.2.0 && git push origin v0.2.0`.
3. O workflow *Release build* testa, gera o instalador e cria uma release **rascunho** com ele.
4. Baixe o instalador do rascunho e assine o manifesto na sua máquina:

   ```powershell
   cd app
   cargo run --locked --release -- --update-sign C:\downloads\Lanvox-0.2.0-windows-x64-setup.exe --update-key C:\caminho\privado\lanvox-update.key --update-version 0.2.0 --update-notes "O que mudou" --out C:\downloads\assinatura
   ```

5. Anexe `update-manifest.json` e `update-manifest.json.sig` ao rascunho e publique a release. O app procura esses dois arquivos em `releases/latest/download/`.
6. Teste num cliente com a versão anterior: Configurações › Minha conta › Atualizações › *Procurar atualizações*.

## Notas

- Se a versão mudar o protocolo (`PROTOCOL_VERSION`), o app avisa que host e clientes precisam atualizar juntos.
- A atualização usa o instalador por usuário do Inno Setup; os dados e a identidade são preservados.
- Instalar uma atualização encerra o app (e o host que ele estiver executando).
- Por padrão o app procura atualizações ao abrir (pode ser desligado nas configurações) e mostra uma faixa no topo com o botão para baixar e instalar. Nada é baixado nem instalado sem um clique.
- Sem certificado Authenticode o Windows SmartScreen ainda avisará no primeiro uso do instalador; a assinatura do projeto protege a cadeia de atualização, não substitui o certificado.
