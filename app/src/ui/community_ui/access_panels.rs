use super::*;

pub fn roles(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    let mut roles = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .roles
                .iter()
                .filter(|r| r.guild == guild && r.position > 0)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    roles.sort_by_key(|r| r.position);
    let current_base = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .community
                .roles
                .iter()
                .find(|r| r.guild == guild && r.position == 0)
        })
        .cloned();
    let base_key = egui::Id::unique(("community-base-permissions", guild));
    let mut base = ui
        .ctx()
        .data(|d| d.get_temp::<CommunityRole>(base_key))
        .unwrap_or_else(|| {
            current_base.clone().unwrap_or(CommunityRole {
                guild,
                name: "Todos".into(),
                position: 0,
                permissions: permission::MEMBER,
                ..Default::default()
            })
        });
    if base.id == 0 {
        if let Some(saved) = current_base.as_ref() {
            base.id = saved.id;
        }
    }
    let owner = app.view.as_ref().is_some_and(|v| {
        v.workspace.host_owner == Some(v.me)
            || v.workspace
                .memberships
                .iter()
                .any(|m| m.guild == guild && m.user == v.me && m.role == MemberRole::Owner)
    });
    ui.collapsing("Permissões básicas de todos os membros", |ui| {
        ui.label("Estas permissões são a base da comunidade. Cargos podem conceder direitos adicionais; regras de categoria e canal podem substituí-los.");
        ui.add_enabled_ui(owner, |ui| {
            for &(bit, label) in CAPABILITIES.iter().filter(|(bit, _)| *bit & permission::MEMBER != 0) {
                let mut enabled = base.permissions & bit != 0;
                if ui.checkbox(&mut enabled, label).changed() {
                    if enabled { base.permissions |= bit; } else { base.permissions &= !bit; }
                }
            }
            if ui.button("Salvar permissões básicas").clicked() {
                command(app, CommunityCommand::UpsertRole { role: base.clone() });
                // Reload the assigned identifier from the next authoritative snapshot.
                ui.ctx().data_mut(|d| d.remove::<CommunityRole>(base_key));
            } else {
                ui.ctx().data_mut(|d| d.insert_temp(base_key, base.clone()));
            }
        });
        if !owner { ui.label("Somente o proprietário pode alterar as permissões básicas."); }
    });
    ui.label("Cargos concedem capacidades adicionais. Desmarcar uma capacidade não a remove da base nem de outros cargos; para restringi-la em um canal, use as sobrescritas.");
    ui.label(
        RichText::new("A posição define a hierarquia administrativa.")
            .small()
            .color(theme::TEXT_MUTED),
    );
    ui.horizontal_wrapped(|ui| {
        if ui.button("Novo cargo").clicked() {
            app.workspace_ui.extras.role = CommunityRole {
                guild,
                color: 0x0e9488,
                position: 1,
                permissions: permission::MEMBER,
                ..Default::default()
            };
        }
        for role in &roles {
            if ui
                .selectable_label(
                    app.workspace_ui.extras.role.id == role.id,
                    RichText::new(&role.name).color(theme::readable_text(
                        color32(role.color),
                        theme::color(theme::CHAT),
                    )),
                )
                .clicked()
            {
                app.workspace_ui.extras.role = role.clone();
            }
        }
    });
    if app.workspace_ui.extras.role.id == 0 && app.workspace_ui.extras.role.guild != guild {
        ui.label("Selecione um cargo para editar ou escolha Novo cargo.");
        return;
    }
    let mut role = app.workspace_ui.extras.role.clone();
    if role.id == 0 && role.position == 0 {
        role.position = 1;
    }
    if role.id == 0 {
        if let Some(saved) = roles
            .iter()
            .find(|r| !role.name.is_empty() && r.name == role.name)
        {
            role.id = saved.id;
        }
    }
    role.guild = guild;
    components::form_row(ui, "Nome do cargo", |ui| {
        components::text_field(ui, &mut role.name, "Ex.: Moderador", 40);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Cor");
        let mut color = color32(role.color);
        if ui.color_edit_button_srgba(&mut color).changed() {
            role.color = ((color.r() as u32) << 16) | ((color.g() as u32) << 8) | color.b() as u32;
        }
        ui.label("Posição");
        ui.add(egui::DragValue::new(&mut role.position).range(1..=999));
    });
    ui.add_space(8.0);
    ui.label(RichText::new("Permissões do cargo").strong());
    for &(bit, label) in CAPABILITIES {
        if bit == permission::CONNECT {
            ui.add_space(6.0);
            ui.label(RichText::new("Voz e transmissão").strong());
        } else if bit == permission::MANAGE_MESSAGES {
            ui.add_space(6.0);
            ui.label(RichText::new("Administração e moderação").strong());
        }
        let mut enabled = role.permissions & bit != 0;
        if ui.checkbox(&mut enabled, label).changed() {
            if enabled {
                role.permissions |= bit;
            } else {
                role.permissions &= !bit;
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !role.name.trim().is_empty(),
                egui::Button::new("Salvar cargo").fill(theme::color(theme::ACTION_FILL)),
            )
            .clicked()
        {
            command(app, CommunityCommand::UpsertRole { role: role.clone() });
        }
        if role.id != 0 && ui.button("Excluir cargo…").clicked() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::unique(("confirm-delete-role", guild)),
                    Some((role.id, role.name.clone())),
                )
            });
        }
    });
    let confirm_key = egui::Id::unique(("confirm-delete-role", guild));
    let confirmation = ui
        .ctx()
        .data(|d| d.get_temp::<Option<(RoleId, String)>>(confirm_key))
        .flatten();
    if let Some((id, name)) = confirmation {
        ui.add_space(8.0);
        ui.colored_label(
            theme::YELLOW,
            format!("Excluir o cargo {name}? Ele será removido dos membros."),
        );
        ui.horizontal_wrapped(|ui| {
            if components::danger_button(ui, "Confirmar exclusão").clicked() {
                command(app, CommunityCommand::DeleteRole { guild, role: id });
                role = CommunityRole::default();
                ui.ctx()
                    .data_mut(|d| d.remove::<Option<(RoleId, String)>>(confirm_key));
            }
            if ui.button("Cancelar").clicked() {
                ui.ctx()
                    .data_mut(|d| d.remove::<Option<(RoleId, String)>>(confirm_key));
            }
        });
    }
    app.workspace_ui.extras.role = role;
}

pub fn members(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    components::form_row(ui, "Motivo para restrição temporária", |ui| {
        components::text_field(
            ui,
            &mut app.workspace_ui.extras.moderation_reason,
            "Opcional",
            240,
        );
    });
    let people: Vec<_> = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .memberships
                .iter()
                .filter(|m| m.guild == guild)
                .map(|m| (m.clone(), v.user_name(m.user)))
                .collect()
        })
        .unwrap_or_default();
    let roles = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .roles
                .iter()
                .filter(|r| r.guild == guild && r.position > 0)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (member, name) in people {
        ui.push_id(member.user, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.link(&name).clicked() {
                    app.open_profile(member.user);
                }
                ui.label(match member.role {
                    MemberRole::Owner => "Proprietário",
                    MemberRole::Admin => "Administrador",
                    MemberRole::Member => "Membro",
                });
                if member.user != app.me() && member.role != MemberRole::Owner {
                    ui.menu_button("Ações", |ui| {
                        if ui
                            .button(if member.role == MemberRole::Admin {
                                "Tornar membro"
                            } else {
                                "Tornar administrador"
                            })
                            .clicked()
                        {
                            app.workspace(WorkspaceCommand::SetRole {
                                guild,
                                user: member.user,
                                role: if member.role == MemberRole::Admin {
                                    MemberRole::Member
                                } else {
                                    MemberRole::Admin
                                },
                            });
                            ui.close();
                        }
                        if ui.button("Remover da comunidade…").clicked() {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::unique(("confirm-member-action", guild)),
                                    Some((member.user, name.clone(), false)),
                                )
                            });
                            ui.close();
                        }
                        if ui.button("Banir…").clicked() {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::unique(("confirm-member-action", guild)),
                                    Some((member.user, name.clone(), true)),
                                )
                            });
                            ui.close();
                        }
                        if ui.button("Silenciar por 10 minutos").clicked() {
                            command(
                                app,
                                CommunityCommand::Timeout {
                                    guild,
                                    user: member.user,
                                    until: now_secs() + 600,
                                    reason: app.workspace_ui.extras.moderation_reason.clone(),
                                },
                            );
                            ui.close();
                        }
                        if ui.button("Remover restrição temporária").clicked() {
                            command(
                                app,
                                CommunityCommand::Timeout {
                                    guild,
                                    user: member.user,
                                    until: 0,
                                    reason: String::new(),
                                },
                            );
                            ui.close();
                        }
                    });
                }
            });
            let mut assigned = app
                .view
                .as_ref()
                .and_then(|v| {
                    v.workspace
                        .community
                        .assignments
                        .iter()
                        .find(|a| a.guild == guild && a.user == member.user)
                })
                .map(|a| a.roles.clone())
                .unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                for role in &roles {
                    let mut selected = assigned.contains(&role.id);
                    if ui
                        .checkbox(
                            &mut selected,
                            RichText::new(&role.name).color(theme::readable_text(
                                color32(role.color),
                                theme::color(theme::CHAT),
                            )),
                        )
                        .changed()
                    {
                        if selected {
                            assigned.push(role.id);
                        } else {
                            assigned.retain(|id| *id != role.id);
                        }
                        command(
                            app,
                            CommunityCommand::AssignRoles {
                                guild,
                                user: member.user,
                                roles: assigned.clone(),
                            },
                        );
                    }
                }
            });
            ui.separator();
        });
    }
    let confirm_key = egui::Id::unique(("confirm-member-action", guild));
    let confirmation = ui
        .ctx()
        .data(|d| d.get_temp::<Option<(UserId, String, bool)>>(confirm_key))
        .flatten();
    if let Some((user, name, ban)) = confirmation {
        ui.separator();
        ui.colored_label(theme::YELLOW, if ban { format!("Banir {name}? Essa pessoa perderá o acesso e não poderá retornar enquanto estiver banida.") } else { format!("Remover {name} da comunidade? A pessoa perderá acesso aos canais.") });
        ui.horizontal_wrapped(|ui| {
            if components::danger_button(
                ui,
                if ban {
                    "Confirmar banimento"
                } else {
                    "Confirmar remoção"
                },
            )
            .clicked()
            {
                app.workspace(WorkspaceCommand::RemoveMember { guild, user, ban });
                ui.ctx()
                    .data_mut(|d| d.remove::<Option<(UserId, String, bool)>>(confirm_key));
            }
            if ui.button("Cancelar").clicked() {
                ui.ctx()
                    .data_mut(|d| d.remove::<Option<(UserId, String, bool)>>(confirm_key));
            }
        });
    }
}

pub fn invites(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    ui.label("Crie convites com prazo e quantidade de usos definidos.");
    ui.horizontal_wrapped(|ui| {
        ui.label("Validade em horas");
        ui.add(egui::DragValue::new(&mut app.workspace_ui.extras.invite_hours).range(1..=720));
        ui.label("Usos");
        ui.add(egui::DragValue::new(&mut app.workspace_ui.extras.invite_uses).range(1..=1000));
    });
    ui.horizontal_wrapped(|ui| {
        if components::primary_button(ui, "Criar convite").clicked() {
            app.workspace(WorkspaceCommand::CreateInvite {
                guild,
                max_uses: app.workspace_ui.extras.invite_uses.max(1),
                expires_hours: app.workspace_ui.extras.invite_hours.max(1),
            });
        }
        if ui.button("Atualizar lista").clicked() {
            command(app, CommunityCommand::Invites { guild });
        }
    });
    if !app.workspace_ui.invite_created.is_empty() {
        ui.label(RichText::new(&app.workspace_ui.invite_created).monospace());
        if ui.button("Copiar novo convite").clicked() {
            ui.ctx().copy_text(app.workspace_ui.invite_created.clone());
        }
    }
    let invites = app.workspace_ui.extras.invites.clone();
    if invites.is_empty() {
        ui.label("Nenhum convite carregado. Use Atualizar lista para consultar o host.");
    }
    for invite in invites.iter().filter(|i| i.guild == guild) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&invite.code).monospace());
            ui.label(format!(
                "{} usos restantes · {}",
                invite.remaining,
                format_time(invite.expires)
            ));
            if ui.small_button("Copiar").clicked() {
                ui.ctx().copy_text(invite.code.clone());
            }
            if ui.small_button("Revogar").clicked() {
                app.workspace(WorkspaceCommand::RevokeInvite {
                    code: invite.code.clone(),
                });
                command(app, CommunityCommand::Invites { guild });
            }
        });
    }
}

pub fn moderation(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    if ui.button("Atualizar moderação").clicked() {
        command(app, CommunityCommand::Moderation { guild });
    }
    ui.heading("Restrições temporárias");
    let timeouts = app.workspace_ui.extras.timeouts.clone();
    for timeout in timeouts
        .iter()
        .filter(|t| t.guild == guild && t.until > now_secs())
    {
        let name = app
            .view
            .as_ref()
            .map(|v| v.user_name(timeout.user))
            .unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{name} · até {}", format_time(timeout.until)));
            if ui.small_button("Remover restrição").clicked() {
                command(
                    app,
                    CommunityCommand::Timeout {
                        guild,
                        user: timeout.user,
                        until: 0,
                        reason: String::new(),
                    },
                );
            }
        });
    }
    ui.add_space(12.0);
    ui.heading("Pessoas banidas");
    let bans = app.workspace_ui.extras.bans.clone();
    if bans.iter().all(|b| b.guild != guild) {
        ui.label("Nenhum banimento carregado.");
    }
    for ban in bans.iter().filter(|b| b.guild == guild) {
        let name = app
            .view
            .as_ref()
            .map(|v| v.user_name(ban.user))
            .unwrap_or_else(|| format!("Pessoa {}", ban.user));
        ui.horizontal_wrapped(|ui| {
            ui.label(name);
            if ui.small_button("Desbanir").clicked() {
                command(
                    app,
                    CommunityCommand::Unban {
                        guild,
                        user: ban.user,
                    },
                );
                command(app, CommunityCommand::Moderation { guild });
            }
        });
    }
    ui.add_space(12.0);
    ui.heading("Registro de ações");
    let audit = app.workspace_ui.extras.audit.clone();
    for entry in audit.iter().filter(|a| a.guild == guild).rev() {
        let actor = app
            .view
            .as_ref()
            .map(|v| v.user_name(entry.actor))
            .unwrap_or_default();
        ui.label(
            RichText::new(format!(
                "{} · {actor} · {}",
                format_time(entry.time),
                entry.action
            ))
            .strong(),
        );
        if !entry.reason.is_empty() {
            ui.label(&entry.reason);
        }
        ui.separator();
    }
}
