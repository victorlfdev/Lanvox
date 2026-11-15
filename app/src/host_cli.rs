//! Headless host administration. Secrets are read from files, never printed.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::net::server::ServerOptions;
use crate::protocol::DEFAULT_PORT;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct HostConfig {
    name: String,
    bind: String,
    port: u16,
    owner_key: Option<String>,
    allowlist: Vec<String>,
    revoked_keys: Vec<String>,
    registration_token_file: Option<PathBuf>,
    max_connections: usize,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            name: "Lanvox Hub".into(),
            bind: "127.0.0.1".into(),
            port: DEFAULT_PORT,
            owner_key: None,
            allowlist: Vec::new(),
            revoked_keys: Vec::new(),
            registration_token_file: None,
            max_connections: 64,
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn value(args: &[String], flag: &str) -> io::Result<Option<String>> {
    match args.iter().position(|arg| arg == flag) {
        None => Ok(None),
        Some(index) => args
            .get(index + 1)
            .filter(|arg| !arg.starts_with("--"))
            .cloned()
            .map(Some)
            .ok_or_else(|| invalid(format!("Falta o valor de {flag}."))),
    }
}

fn option(args: &[String], flag: &str, environment: &str) -> io::Result<Option<String>> {
    Ok(value(args, flag)?.or_else(|| crate::branding::env_var(environment)))
}

fn public_key(value: &str) -> io::Result<[u8; 32]> {
    let value = value.trim();
    if value.len() != 64 || !value.is_ascii() {
        return Err(invalid(
            "A chave pública deve ter 64 caracteres hexadecimais.",
        ));
    }
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| invalid("A chave pública contém caracteres inválidos."))?;
    }
    Ok(key)
}

fn data_directory(args: &[String]) -> io::Result<PathBuf> {
    option(args, "--data", "LANVOX_DATA")?
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("LanvoxHost")))
        .ok_or_else(|| invalid("Informe a pasta de dados com --data."))
}

fn configuration(args: &[String], data: &Path) -> io::Result<(HostConfig, PathBuf)> {
    let explicit = option(args, "--config", "LANVOX_CONFIG")?.map(PathBuf::from);
    let path = explicit
        .clone()
        .unwrap_or_else(|| data.join("host-config.json"));
    let config = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Configuração inválida: {error}"),
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound && explicit.is_none() => {
            HostConfig::default()
        }
        Err(error) => return Err(error),
    };
    Ok((config, path))
}

fn configured(args: &[String], data: &Path) -> io::Result<(String, ServerOptions)> {
    let (config, path) = configuration(args, data)?;
    let bind = option(args, "--bind", "LANVOX_BIND")?.unwrap_or(config.bind);
    bind.parse::<std::net::IpAddr>()
        .map_err(|_| invalid("--bind deve ser um endereço IP local."))?;
    let port = match option(args, "--port", "LANVOX_PORT")? {
        Some(value) => value.parse().map_err(|_| invalid("Porta inválida."))?,
        None => config.port,
    };
    if port == 0 {
        return Err(invalid("A porta deve estar entre 1 e 65535."));
    }
    let max_connections = match option(args, "--max-connections", "LANVOX_MAX_CONNECTIONS")? {
        Some(value) => value
            .parse()
            .map_err(|_| invalid("Limite de conexões inválido."))?,
        None => config.max_connections,
    };
    if !(1..=512).contains(&max_connections) {
        return Err(invalid("O limite de conexões deve estar entre 1 e 512."));
    }
    let owner = option(args, "--owner-key", "LANVOX_OWNER_KEY")?.or(config.owner_key);
    let token_path = option(
        args,
        "--registration-token-file",
        "LANVOX_REGISTRATION_TOKEN_FILE",
    )?
    .map(PathBuf::from)
    .or(config.registration_token_file);
    let token = token_path
        .map(|token_path| {
            let token_path = if token_path.is_absolute() {
                token_path
            } else {
                path.parent().unwrap_or(data).join(token_path)
            };
            let bytes = crate::secret_store::read(&token_path)?;
            if bytes.len() > 512 {
                return Err(invalid("Arquivo de convite inválido."));
            }
            let token =
                String::from_utf8(bytes).map_err(|_| invalid("Arquivo de convite inválido."))?;
            let token = token.trim().to_string();
            if token.len() < 32 {
                return Err(invalid(
                    "O segredo de entrada deve ter ao menos 32 caracteres.",
                ));
            }
            Ok(token)
        })
        .transpose()?;
    let allowlist = config
        .allowlist
        .iter()
        .map(|key| public_key(key))
        .collect::<io::Result<_>>()?;
    Ok((
        option(args, "--name", "LANVOX_NAME")?.unwrap_or(config.name),
        ServerOptions {
            bind,
            port,
            registration_token: token,
            owner_key: owner.as_deref().map(public_key).transpose()?,
            allowlist,
            revoked_keys: config
                .revoked_keys
                .iter()
                .map(|key| public_key(key))
                .collect::<io::Result<_>>()?,
            max_connections,
            allow_registration: false,
        },
    ))
}

fn private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn initialize(args: &[String], data: &Path) -> io::Result<()> {
    use ring::rand::SecureRandom;
    std::fs::create_dir_all(data)?;
    let config_path = data.join("host-config.json");
    if config_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Já existe uma configuração nessa pasta.",
        ));
    }
    let owner_key = option(args, "--owner-key", "LANVOX_OWNER_KEY")?;
    if let Some(key) = &owner_key {
        public_key(key)?;
    }
    let mut secret = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut secret)
        .map_err(|_| io::Error::other("Não foi possível gerar o segredo."))?;
    let token: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();
    let token_name = "registration-token.secret";
    crate::secret_store::write_new(&data.join(token_name), token.as_bytes())?;
    let config = HostConfig {
        owner_key,
        registration_token_file: Some(token_name.into()),
        ..HostConfig::default()
    };
    private_file(&config_path, &serde_json::to_vec_pretty(&config)?)?;
    println!(
        "Configuração criada em {}. O segredo está no arquivo privado de entrada.",
        config_path.display()
    );
    println!("O bind inicial é 127.0.0.1. Configure o IP da LAN/VPN e a chave pública do proprietário antes do uso remoto.");
    Ok(())
}

/// Builds the shareable invitation. It necessarily carries the entry token (a bearer
/// secret) and the host fingerprint; no other protected material is read or printed.
fn invite(args: &[String], data: &Path) -> io::Result<String> {
    let (_, options) = configured(args, data)?;
    let token = options.registration_token.ok_or_else(|| invalid(
        "Este host não usa segredo de entrada. Crie um com --init-host ou configure registration_token_file."))?;
    let fingerprint = crate::auth::transport::HostKey::load_existing(&data.join("hosted-server.host-key"))
        .map_err(|error| if error.kind() == io::ErrorKind::NotFound {
            invalid("A chave do host ainda não existe. Inicie o host uma vez com --server e tente novamente.")
        } else { error })?
        .fingerprint();
    let advertised = match option(args, "--address", "LANVOX_PUBLIC_ADDRESS")? {
        Some(address) => address,
        None => {
            let bind: std::net::IpAddr = options
                .bind
                .parse()
                .map_err(|_| invalid("--bind inválido."))?;
            if bind.is_unspecified() {
                return Err(invalid("O host escuta em todas as interfaces. Informe o endereço que seus amigos usam com --address IP_OU_DNS[:PORTA]."));
            }
            bind.to_string()
        }
    };
    let advertised = crate::branding::strip_scheme(advertised.trim())
        .unwrap_or(advertised.trim())
        .to_string();
    if advertised.is_empty()
        || advertised
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '?' | '&' | '#' | '/'))
    {
        return Err(invalid("Endereço de convite inválido."));
    }
    let address = if advertised.starts_with('[') && advertised.contains("]:") {
        advertised
    } else if advertised.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{advertised}]:{}", options.port)
    } else if advertised.starts_with('[') {
        format!("{advertised}:{}", options.port)
    } else if advertised.contains(':') {
        advertised
    } else {
        format!("{advertised}:{}", options.port)
    };
    Ok(format!(
        "lanvox://{address}?token={token}&fingerprint={fingerprint}"
    ))
}

/// Puts the invite on the clipboard without echoing it to the terminal history.
fn copy_to_clipboard(text: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::io::Write;
        let mut child = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("clip indisponível"))?
            .write_all(text.as_bytes())?;
        if child.wait()?.success() {
            return Ok(());
        }
        Err(io::Error::other(
            "Não foi possível copiar para a área de transferência.",
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err(invalid(
            "--copy só está disponível no Windows; omita-o para imprimir o convite.",
        ))
    }
}

fn lease(data: &Path) -> io::Result<std::fs::File> {
    std::fs::create_dir_all(data)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("host.lock"))?;
    file.try_lock().map_err(|_| io::Error::new(io::ErrorKind::AddrInUse,
        "Outro host está usando essa pasta de dados. Pare-o antes de restaurar ou iniciar novamente."))?;
    Ok(file)
}

pub fn requested(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--server"
                | "--print-invite"
                | "--init-host"
                | "--backup"
                | "--restore"
                | "--check"
                | "--update-keygen"
                | "--update-sign"
                | "--help"
                | "-h"
                | "--version"
        )
    })
}

pub fn run(args: &[String]) -> io::Result<()> {
    if args.iter().any(|arg| arg == "--version") {
        println!(
            "Lanvox {} (protocolo {})",
            env!("CARGO_PKG_VERSION"),
            crate::auth::PROTOCOL_VERSION
        );
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Lanvox — host privado para LAN/VPN\n\n  --server [--data PASTA] [--bind IP] [--port PORTA]\n  --init-host --data PASTA [--owner-key CHAVE_PUBLICA_HEX]\n  --backup DESTINO --data PASTA\n  --restore ORIGEM --data PASTA\n  --check ENDERECO:PORTA\n  --print-invite --data PASTA [--address IP_OU_DNS[:PORTA]] [--copy]\n\nO host recusa novas identidades sem convite ou allowlist.\nConfiguração: host-config.json; variáveis LANVOX_BIND, LANVOX_PORT, LANVOX_DATA,\nLANVOX_NAME, LANVOX_OWNER_KEY, LANVOX_REGISTRATION_TOKEN_FILE.\nBackup usa SQLite online. Restauração exige host parado e destino novo.\n--print-invite mostra o convite lanvox:// (contém o segredo de entrada); --copy envia à área de transferência (Windows).\n--check verifica somente alcance TCP, não autenticação ou qualidade de mídia.");
        return Ok(());
    }
    if crate::updater::run_cli(args)? {
        return Ok(());
    }
    if let Some(address) = value(args, "--check")? {
        use std::net::ToSocketAddrs;
        let addresses: Vec<_> = address.to_socket_addrs()?.collect();
        let reachable = addresses.iter().any(|address| {
            std::net::TcpStream::connect_timeout(address, std::time::Duration::from_secs(3)).is_ok()
        });
        if !reachable {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "O host não respondeu na porta TCP informada.",
            ));
        }
        println!(
            "Porta TCP acessível. Confirme a identidade no aplicativo e teste a mídia pela VPN."
        );
        return Ok(());
    }
    let data = data_directory(args)?;
    if args.iter().any(|arg| arg == "--init-host") {
        return initialize(args, &data);
    }
    if args.iter().any(|arg| arg == "--print-invite") {
        let invite = invite(args, &data)?;
        if args.iter().any(|arg| arg == "--copy") {
            copy_to_clipboard(&invite)?;
            eprintln!("Convite copiado. Ele concede acesso ao host: compartilhe somente com quem deve entrar.");
        } else {
            eprintln!("O convite abaixo concede acesso ao host: compartilhe somente com quem deve entrar.");
            println!("{invite}");
        }
        return Ok(());
    }
    let database = data.join("hosted-server.sqlite3");
    if let Some(destination) = value(args, "--backup")? {
        crate::host_backup::backup(&data, Path::new(&destination))?;
        println!("Backup consistente criado em {destination}.");
        return Ok(());
    }
    if let Some(source) = value(args, "--restore")? {
        let _lease = lease(&data)?;
        if database.exists() || data.join("hosted-server.json").exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists,
                "Restaure em uma pasta nova para preservar os dados atuais. Depois aponte --data para ela."));
        }
        crate::host_backup::restore(Path::new(&source), &data)?;
        println!("Backup validado e restaurado em {}. Configure a identidade do host e o acesso antes de iniciar.", data.display());
        return Ok(());
    }
    let (name, options) = configured(args, &data)?;
    let _lease = lease(&data)?;
    let bind = options.bind.clone();
    let host = crate::net::server::start_configured(
        name.clone(),
        Some(data.join("hosted-server.json")),
        options,
    )?;
    println!(
        "Lanvox '{name}' em {bind}:{}. Dados em {}.",
        host.port,
        data.display()
    );
    println!("Entrada privada ativa. Pressione Ctrl+C para encerrar.");
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = running.clone();
    ctrlc::set_handler(move || flag.store(false, std::sync::atomic::Ordering::SeqCst))
        .map_err(io::Error::other)?;
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    host.stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_keys_are_strict_and_do_not_panic_on_unicode() {
        assert_eq!(public_key(&"ab".repeat(32)).unwrap(), [0xab; 32]);
        assert!(public_key(&"é".repeat(32)).is_err());
        assert!(public_key(&"zz".repeat(32)).is_err());
        assert!(public_key("abcd").is_err());
    }

    #[test]
    fn missing_option_values_are_errors() {
        assert!(value(&["--bind".into(), "--port".into()], "--bind").is_err());
        assert_eq!(
            value(&["--bind".into(), "127.0.0.1".into()], "--bind").unwrap(),
            Some("127.0.0.1".into())
        );
    }

    #[test]
    fn host_lease_excludes_second_process_and_releases_on_drop() {
        let dir = std::env::temp_dir().join(format!("lanvox-host-lease-{}", rand::random::<u64>()));
        let first = lease(&dir).unwrap();
        assert!(lease(&dir).is_err());
        drop(first);
        drop(lease(&dir).unwrap());
        std::fs::remove_file(dir.join("host.lock")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn invite_uses_protected_token_and_host_fingerprint_and_requires_a_routable_address() {
        let dir = std::env::temp_dir().join(format!("lanvox-invite-{}", rand::random::<u64>()));
        initialize(&[], &dir).unwrap();
        let args = |extra: &[&str]| -> Vec<String> {
            ["--data", dir.to_str().unwrap()]
                .iter()
                .chain(extra.iter())
                .map(|s| s.to_string())
                .collect()
        };
        // The host key must already exist: printing an invite never creates credentials.
        assert!(invite(&args(&[]), &dir).is_err());
        assert!(!dir.join("hosted-server.host-key").exists());
        let key =
            crate::auth::transport::HostKey::load_or_create(&dir.join("hosted-server.host-key"))
                .unwrap();
        let token = String::from_utf8(
            crate::secret_store::read(&dir.join("registration-token.secret")).unwrap(),
        )
        .unwrap();
        #[cfg(windows)]
        assert!(!std::fs::read(dir.join("registration-token.secret"))
            .unwrap()
            .windows(token.len())
            .any(|w| w == token.as_bytes()));
        let expected = format!(
            "lanvox://127.0.0.1:{DEFAULT_PORT}?token={token}&fingerprint={}",
            key.fingerprint()
        );
        assert_eq!(invite(&args(&[]), &dir).unwrap(), expected);
        let shared = invite(&args(&["--address", "friends.vpn"]), &dir).unwrap();
        assert!(shared.starts_with(&format!("lanvox://friends.vpn:{DEFAULT_PORT}?token=")));
        assert!(invite(&args(&["--address", "[fd00::1]:5000"]), &dir)
            .unwrap()
            .starts_with("lanvox://[fd00::1]:5000?"));
        assert!(invite(&args(&["--address", "a b"]), &dir).is_err());
        assert!(invite(&args(&["--bind", "0.0.0.0"]), &dir).is_err());
        assert!(
            invite(&args(&["--bind", "0.0.0.0", "--address", "10.0.0.2"]), &dir)
                .unwrap()
                .starts_with("lanvox://10.0.0.2:")
        );
        let (host, query) = shared
            .strip_prefix("lanvox://")
            .unwrap()
            .split_once('?')
            .unwrap();
        assert_eq!(host, format!("friends.vpn:{DEFAULT_PORT}"));
        assert!(query.contains(&format!("token={token}")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
