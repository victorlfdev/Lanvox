//! Accessible forms for profiles, private groups and community administration.
use super::{components, theme};
use crate::{app::LanvoxApp, model::*};
use egui::vec2;
type EmojiImageTask =
    crossbeam_channel::Receiver<Result<Option<(GuildId, String, Vec<u8>)>, String>>;
type CommunityImageTask =
    crossbeam_channel::Receiver<Result<Option<(GuildId, bool, image::DynamicImage)>, String>>;
type ConversationImageTask =
    crossbeam_channel::Receiver<Result<Option<(ChannelId, Vec<u8>)>, String>>;
#[derive(Default)]
pub struct UiExtras {
    pub search_draft: Option<SearchRequest>,
    pub search_signature: String,
    pub emoji_name: String,
    pub emoji_pending: Option<EmojiImageTask>,
    pub event: ScheduledEvent,
    pub event_date: String,
    pub event_end: String,
    pub invites: Vec<InviteInfo>,
    pub bans: Vec<BanInfo>,
    pub timeouts: Vec<MemberTimeout>,
    pub audit: Vec<AuditEntry>,
    pub profile_user: Option<UserId>,
    pub profile_bio: String,
    pub profile_nickname: String,
    pub profile_guild: Option<GuildId>,
    pub group_filter: String,
    pub admin_tab: u8,
    pub admin_guild: Option<GuildId>,
    pub invite_hours: u16,
    pub invite_uses: u16,
    pub role: CommunityRole,
    pub moderation_reason: String,
    pub thread_name: String,
    pub thread_tags: String,
    pub thread_tags_channel: Option<ChannelId>,
    pub forum_filter: String,
    pub override_loaded: Option<(ChannelId, u8, u32)>,
    pub override_target: u32,
    pub override_kind: u8,
    pub override_allow: u64,
    pub override_deny: u64,
    pub inbox_mentions: bool,
    pub visual_pending: Option<CommunityImageTask>,
    pub visual_editor: Option<super::community_ui::VisualEditor>,
    pub conversation_image_pending: Option<ConversationImageTask>,
}
pub fn show(app: &mut LanvoxApp, ctx: &egui::Context) {
    super::community_ui::poll_visual(app);
    super::community_ui::visual_editor(app, ctx);
    edit_message(app, ctx);
    host_access_confirmation(app, ctx);
    profile(app, ctx);
    group(app, ctx);
    manage(app, ctx);
}
fn edit_message(app: &mut LanvoxApp, ctx: &egui::Context) {
    if let Some(message) = app.workspace_ui.deleting {
        let result = components::modal(
            ctx,
            "delete-message",
            "Excluir esta mensagem?",
            components::ModalOptions::new(360.0)
                .primary("Excluir mensagem")
                .danger()
                .secondary("Cancelar"),
            |ui| {
                ui.label(
                    "A mensagem será removida para os participantes. Esta ação não pode ser desfeita.",
                );
            },
        );
        let remove = result.primary;
        let cancel = result.cancelled();
        if remove {
            app.workspace(WorkspaceCommand::Delete { message });
        }
        if remove || cancel {
            app.workspace_ui.deleting = None;
        }
    }
}
fn profile(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some(user) = app.workspace_ui.profile else {
        return;
    };
    let name = app
        .view
        .as_ref()
        .map(|v| v.user_name(user))
        .unwrap_or_default();
    let own = user == app.me();
    let guild = if app.home { None } else { app.guild };
    if app.workspace_ui.extras.profile_user != Some(user)
        || app.workspace_ui.extras.profile_guild != guild
    {
        let extra = app
            .view
            .as_ref()
            .and_then(|v| {
                v.workspace
                    .community
                    .profile_extras
                    .iter()
                    .filter(|p| p.user == user && (p.guild == guild || p.guild.is_none()))
                    .max_by_key(|p| u8::from(p.guild == guild))
            })
            .cloned()
            .unwrap_or_default();
        app.workspace_ui.extras.profile_user = Some(user);
        app.workspace_ui.extras.profile_guild = guild;
        app.workspace_ui.extras.profile_bio = extra.bio;
        app.workspace_ui.extras.profile_nickname = extra.nickname;
    }
    let result = components::modal(
        ctx,
        "profile-card",
        if own { "Seu perfil" } else { &name },
        components::ModalOptions::new(440.0)
            .primary(if own {
                "Salvar perfil"
            } else {
                "Enviar mensagem"
            })
            .secondary("Fechar"),
        |ui| {
            let texture = app.avatar_texture(ctx, user);
            components::profile_card(
                ui,
                &name,
                Some(presence_label(app.workspace_ui.profile_presence)),
                Some(&app.workspace_ui.extras.profile_bio),
                |ui| match &texture {
                    Some(t) => {
                        ui.add(egui::Image::new((t.id(), vec2(80.0, 80.0))).corner_radius(40));
                    }
                    None => components::initials_avatar(ui, 80.0, &name),
                },
            );
            if !app.workspace_ui.profile_status.is_empty() {
                super::widgets::section_label(ui, "Status");
                ui.label(&app.workspace_ui.profile_status);
            }
            let roles: Vec<_> = app
                .view
                .as_ref()
                .map(|v| {
                    v.workspace
                        .community
                        .roles
                        .iter()
                        .filter(|r| {
                            Some(r.guild) == guild
                                && v.workspace.community.assignments.iter().any(|a| {
                                    a.user == user && a.guild == r.guild && a.roles.contains(&r.id)
                                })
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                for role in &roles {
                    ui.colored_label(color32(role.color), &role.name);
                }
            });
            if own {
                components::form_row(ui, "Status", |ui| {
                    components::text_field(
                        ui,
                        &mut app.workspace_ui.profile_status,
                        "O que está acontecendo?",
                        128,
                    );
                });
                egui::ComboBox::from_id_salt("profile-presence")
                    .selected_text(presence_label(app.workspace_ui.profile_presence))
                    .show_ui(ui, |ui| {
                        for p in [
                            Presence::Online,
                            Presence::Away,
                            Presence::DoNotDisturb,
                            Presence::Invisible,
                        ] {
                            ui.selectable_value(
                                &mut app.workspace_ui.profile_presence,
                                p,
                                presence_label(p),
                            );
                        }
                    });
                components::form_row(ui, "Sobre mim", |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut app.workspace_ui.extras.profile_bio)
                            .desired_rows(3)
                            .char_limit(500)
                            .desired_width(f32::INFINITY),
                    );
                });
                if guild.is_some() {
                    components::form_row(ui, "Apelido nesta comunidade", |ui| {
                        components::text_field(
                            ui,
                            &mut app.workspace_ui.extras.profile_nickname,
                            "Seu nome nesta comunidade",
                            32,
                        );
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if components::secondary_button(ui, "Escolher avatar").clicked() {
                        app.pick_avatar();
                    }
                    if components::secondary_button(ui, "Remover avatar").clicked() {
                        app.workspace_ui.profile_avatar.clear();
                    }
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Chamar").clicked() {
                        app.start_call(vec![user]);
                    }
                    if ui.button("Adicionar amigo").clicked() {
                        app.send(crate::protocol::ClientMsg::FriendRequest { name: name.clone() });
                    }
                    let blocked = app.is_blocked(user);
                    if ui
                        .button(if blocked {
                            "Desbloquear"
                        } else {
                            "Bloquear pessoa"
                        })
                        .clicked()
                    {
                        app.set_blocked(user, !blocked);
                    }
                });
            }
        },
    );
    if result.primary {
        if own {
            app.settings.do_not_disturb =
                app.workspace_ui.profile_presence == Presence::DoNotDisturb;
            app.settings.save();
            app.workspace(WorkspaceCommand::UpdateProfile {
                status: app.workspace_ui.profile_status.clone(),
                presence: app.workspace_ui.profile_presence,
                avatar: app.workspace_ui.profile_avatar.clone(),
            });
            app.workspace(WorkspaceCommand::Community(
                CommunityCommand::UpdateProfileExtra {
                    guild,
                    bio: app.workspace_ui.extras.profile_bio.clone(),
                    nickname: app.workspace_ui.extras.profile_nickname.clone(),
                },
            ));
        } else {
            app.open_conversation(name, vec![user], true);
        }
    }
    if result.primary || result.cancelled() {
        app.workspace_ui.profile = None;
        app.workspace_ui.extras.profile_user = None;
    }
}
pub fn presence_label(presence: Presence) -> &'static str {
    match presence {
        Presence::Online => "Disponível",
        Presence::Away => "Ausente",
        Presence::DoNotDisturb => "Não perturbar",
        Presence::Invisible => "Invisível",
    }
}
fn group(app: &mut LanvoxApp, ctx: &egui::Context) {
    if !app.workspace_ui.group_open {
        return;
    }
    let friends: Vec<_> = app
        .view
        .as_ref()
        .map(|v| {
            v.friends
                .iter()
                .filter(|f| f.status == FriendStatus::Friend)
                .map(|f| (f.user, v.user_name(f.user)))
                .collect()
        })
        .unwrap_or_default();
    let ready = !app.workspace_ui.group_members.is_empty()
        && !app.workspace_ui.group_name.trim().is_empty();
    let result = components::modal(
        ctx,
        "create-group",
        "Nova conversa em grupo",
        components::ModalOptions::new(440.0)
            .primary("Criar grupo")
            .enabled(ready)
            .secondary("Cancelar"),
        |ui| {
            components::form_row(ui, "Nome do grupo", |ui| {
                components::text_field(
                    ui,
                    &mut app.workspace_ui.group_name,
                    "Como o grupo vai se chamar?",
                    40,
                );
            });
            components::text_field(
                ui,
                &mut app.workspace_ui.extras.group_filter,
                "Buscar participantes",
                64,
            );
            ui.add_space(8.0);
            let filter = app.workspace_ui.extras.group_filter.to_lowercase();
            for (id, name) in friends
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&filter))
            {
                ui.horizontal(|ui| {
                    let texture = app.avatar_texture(ctx, *id);
                    let color = app
                        .view
                        .as_ref()
                        .map(|v| v.user_color(*id))
                        .unwrap_or(0x0e9488);
                    super::avatar::Avatar::new(name, super::avatar::AvatarSize::Sm)
                        .user(*id)
                        .color(color)
                        .texture(texture.as_ref())
                        .show(ui);
                    let mut selected = app.workspace_ui.group_members.contains(id);
                    if ui.checkbox(&mut selected, name).changed() {
                        if selected {
                            app.workspace_ui.group_members.insert(*id);
                        } else {
                            app.workspace_ui.group_members.remove(id);
                        }
                    }
                });
            }
            if friends.is_empty() {
                ui.label("Adicione amigos para criar um grupo.");
            }
            ui.label(format!(
                "{} pessoa(s) selecionada(s)",
                app.workspace_ui.group_members.len()
            ));
        },
    );
    if result.primary {
        app.open_conversation(
            app.workspace_ui.group_name.clone(),
            app.workspace_ui.group_members.iter().copied().collect(),
            false,
        );
        app.workspace_ui.group_name.clear();
        app.workspace_ui.group_members.clear();
        app.workspace_ui.extras.group_filter.clear();
    }
    if result.primary || result.cancelled() {
        app.workspace_ui.group_open = false;
    }
}
fn manage(app: &mut LanvoxApp, ctx: &egui::Context) {
    if !app.workspace_ui.managing {
        return;
    }
    let selected = app
        .selected
        .and_then(|id| app.view.as_ref()?.channel(id))
        .cloned();
    let conversation = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .conversations
                .iter()
                .find(|c| Some(c.channel) == app.selected)
        })
        .cloned();
    let guild = selected
        .as_ref()
        .filter(|c| c.guild > 0)
        .map(|c| c.guild)
        .or(app.guild);
    if app.workspace_ui.extras.admin_guild != guild {
        app.workspace_ui.extras.admin_guild = guild;
        app.workspace_ui.extras.admin_tab = 0;
        app.workspace_ui.extras.role = CommunityRole::default();
        app.workspace_ui.extras.invite_hours = 24;
        app.workspace_ui.extras.invite_uses = 10;
        if let Some(guild) = guild {
            app.workspace(WorkspaceCommand::Community(CommunityCommand::Invites {
                guild,
            }));
            app.workspace(WorkspaceCommand::Community(CommunityCommand::Moderation {
                guild,
            }));
        }
    }
    let result = components::modal(
        ctx,
        "community-settings",
        if conversation.is_some() {
            "Configurações da conversa"
        } else {
            "Configurações da comunidade"
        },
        components::ModalOptions::new(780.0).secondary("Concluir"),
        |ui| {
            if let Some(conversation) = &conversation {
                conversation_settings(app, ui, conversation);
                return;
            }
            let permissions = guild
                .and_then(|g| app.view.as_ref().map(|v| v.guild_permissions(g)))
                .unwrap_or(0);
            let tab_allowed = |tab: u8| match tab {
                1 => permissions & permission::MANAGE_CHANNELS != 0,
                2 => permissions & permission::MANAGE_ROLES != 0,
                3 => permissions & (permission::MODERATE | permission::MANAGE_ROLES) != 0,
                4 => permissions & permission::INVITE != 0,
                5 => permissions & permission::MODERATE != 0,
                7 => permissions & permission::MANAGE_GUILD != 0,
                _ => true,
            };
            if !tab_allowed(app.workspace_ui.extras.admin_tab) {
                app.workspace_ui.extras.admin_tab = 0;
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for (tab, label) in [
                    (0, "Visão geral"),
                    (1, "Canais"),
                    (2, "Cargos"),
                    (3, "Membros"),
                    (4, "Convites"),
                    (5, "Moderação"),
                    (6, "Notificações"),
                    (7, "Expressões"),
                    (8, "Eventos"),
                ] {
                    if tab_allowed(tab) {
                        ui.selectable_value(&mut app.workspace_ui.extras.admin_tab, tab, label);
                    }
                }
            });
            ui.separator();
            let Some(guild) = guild else {
                ui.label("Entre numa comunidade ou cole um convite para começar.");
                invite_entry(app, ui);
                return;
            };
            let admin = app
                .view
                .as_ref()
                .is_some_and(|v| v.guild_permissions(guild) & permission::MANAGE_GUILD != 0);
            match app.workspace_ui.extras.admin_tab {
                0 => {
                    let name = app
                        .view
                        .as_ref()
                        .and_then(|v| v.guilds.iter().find(|g| g.id == guild))
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    ui.heading(&name);
                    if admin {
                        components::text_field(
                            ui,
                            &mut app.workspace_ui.guild_name,
                            "Novo nome da comunidade",
                            40,
                        );
                        if ui.button("Salvar nome").clicked() {
                            app.workspace(WorkspaceCommand::UpdateGuild {
                                guild,
                                name: app.workspace_ui.guild_name.clone(),
                            });
                        }
                        super::community_ui::visuals(app, ui, guild);
                    }
                    ui.add_space(12.0);
                    invite_entry(app, ui);
                    ui.add_space(12.0);
                    ui.collapsing("Administração do host", |ui| {
                        if app
                            .view
                            .as_ref()
                            .is_some_and(|v| v.workspace.host_owner == Some(v.me))
                        {
                            let people: Vec<_> = app
                                .view
                                .as_ref()
                                .map(|v| {
                                    v.users
                                        .values()
                                        .filter(|u| u.id != v.me)
                                        .map(|u| {
                                            (
                                                u.id,
                                                u.name.clone(),
                                                v.workspace.disabled_users.contains(&u.id),
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            for (user, name, disabled) in people {
                                ui.horizontal(|ui| {
                                    ui.label(name);
                                    if ui
                                        .button(if disabled {
                                            "Reabilitar acesso"
                                        } else {
                                            "Bloquear acesso"
                                        })
                                        .clicked()
                                    {
                                        app.workspace_ui.host_access_confirmation =
                                            Some((user, disabled));
                                    }
                                });
                            }
                            if ui.button("Criar cópia do banco de dados").clicked() {
                                app.workspace(WorkspaceCommand::Backup);
                            }
                        } else {
                            ui.label("Somente o proprietário do host pode administrar seu acesso.");
                        }
                    });
                    ui.collapsing("Sair ou excluir", |ui| {
                        if components::danger_button(ui, "Sair desta comunidade").clicked() {
                            app.workspace(WorkspaceCommand::LeaveGuild { guild });
                            app.workspace_ui.managing = false;
                        }
                        if admin {
                            ui.checkbox(
                                &mut app.workspace_ui.delete_guild_confirm,
                                "Confirmo a exclusão permanente da comunidade e dos canais",
                            );
                            if ui
                                .add_enabled_ui(app.workspace_ui.delete_guild_confirm, |ui| {
                                    components::danger_button(ui, "Excluir comunidade")
                                })
                                .inner
                                .clicked()
                            {
                                app.workspace(WorkspaceCommand::DeleteGuild { guild });
                                app.workspace_ui.managing = false;
                            }
                        }
                    });
                }
                1 => {
                    let available: Vec<Channel> = app
                        .view
                        .as_ref()
                        .map(|v| {
                            v.channels
                                .iter()
                                .filter(|c| c.guild == guild)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    let selection_key = egui::Id::unique(("admin-channel-selection", guild));
                    let mut chosen = ui
                        .ctx()
                        .data(|d| d.get_temp::<ChannelId>(selection_key))
                        .filter(|id| available.iter().any(|c| c.id == *id))
                        .or_else(|| selected.as_ref().filter(|c| c.guild == guild).map(|c| c.id))
                        .or_else(|| available.first().map(|c| c.id));
                    egui::ComboBox::from_id_salt(("admin-channel-picker", guild))
                        .selected_text(
                            chosen
                                .and_then(|id| available.iter().find(|c| c.id == id))
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| "Nenhum canal disponível".into()),
                        )
                        .show_ui(ui, |ui| {
                            for channel in &available {
                                ui.selectable_value(&mut chosen, Some(channel.id), &channel.name);
                            }
                        });
                    if let Some(id) = chosen {
                        ui.ctx().data_mut(|d| d.insert_temp(selection_key, id));
                    }
                    if let Some(channel) =
                        chosen.and_then(|id| available.iter().find(|c| c.id == id))
                    {
                        if app
                            .view
                            .as_ref()
                            .is_some_and(|v| v.can_manage_channels(guild))
                        {
                            channel_settings(app, ui, channel);
                            super::community_ui::channel_policy(app, ui, channel);
                        } else {
                            ui.label("Somente administradores podem alterar o canal.");
                        }
                    } else {
                        ui.label("Selecione um canal para configurá-lo.");
                    }
                }
                2 => super::community_ui::roles(app, ui, guild),
                3 => super::community_ui::members(app, ui, guild),
                4 => super::community_ui::invites(app, ui, guild),
                5 => super::community_ui::moderation(app, ui, guild),
                6 => super::community_ui::notifications(app, ui, guild),
                7 => super::community_ui::emojis(app, ui, guild),
                _ => super::community_ui::events(app, ui, guild),
            }
        },
    );
    if result.cancelled() {
        app.workspace_ui.managing = false;
    }
}
fn invite_entry(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    ui.collapsing("Entrar em outra comunidade", |ui| {
        ui.horizontal_wrapped(|ui| {
            components::text_field(
                ui,
                &mut app.workspace_ui.invite_input,
                "Código da comunidade",
                128,
            );
            if ui.button("Entrar").clicked() {
                app.workspace(WorkspaceCommand::RedeemInvite {
                    code: app.workspace_ui.invite_input.trim().to_owned(),
                });
            }
        });
    });
}
fn channel_settings(app: &mut LanvoxApp, ui: &mut egui::Ui, channel: &Channel) {
    if app.workspace_ui.editing_channel != Some(channel.id) {
        app.workspace_ui.editing_channel = Some(channel.id);
        app.workspace_ui.channel_name = channel.name.clone();
        app.workspace_ui.channel_topic = channel.topic.clone();
        let settings = app
            .view
            .as_ref()
            .and_then(|v| {
                v.workspace
                    .channel_settings
                    .iter()
                    .find(|c| c.channel == channel.id)
            })
            .cloned()
            .unwrap_or_default();
        let current = settings.category_id.filter(|id| {
            app.view.as_ref().is_some_and(|v| {
                v.workspace
                    .categories
                    .iter()
                    .any(|c| c.id == *id && c.guild == channel.guild)
            })
        });
        app.workspace_ui.channel_category = current;
        app.workspace_ui.channel_category_before = current;
        app.workspace_ui.channel_allowed = settings.allowed.into_iter().collect();
        app.workspace_ui.delete_channel_confirm = false;
    }
    ui.separator();
    ui.heading("Canal selecionado");
    ui.label("Nome");
    ui.text_edit_singleline(&mut app.workspace_ui.channel_name);
    ui.label("Assunto");
    ui.text_edit_singleline(&mut app.workspace_ui.channel_topic);
    let categories = app
        .view
        .as_ref()
        .map(|v| crate::categories::sorted_categories(channel.guild, &v.workspace.categories))
        .unwrap_or_default();
    let selected_text = categories
        .iter()
        .find(|c| Some(c.id) == app.workspace_ui.channel_category)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Sem categoria".to_string());
    components::form_row_inline(ui, "Categoria", |ui| {
        egui::ComboBox::from_id_salt("channel-category")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut app.workspace_ui.channel_category,
                    None,
                    "Sem categoria",
                );
                for c in &categories {
                    ui.selectable_value(
                        &mut app.workspace_ui.channel_category,
                        Some(c.id),
                        &c.name,
                    );
                }
            });
    });
    ui.label(
        egui::RichText::new("Para reordenar, arraste o canal na barra lateral.")
            .small()
            .color(super::theme::color(theme::TEXT_FAINT)),
    );
    ui.collapsing("Restringir canal a pessoas específicas", |ui| {
        ui.label("Nenhuma pessoa marcada: todos os membros. Administradores sempre têm acesso.");
        let people: Vec<_> = app
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
        for (id, name) in people {
            let mut allowed = app.workspace_ui.channel_allowed.contains(&id);
            if ui.checkbox(&mut allowed, name).changed() {
                if allowed {
                    app.workspace_ui.channel_allowed.insert(id);
                } else {
                    app.workspace_ui.channel_allowed.remove(&id);
                }
            }
        }
    });
    if components::primary_button(ui, "Salvar canal").clicked() {
        app.workspace(WorkspaceCommand::UpdateChannel {
            channel: channel.id,
            name: app.workspace_ui.channel_name.clone(),
            topic: app.workspace_ui.channel_topic.clone(),
            // Placement is handled by MoveChannel below; these legacy fields are ignored.
            category: String::new(),
            position: 0,
            allowed: app.workspace_ui.channel_allowed.iter().copied().collect(),
        });
        if app.workspace_ui.channel_category != app.workspace_ui.channel_category_before {
            app.category_command(WorkspaceCommand::MoveChannel {
                channel: channel.id,
                category: app.workspace_ui.channel_category,
                index: u32::MAX,
            });
            app.workspace_ui.channel_category_before = app.workspace_ui.channel_category;
        }
    }
    ui.checkbox(
        &mut app.workspace_ui.delete_channel_confirm,
        "Confirmo a exclusão deste canal e histórico",
    );
    if ui
        .add_enabled_ui(app.workspace_ui.delete_channel_confirm, |ui| {
            components::danger_button(ui, "Excluir canal")
        })
        .inner
        .clicked()
    {
        app.workspace(WorkspaceCommand::DeleteChannel {
            channel: channel.id,
        });
        app.workspace_ui.managing = false;
    }
}
fn conversation_settings(app: &mut LanvoxApp, ui: &mut egui::Ui, conversation: &Conversation) {
    ui.separator();
    ui.heading(if conversation.direct {
        "Conversa direta"
    } else {
        "Grupo privado"
    });
    ui.label(&conversation.name);
    if app.workspace_ui.editing_channel != Some(conversation.channel) {
        app.workspace_ui.editing_channel = Some(conversation.channel);
        app.workspace_ui.channel_name = conversation.name.clone();
        app.workspace_ui.group_members = conversation.members.iter().copied().collect();
    }
    if !conversation.direct && conversation.owner == app.me() {
        super::community_ui::conversation_icon(app, ui, conversation.channel);
        ui.label("Nome");
        ui.text_edit_singleline(&mut app.workspace_ui.channel_name);
        let people: Vec<_> = app
            .view
            .as_ref()
            .map(|v| v.users.values().map(|u| (u.id, u.name.clone())).collect())
            .unwrap_or_default();
        for (id, display) in people {
            let mut selected = app.workspace_ui.group_members.contains(&id);
            if ui
                .add_enabled(id != app.me(), egui::Checkbox::new(&mut selected, display))
                .changed()
            {
                if selected {
                    app.workspace_ui.group_members.insert(id);
                } else {
                    app.workspace_ui.group_members.remove(&id);
                }
            }
        }
        if components::primary_button(ui, "Salvar grupo").clicked() {
            app.workspace(WorkspaceCommand::UpdateConversation {
                channel: conversation.channel,
                name: app.workspace_ui.channel_name.clone(),
                members: app.workspace_ui.group_members.iter().copied().collect(),
            });
        }
    } else {
        for id in &conversation.members {
            let name = app
                .view
                .as_ref()
                .map(|v| v.user_name(*id))
                .unwrap_or_default();
            if ui.link(name).clicked() {
                app.open_profile(*id);
            }
        }
    }
    if components::danger_button(ui, "Sair da conversa").clicked() {
        app.workspace(WorkspaceCommand::LeaveConversation {
            channel: conversation.channel,
        });
        app.workspace_ui.managing = false;
    }
}
fn host_access_confirmation(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some((user, allowed)) = app.workspace_ui.host_access_confirmation else {
        return;
    };
    let name = app
        .view
        .as_ref()
        .map(|view| view.user_name(user))
        .unwrap_or_default();
    let title = if allowed {
        format!("Reabilitar {name}?")
    } else {
        format!("Bloquear {name} no host?")
    };
    let result = components::modal(
        ctx,
        "host-access-confirmation",
        &title,
        components::ModalOptions::new(380.0)
            .primary(if allowed {
                "Reabilitar acesso"
            } else {
                "Bloquear acesso"
            })
            .secondary("Cancelar"),
        |ui| {
            ui.label(if allowed {
                "A identidade poderá se conectar novamente. As permissões das comunidades continuam sendo verificadas."
            } else {
                "A sessão atual será encerrada e esta identidade não poderá entrar novamente até ser reabilitada. O histórico será preservado."
            });
        },
    );
    let confirm = result.primary;
    if confirm {
        app.workspace(WorkspaceCommand::SetHostAccess { user, allowed });
    }
    if confirm || result.cancelled() {
        app.workspace_ui.host_access_confirmation = None;
    }
}
