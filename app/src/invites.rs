//! Invitation links are data. Opening one never authenticates or runs a command automatically.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteLink {
    pub address: String,
    pub token: Option<String>,
    pub fingerprint: Option<String>,
    pub message: Option<(u32, u64)>,
}

pub fn parse(input: &str) -> Result<InviteLink, String> {
    let input = input.trim();
    if input.len() > 8192 || input.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("O convite contém caracteres inválidos.".into());
    }
    let value = crate::branding::strip_scheme(input).ok_or("Use um convite lanvox:// válido.")?;
    let (destination, query) = value.split_once('?').unwrap_or((value, ""));
    let (address, path) = destination.split_once('/').unwrap_or((destination, ""));
    if address.is_empty() || address.len() > 300 || address.contains(['/', '\\', '@', '#', '%']) {
        return Err("O endereço do convite é inválido.".into());
    }
    if let Some(ipv6) = address.strip_prefix('[') {
        let (ip, rest) = ipv6
            .split_once(']')
            .ok_or("Endereço IPv6 inválido no convite.")?;
        if ip.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("Endereço IPv6 inválido no convite.".into());
        }
        if !rest.is_empty()
            && rest
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok())
                .is_none_or(|p| p == 0)
        {
            return Err("A porta do convite é inválida.".into());
        }
    } else {
        let host = if let Some((host, port)) = address.split_once(':') {
            if port.parse::<u16>().ok().is_none_or(|p| p == 0) {
                return Err("A porta do convite é inválida.".into());
            }
            host
        } else {
            address
        };
        if host.is_empty()
            || host.len() > 253
            || !host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
            || host
                .trim_end_matches('.')
                .split('.')
                .any(|label| label.is_empty() || label.len() > 63)
        {
            return Err("O nome do host no convite é inválido.".into());
        }
    }
    let message = if path.is_empty() {
        None
    } else {
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() != 3 || parts[0] != "message" {
            return Err("O caminho do link é inválido.".into());
        }
        let channel = parts[1]
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("Canal inválido no link.")?;
        let message = parts[2]
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("Mensagem inválida no link.")?;
        Some((channel, message))
    };
    let mut link = InviteLink {
        address: address.to_ascii_lowercase(),
        token: None,
        fingerprint: None,
        message,
    };
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .ok_or("O convite tem um parâmetro inválido.")?;
        if value.is_empty() || value.len() > 4096 || value.contains(['#', '?']) {
            return Err("O convite tem um parâmetro inválido.".into());
        }
        match key {
            "token" | "invite" if link.token.is_none() => link.token = Some(value.into()),
            "fingerprint" if link.fingerprint.is_none() => link.fingerprint = Some(value.into()),
            _ => return Err("O convite contém parâmetros desconhecidos ou repetidos.".into()),
        }
    }
    Ok(link)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitations_do_not_accept_paths_controls_or_ambiguous_parameters() {
        let link = parse("lanvox://Friends:47800?token=abc&fingerprint=SHA256:123").unwrap();
        assert_eq!(link.address, "friends:47800");
        assert_eq!(link.token.as_deref(), Some("abc"));
        assert!(parse("lanvox://[::1]:47800?token=abc").is_ok());
        assert!(parse("lanvox://[2001:db8::abcd]?token=abc").is_ok());
        let message = parse("lanvox://friends:47800/message/42/900").unwrap();
        assert_eq!(message.message, Some((42, 900)));
        assert!(message.token.is_none());
        for value in [
            "https://friends",
            "lanvox://",
            "lanvox://host/path",
            "lanvox://host:0",
            "lanvox://host?token=a&token=b",
            "lanvox://host\n?token=a",
            "lanvox://host?run=x",
            "lanvox://[::1",
            "lanvox://[]:123",
            "lanvox://host/message/0/2",
            "lanvox://host/message/1/0",
            "lanvox://host/message/1/2/3",
            "lanvox://a:b:c",
            "lanvox://host|other",
        ] {
            assert!(parse(value).is_err(), "{value}");
        }
    }
}
