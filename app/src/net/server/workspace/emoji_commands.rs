use super::*;

impl State {
    pub(super) fn emoji_commands(
        &mut self,
        user: UserId,
        command: CommunityCommand,
    ) -> std::io::Result<()> {
        match command {
            CommunityCommand::UpsertEmoji {
                guild,
                id,
                name,
                image,
            } => {
                require(
                    self.guild_has(user, guild, permission::MANAGE_GUILD),
                    "Você não pode gerenciar emojis",
                )?;
                let name = name.trim().to_ascii_lowercase();
                require(
                    (2..=32).contains(&name.len())
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "Nome de emoji: 2 a 32 letras, números ou sublinhado",
                )?;
                require(!image.is_empty(), "Selecione uma imagem")
                    .and_then(|_| validate_visual(&image, 256 * 1024, 128))?;
                image::load_from_memory(&image).map_err(std::io::Error::other)?;
                require(
                    !self
                        .workspace
                        .community
                        .emojis
                        .iter()
                        .any(|e| e.guild == guild && e.name == name && Some(e.id) != id),
                    "Nome de emoji já utilizado",
                )?;
                if let Some(id) = id {
                    require(
                        self.workspace
                            .community
                            .emojis
                            .iter()
                            .any(|e| e.guild == guild && e.id == id),
                        "Emoji não encontrado",
                    )?;
                } else {
                    require(
                        self.workspace
                            .community
                            .emojis
                            .iter()
                            .filter(|e| e.guild == guild)
                            .count()
                            < 64,
                        "Limite de 64 emojis por comunidade",
                    )?;
                }
                let total = self
                    .workspace
                    .community
                    .emojis
                    .iter()
                    .filter(|e| Some(e.id) != id)
                    .map(|e| e.image.len())
                    .sum::<usize>();
                require(
                    total + image.len() <= 4 * 1024 * 1024,
                    "Armazenamento de emojis cheio",
                )?;
                let id = match id {
                    Some(id) => id,
                    None => self
                        .workspace
                        .next_emoji
                        .max(
                            self.workspace
                                .community
                                .emojis
                                .iter()
                                .map(|e| e.id)
                                .max()
                                .unwrap_or(0)
                                .checked_add(1)
                                .ok_or_else(|| {
                                    std::io::Error::other("Limite de emojis atingido")
                                })?,
                        )
                        .max(1),
                };
                require(id < i64::MAX as u64, "Limite de emojis atingido")?;
                let previous = self.workspace.clone();
                self.workspace.next_emoji = self.workspace.next_emoji.max(id + 1);
                self.workspace.community.emojis.retain(|e| e.id != id);
                self.workspace.community.emojis.push(CustomEmoji {
                    id,
                    guild,
                    name,
                    image,
                });
                self.audit(guild, user, None, "Emoji atualizado", "");
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            CommunityCommand::DeleteEmoji { guild, id } => {
                require(
                    self.guild_has(user, guild, permission::MANAGE_GUILD),
                    "Você não pode gerenciar emojis",
                )?;
                require(
                    self.workspace
                        .community
                        .emojis
                        .iter()
                        .any(|e| e.guild == guild && e.id == id),
                    "Emoji não encontrado",
                )?;
                let previous = self.workspace.clone();
                self.workspace.community.emojis.retain(|e| e.id != id);
                self.audit(guild, user, None, "Emoji removido", "");
                self.persist_workspace(previous)?;
                self.refresh_clients();
            }
            _ => unreachable!("routed by communitycommand"),
        }
        Ok(())
    }
}
