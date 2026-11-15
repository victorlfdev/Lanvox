# Contribuindo

Obrigado por ajudar. Antes de abrir uma issue ou um pull request:

## Nunca publique segredos

Issues, pull requests, comentários, logs de CI, Releases e capturas de tela são públicos. **Não inclua**:

- convites `lanvox://…?token=…` ou tokens de entrada (equivalem a uma senha de acesso ao host);
- backups (`*.lanvox-backup`), bancos `*.sqlite3`, `host-config.json`;
- chaves e segredos (`identity.key`, `hosted-server.host-key`, `*.secret`, `*.key`);
- conversas, anexos ou nomes de pessoas reais de um host em uso.

Para reproduzir um problema, use um host de teste com dados e segredos descartáveis. Se um segredo vazou, gere outro convite ou token e troque as chaves envolvidas.

Falhas de segurança seguem o [SECURITY.md](SECURITY.md): reporte em privado, não em issue pública.

## Transparência sobre privacidade

O administrador do host consegue ler os dados armazenados nele; não há criptografia de ponta a ponta. Mudanças que alterem isso, ou que contatem serviços de terceiros, precisam manter os avisos no app e no README e, para terceiros, ser opcionais e desligadas por padrão.

## Antes do pull request

```powershell
cd app
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo clippy --locked --no-default-features --all-targets -- -D warnings
```

Use o e-mail `noreply` do GitHub nos commits para não expor o seu e-mail pessoal.
