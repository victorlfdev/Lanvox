use super::*;

pub fn channel_policy(app: &mut LanvoxApp, ui: &mut Ui, channel: &Channel) {
    let mut policy = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .community
                .policies
                .iter()
                .find(|p| p.channel == channel.id)
        })
        .cloned()
        .unwrap_or(ChannelPolicy {
            channel: channel.id,
            ..Default::default()
        });
    let key = egui::Id::unique(("channel-policy", channel.id));
    policy = ui
        .ctx()
        .data(|d| d.get_temp::<ChannelPolicy>(key))
        .unwrap_or(policy);
    ui.separator();
    ui.heading("Conversa e permissões");
    let members: Vec<_> = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .memberships
                .iter()
                .filter(|m| m.guild == channel.guild)
                .map(|m| (m.user, v.user_name(m.user)))
                .collect()
        })
        .unwrap_or_default();
    let me = app.me();
    ui.collapsing("Ver permissões efetivas", |ui| {
        let key = egui::Id::unique(("effective-person", channel.id));
        let mut target = ui.ctx().data(|d| d.get_temp::<UserId>(key)).filter(|id| members.iter().any(|m| m.0 == *id)).unwrap_or(me);
        egui::ComboBox::from_id_salt(("effective-person-picker", channel.id))
            .selected_text(members.iter().find(|(id, _)| *id == target).map(|(_, name)| name.as_str()).unwrap_or("Selecione uma pessoa"))
            .show_ui(ui, |ui| { for (id, name) in &members { ui.selectable_value(&mut target, *id, name); } });
        ui.ctx().data_mut(|d| d.insert_temp(key, target));
        ui.label("Resultado calculado a partir da base, cargos e regras da categoria e do canal. Restrições temporárias e tipo de canal podem limitar ações; o host confirma cada operação.");
        if let Some(view) = app.view.as_ref() {
            let effective = view.channel_permissions_for(channel.id, target);
            for &(bit, label) in CAPABILITIES {
                let allowed = effective & permission::VIEW != 0 && effective & bit == bit;
                ui.label(format!("{} · {}", if allowed { "Permitido" } else { "Não permitido" }, label));
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Intervalo entre mensagens");
        ui.add(
            egui::DragValue::new(&mut policy.slowmode_secs)
                .range(0..=21600)
                .suffix(" s"),
        );
    });
    egui::ComboBox::from_id_salt("channel-style")
        .selected_text(style_name(policy.style))
        .show_ui(ui, |ui| {
            for style in [
                ChannelStyle::Standard,
                ChannelStyle::Forum,
                ChannelStyle::Announcement,
                ChannelStyle::Stage,
            ]
            .into_iter()
            .filter(|style| {
                if channel.kind == ChannelKind::Voice {
                    matches!(style, ChannelStyle::Standard | ChannelStyle::Stage)
                } else {
                    *style != ChannelStyle::Stage
                }
            }) {
                ui.selectable_value(&mut policy.style, style, style_name(style));
            }
        });
    if ui.button("Salvar tipo e intervalo").clicked() {
        command(
            app,
            CommunityCommand::SetChannelPolicy {
                policy: policy.clone(),
            },
        );
    }
    ui.ctx().data_mut(|d| d.insert_temp(key, policy));
    ui.collapsing("Sobrescrever permissões", |ui| {
        ui.label("Herdar mantém o valor anterior; permitir ou negar altera o direito neste escopo. A ordem é: todos, cargos e pessoa. As regras do canal prevalecem sobre as da categoria.");
        let roles = app
            .view
            .as_ref()
            .map(|v| {
                v.workspace
                    .community
                    .roles
                    .iter()
                    .filter(|r| r.guild == channel.guild && r.position > 0)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut app.workspace_ui.extras.override_kind, 0, "Todos");
            ui.selectable_value(&mut app.workspace_ui.extras.override_kind, 1, "Cargo");
            ui.selectable_value(&mut app.workspace_ui.extras.override_kind, 2, "Pessoa");
        });
        if app.workspace_ui.extras.override_kind == 1 {
            egui::ComboBox::from_id_salt("override-role")
                .selected_text(
                    roles
                        .iter()
                        .find(|r| r.id == app.workspace_ui.extras.override_target)
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| "Escolha um cargo".into()),
                )
                .show_ui(ui, |ui| {
                    for role in &roles {
                        ui.selectable_value(
                            &mut app.workspace_ui.extras.override_target,
                            role.id,
                            &role.name,
                        );
                    }
                });
        }
        if app.workspace_ui.extras.override_kind == 2 {
            let users = app
                .view
                .as_ref()
                .map(|v| {
                    v.workspace
                        .memberships
                        .iter()
                        .filter(|m| m.guild == channel.guild)
                        .map(|m| (m.user, v.user_name(m.user)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("override-person")
                .selected_text(
                    users
                        .iter()
                        .find(|u| u.0 == app.workspace_ui.extras.override_target)
                        .map(|u| u.1.clone())
                        .unwrap_or_else(|| "Escolha uma pessoa".into()),
                )
                .show_ui(ui, |ui| {
                    for (id, name) in users {
                        ui.selectable_value(&mut app.workspace_ui.extras.override_target, id, name);
                    }
                });
        }
        let loaded = (
            channel.id,
            app.workspace_ui.extras.override_kind,
            app.workspace_ui.extras.override_target,
        );
        if app.workspace_ui.extras.override_loaded != Some(loaded) {
            let target = match app.workspace_ui.extras.override_kind {
                1 => PermissionTarget::Role(app.workspace_ui.extras.override_target),
                2 => PermissionTarget::Member(app.workspace_ui.extras.override_target),
                _ => PermissionTarget::Everyone,
            };
            let current =
                app.view.as_ref().and_then(|v| {
                    v.workspace.community.overrides.iter().find(|o| {
                        o.scope == PermissionScope::Channel(channel.id) && o.target == target
                    })
                });
            app.workspace_ui.extras.override_allow = current.map(|o| o.allow).unwrap_or(0);
            app.workspace_ui.extras.override_deny = current.map(|o| o.deny).unwrap_or(0);
            app.workspace_ui.extras.override_loaded = Some(loaded);
        }
        for &(bit, label) in CAPABILITIES {
            let mut state = if app.workspace_ui.extras.override_deny & bit != 0 {
                2
            } else if app.workspace_ui.extras.override_allow & bit != 0 {
                1
            } else {
                0
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(label);
                ui.selectable_value(&mut state, 0, "Herdar");
                ui.selectable_value(&mut state, 1, "Permitir");
                ui.selectable_value(&mut state, 2, "Negar");
            });
            app.workspace_ui.extras.override_allow &= !bit;
            app.workspace_ui.extras.override_deny &= !bit;
            if state == 1 {
                app.workspace_ui.extras.override_allow |= bit;
            } else if state == 2 {
                app.workspace_ui.extras.override_deny |= bit;
            }
        }
        let target = match app.workspace_ui.extras.override_kind {
            1 => PermissionTarget::Role(app.workspace_ui.extras.override_target),
            2 => PermissionTarget::Member(app.workspace_ui.extras.override_target),
            _ => PermissionTarget::Everyone,
        };
        ui.horizontal_wrapped(|ui| {
            if ui.button("Salvar no canal").clicked() {
                command(
                    app,
                    CommunityCommand::SetOverride {
                        value: PermissionOverride {
                            scope: PermissionScope::Channel(channel.id),
                            target,
                            allow: app.workspace_ui.extras.override_allow,
                            deny: app.workspace_ui.extras.override_deny,
                        },
                    },
                );
            }
            if ui.button("Remover do canal").clicked() {
                command(
                    app,
                    CommunityCommand::RemoveOverride {
                        scope: PermissionScope::Channel(channel.id),
                        target,
                    },
                );
            }
            if let Some(category) = app.workspace_ui.channel_category {
                if ui.button("Aplicar à categoria").clicked() {
                    command(
                        app,
                        CommunityCommand::SetOverride {
                            value: PermissionOverride {
                                scope: PermissionScope::Category(category),
                                target,
                                allow: app.workspace_ui.extras.override_allow,
                                deny: app.workspace_ui.extras.override_deny,
                            },
                        },
                    );
                }
            }
        });
    });
}

pub fn notifications(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    use crate::settings::{NotificationLevel, NotificationPreference};
    let scope = app.notification_scope();
    let key = format!("{scope}|{guild}");
    let mut pref = app
        .settings
        .guild_notifications
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let before = pref;
    ui.label("Escolha quais mensagens desta comunidade devem chamar sua atenção.");
    for (level, label) in [
        (NotificationLevel::All, "Todas as mensagens"),
        (NotificationLevel::Mentions, "Somente menções"),
        (NotificationLevel::None, "Nenhuma"),
    ] {
        ui.radio_value(&mut pref.level, level, label);
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Silenciar por 1 hora").clicked() {
            pref.muted_until = now_secs() + 3600;
        }
        if ui.button("Silenciar por 8 horas").clicked() {
            pref.muted_until = now_secs() + 28800;
        }
        if ui.button("Reativar").clicked() {
            pref.muted_until = 0;
        }
    });
    if pref.muted_until > now_secs() {
        ui.label(format!("Silenciada até {}", format_time(pref.muted_until)));
    }
    if pref.level != before.level || pref.muted_until != before.muted_until {
        app.set_guild_notifications(
            guild,
            NotificationPreference {
                level: pref.level,
                muted_until: pref.muted_until,
            },
        );
    }
}
