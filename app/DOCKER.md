# Host privado com Docker

O contêiner usa uma conta sem privilégios. O volume `lanvox-data` mantém SQLite, identidade e configuração. A publicação padrão é limitada a `127.0.0.1` no computador Docker.

Copie a chave pública da sua identidade nas preferências do desktop e substitua `CHAVE_PUBLICA_HEX`:

```sh
docker compose build
docker compose run --rm lanvox-host --init-host --data /data --owner-key CHAVE_PUBLICA_HEX
```

São criados `/data/host-config.json` e `/data/registration-token.secret` sem imprimir o segredo. Distribua o token somente aos amigos autorizados, junto com endereço e impressão digital do host. O token concede entrada como membro, nunca como proprietário.

Para acesso remoto, defina `LANVOX_LISTEN_IP` como o IP da interface VPN/LAN do computador Docker. Em PowerShell:

```powershell
$env:LANVOX_LISTEN_IP = '100.80.20.10'
docker compose up -d
```

Em shell POSIX:

```sh
LANVOX_LISTEN_IP=100.80.20.10 docker compose up -d
```

São publicados **TCP e UDP 47800** no endereço escolhido. O bind `0.0.0.0` dentro do contêiner não altera essa restrição. Configure também firewall e política da VPN. Entre por endereço, DNS ou convite; broadcast não é necessário.

```sh
docker compose logs --tail 100 lanvox-host
docker compose exec lanvox-host lanvox --check 127.0.0.1:47800
docker compose stop
```

`--check` verifica somente TCP. Confira qualidade e caminho de mídia no cliente.

Backup consistente com identidade do host:

```sh
docker compose exec lanvox-host lanvox --backup /data/amigos.lanvox-backup --data /data
docker compose cp lanvox-host:/data/amigos.lanvox-backup ./amigos.lanvox-backup
```

O bundle contém chaves e token: trate-o como credencial. Restaure em uma pasta/volume novo, com o host parado. `docker compose stop` e `down` preservam o volume; comandos que removem volumes apagam seus dados e não fazem parte da operação normal.

Host e clientes devem usar protocolo 5. JSON legado é importado com backup. Restaurar o bundle mantém a identidade que os clientes já conhecem.
