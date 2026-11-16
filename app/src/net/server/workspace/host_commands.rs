use super::*;

impl State {
    pub(super) fn host_commands(
        &mut self,
        user: UserId,
        command: WorkspaceCommand,
    ) -> std::io::Result<()> {
        match command {
            WorkspaceCommand::RotateIdentity {
                old_public_key,
                new_public_key,
                nonce,
                old_signature,
                new_signature,
            } => {
                require(
                    !self.is_revoked(&new_public_key),
                    "A nova identidade foi revogada",
                )?;
                require(
                    old_signature.len() == 64 && new_signature.len() == 64,
                    "Assinaturas de rotação inválidas",
                )?;
                let transcript =
                    identity_rotation_transcript(user, &old_public_key, &new_public_key, &nonce);
                let verify = |key: &[u8; 32], signature: &[u8]| {
                    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
                        .verify(&transcript, signature)
                        .is_ok()
                };
                require(
                    verify(&old_public_key, &old_signature)
                        && verify(&new_public_key, &new_signature),
                    "A rotação exige prova das duas chaves",
                )?;
                if self.workspace.rotations.iter().any(|r| {
                    r.user == user
                        && r.old == old_public_key
                        && r.new == new_public_key
                        && r.nonce == nonce
                }) && self.public_keys.get(&user) == Some(&new_public_key)
                {
                    self.send_to(
                        user,
                        &ServerMsg::Workspace(WorkspaceEvent::IdentityRotated {
                            public_key: new_public_key,
                            nonce,
                        }),
                    );
                    return Ok(());
                }
                require(
                    self.public_keys.get(&user) == Some(&old_public_key)
                        && !self.is_revoked(&old_public_key),
                    "A identidade anterior não corresponde à sessão",
                )?;
                require(
                    new_public_key != old_public_key
                        && !self.public_keys.values().any(|key| *key == new_public_key),
                    "A nova chave já está associada a uma identidade",
                )?;
                require(
                    self.workspace.revoked_keys.len() < 8192,
                    "Limite de chaves revogadas atingido",
                )?;
                let previous = self.workspace.clone();
                let previous_owner = self.options.owner_key;
                self.public_keys.insert(user, new_public_key);
                self.workspace.revoked_keys.push(old_public_key);
                self.workspace.rotations.retain(|r| r.user != user);
                self.workspace.rotations.push(RotationReceipt {
                    user,
                    old: old_public_key,
                    new: new_public_key,
                    nonce,
                });
                if self.options.owner_key == Some(old_public_key) {
                    self.options.owner_key = Some(new_public_key);
                }
                if let Err(e) = self.save_to_disk() {
                    self.workspace = previous;
                    self.public_keys.insert(user, old_public_key);
                    self.options.owner_key = previous_owner;
                    return Err(e);
                }
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::IdentityRotated {
                        public_key: new_public_key,
                        nonce,
                    }),
                );
            }
            WorkspaceCommand::SetHostAccess {
                user: target,
                allowed,
            } => {
                require(
                    self.workspace.host_owner == Some(user) && target != user,
                    "Somente o proprietário pode alterar o acesso de outras pessoas",
                )?;
                let key = *self
                    .public_keys
                    .get(&target)
                    .ok_or_else(|| std::io::Error::other("Identidade não encontrada"))?;
                require(
                    !allowed || !self.options.revoked_keys.contains(&key),
                    "Esta chave está revogada na política do host",
                )?;
                let previous = self.workspace.clone();
                if allowed {
                    self.workspace.revoked_keys.retain(|k| *k != key);
                } else if !self.workspace.revoked_keys.contains(&key) {
                    require(
                        self.workspace.revoked_keys.len() < 8192,
                        "Limite de chaves revogadas atingido",
                    )?;
                    self.workspace.revoked_keys.push(key);
                }
                self.persist_workspace(previous)?;
                if !allowed {
                    self.leave_voice(target);
                    if let Some(out) = self.clients.get(&target) {
                        out.close();
                    }
                }
                self.refresh_clients();
            }
            WorkspaceCommand::Backup => {
                require(
                    self.workspace.host_owner == Some(user),
                    "Somente o proprietário do host pode criar backup",
                )?;
                let path = self
                    .save_path
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("Host sem diretório persistente"))?
                    .with_extension(format!("backup-{}.sqlite3", now_secs()));
                require(!path.exists(), "Um backup já foi criado neste segundo")?;
                self.store
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("Banco indisponível"))?
                    .lock()
                    .backup(&path)?;
                self.send_to(
                    user,
                    &ServerMsg::Workspace(WorkspaceEvent::Notice(
                        "Backup criado no diretório do host".into(),
                    )),
                );
            }
            WorkspaceCommand::UpdateProfile {
                status,
                presence,
                avatar,
            } => {
                require(avatar.len() <= 256 * 1024, "Avatar deve ter até 256 KiB")?;
                if !avatar.is_empty() {
                    let image = image::ImageReader::new(std::io::Cursor::new(&avatar))
                        .with_guessed_format()
                        .map_err(std::io::Error::other)?;
                    let (w, h) = image.into_dimensions().map_err(std::io::Error::other)?;
                    require(
                        w <= 1024 && h <= 1024,
                        "Avatar deve ter até 1024×1024 pixels",
                    )?;
                }
                let total = self
                    .workspace
                    .profiles
                    .iter()
                    .filter(|p| p.user != user)
                    .map(|p| p.avatar.len())
                    .sum::<usize>();
                require(
                    total + avatar.len() <= 4 * 1024 * 1024,
                    "Limite de armazenamento de avatares atingido",
                )?;
                let previous = self.workspace.clone();
                self.workspace.profiles.retain(|p| p.user != user);
                self.workspace.profiles.push(Profile {
                    user,
                    status: clean(&status, 128),
                    presence,
                    avatar,
                });
                self.persist_workspace(previous)?;
                if let Some(profile) = self.workspace.profiles.iter().find(|p| p.user == user) {
                    self.broadcast(&ServerMsg::Workspace(WorkspaceEvent::ProfileUpdated(
                        profile.clone(),
                    )));
                }
                if let Some(user) = self.users.get(&user) {
                    self.broadcast(&ServerMsg::UserUpdated(user.clone()));
                }
            }
            _ => unreachable!("routed by workspacecommand"),
        }
        Ok(())
    }
}
