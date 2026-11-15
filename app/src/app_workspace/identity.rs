use super::*;

impl LanvoxApp {
    pub(super) fn record_identity_migration(
        &mut self,
        receipt: crate::settings::IdentityMigration,
    ) -> std::io::Result<()> {
        if self.settings.identity_migrations.contains(&receipt) {
            return Ok(());
        }
        let mut settings = self.settings.clone();
        settings.identity_migrations.push(receipt);
        settings.try_save()?;
        self.settings = settings;
        Ok(())
    }

    pub fn stage_connection_migration(&mut self, address: &str) -> std::io::Result<()> {
        if let Some(pending) = crate::auth::local::pending_rotation(address)? {
            self.record_identity_migration(crate::settings::IdentityMigration {
                host: crate::settings::host_address(address),
                user: pending.user,
                old_identity: crate::settings::key_fingerprint(pending.old_public_key),
                new_identity: crate::settings::key_fingerprint(pending.public_key),
            })?;
        }
        Ok(())
    }

    pub fn recover_identity_namespace(&mut self, user: UserId) {
        let Some(client) = &self.client else { return };
        let fingerprint = crate::settings::key_fingerprint(client.public_key);
        let host = crate::settings::host_address(&client.address);
        let receipts: Vec<_> = self
            .settings
            .identity_migrations
            .iter()
            .filter(|receipt| {
                receipt.host == host && receipt.user == user && receipt.new_identity == fingerprint
            })
            .cloned()
            .collect();
        if receipts.is_empty() {
            return;
        }
        let mut settings = self.settings.clone();
        for receipt in &receipts {
            settings.migrate_identity_scope(receipt);
        }
        match settings.try_save() {
            Ok(()) => {
                self.settings = settings;
                self.local_identity = fingerprint;
            }
            Err(error) => self.notify(format!(
                "Identidade recuperada; os rascunhos anteriores aguardam gravação local: {error}"
            )),
        }
    }

    pub fn begin_identity_rotation(&mut self) {
        let Some(client) = &self.client else { return };
        let address = client.address.clone();
        let user = self.me();
        let name = self
            .view
            .as_ref()
            .map(|view| view.user_name(user))
            .unwrap_or_default();
        let result = (|| -> std::io::Result<WorkspaceCommand> {
            let command = crate::auth::local::begin_rotation(&address, user, &name)?;
            if let WorkspaceCommand::RotateIdentity {
                old_public_key,
                new_public_key,
                ..
            } = &command
            {
                self.record_identity_migration(crate::settings::IdentityMigration {
                    host: crate::settings::host_address(&address),
                    user,
                    old_identity: crate::settings::key_fingerprint(*old_public_key),
                    new_identity: crate::settings::key_fingerprint(*new_public_key),
                })?;
            }
            Ok(command)
        })();
        match result {
            Ok(command) => {
                self.workspace(command);
                self.workspace_ui.rotation_confirm = false;
                self.notify("Rotação enviada. Aguarde a confirmação do host antes de exportar uma nova cópia.");
            }
            Err(error) => self.notify(format!("Não foi possível preparar a rotação: {error}")),
        }
    }

    pub(super) fn finish_identity_rotation(&mut self, public_key: [u8; 32], nonce: [u8; 32]) {
        let Some(client) = &self.client else { return };
        let address = client.address.clone();
        let user = self.me();
        let result = (|| -> std::io::Result<()> {
            if let Some(pending) = crate::auth::local::pending_rotation(&address)? {
                if pending.public_key != public_key
                    || pending.nonce != nonce
                    || pending.user != user
                {
                    return Err(std::io::Error::other(
                        "A confirmação recebida não corresponde à rotação deste perfil.",
                    ));
                }
                self.record_identity_migration(crate::settings::IdentityMigration {
                    host: crate::settings::host_address(&address),
                    user,
                    old_identity: crate::settings::key_fingerprint(pending.old_public_key),
                    new_identity: crate::settings::key_fingerprint(public_key),
                })?;
            }
            // Keep the durable rotation candidate until the local queue has moved safely.
            let fingerprint = crate::settings::key_fingerprint(public_key);
            let host = crate::settings::host_address(&address);
            let receipts: Vec<_> = self
                .settings
                .identity_migrations
                .iter()
                .filter(|receipt| {
                    receipt.host == host
                        && receipt.user == user
                        && receipt.new_identity == fingerprint
                })
                .cloned()
                .collect();
            let mut settings = self.settings.clone();
            for receipt in &receipts {
                settings.migrate_identity_scope(receipt);
            }
            settings.try_save()?;
            self.settings = settings;
            crate::auth::local::finish_rotation(&address, public_key, nonce)?;
            if let Some(client) = &mut self.client {
                client.public_key = public_key;
            }
            self.local_identity = fingerprint;
            Ok(())
        })();
        match result {
            Ok(()) => self.notify("Identidade rotacionada neste host. Exporte uma nova cópia; os outros hosts foram preservados."),
            Err(error) => self.notify(format!("A rotação precisa ser recuperada. Mantenha os arquivos locais e reconecte: {error}")),
        }
    }
}
