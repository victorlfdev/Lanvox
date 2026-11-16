//! Membership-aware, keyboard accessible member and profile list.
use super::avatar::{Avatar, AvatarSize, PresenceState};
use super::{theme, widgets};
use crate::{app::LanvoxApp, model::*};
use egui::RichText;
pub fn show(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let Some(view) = &app.view else {
        return;
    };
    let conversation = app.selected.and_then(|id| {
        view.workspace
            .conversations
            .iter()
            .find(|c| c.channel == id)
    });
    if let Some(user) = conversation
        .filter(|c| c.direct)
        .and_then(|c| c.members.iter().find(|id| **id != view.me))
        .copied()
    {
        contact_profile(app, ui, user);
        return;
    }
    let authorized: Vec<_> = if let Some(conversation) = conversation {
        conversation.members.clone()
    } else {
        view.workspace
            .memberships
            .iter()
            .filter(|m| Some(m.guild) == app.guild)
            .map(|m| m.user)
            .collect()
    };
    let mut people: Vec<_> = view
        .users
        .values()
        .filter(|u| authorized.is_empty() || authorized.contains(&u.id))
        .cloned()
        .collect();
    people.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let is_online =
        |app: &LanvoxApp, person: &User| PresenceState::of_user(app, person.id).is_present();
    let online_count = people.iter().filter(|p| is_online(app, p)).count();
    let total = people.len();
    let mut last_group: Option<bool> = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(16.0);
        for person in people {
            let group = is_online(app, &person);
            if last_group != Some(group) {
                last_group = Some(group);
                ui.add_space(if group { 0.0 } else { 14.0 });
                let count = if group {
                    online_count
                } else {
                    total.saturating_sub(online_count)
                };
                widgets::section_label(
                    ui,
                    &format!("{} — {}", if group { "Online" } else { "Offline" }, count),
                );
                ui.add_space(4.0);
            }
            let profile = app
                .view
                .as_ref()
                .and_then(|v| v.workspace.profiles.iter().find(|p| p.user == person.id))
                .cloned()
                .unwrap_or_default();
            let extra = app
                .view
                .as_ref()
                .and_then(|v| {
                    v.workspace
                        .community
                        .profile_extras
                        .iter()
                        .find(|p| p.user == person.id && p.guild == app.guild)
                })
                .cloned()
                .unwrap_or_default();
            let shown_name = if extra.nickname.is_empty() {
                person.name.clone()
            } else {
                extra.nickname
            };
            let roles = app
                .view
                .as_ref()
                .map(|v| {
                    v.workspace
                        .community
                        .roles
                        .iter()
                        .filter(|r| {
                            Some(r.guild) == app.guild
                                && v.workspace.community.assignments.iter().any(|a| {
                                    a.user == person.id
                                        && a.guild == r.guild
                                        && a.roles.contains(&r.id)
                                })
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let role_color = roles
                .iter()
                .max_by_key(|r| r.position)
                .map(|r| theme::readable_text(color32(r.color), theme::color(theme::SIDEBAR)));
            let state = PresenceState::of_user(app, person.id);
            let online = state.is_present();
            ui.horizontal(|ui| {
                let texture = app.avatar_texture(ui.ctx(), person.id);
                let avatar_resp = Avatar::new(&person.name, AvatarSize::Sm)
                    .user(person.id)
                    .color(person.color)
                    .texture(texture.as_ref())
                    .presence(state)
                    .show(ui);
                if avatar_resp.clicked() {
                    app.open_profile(person.id);
                }
                super::call::person_audio_menu(app, &avatar_resp, person.id, &person.name);
                ui.vertical(|ui| {
                    let name_resp = ui.link(RichText::new(&shown_name).color(if online {
                        role_color.unwrap_or(theme::color(theme::TEXT))
                    } else {
                        theme::color(theme::TEXT_MUTED)
                    }));
                    if name_resp.clicked() {
                        app.open_profile(person.id);
                    }
                    let name_resp = name_resp.on_hover_text("Botão direito para ajustar volume");
                    super::call::person_audio_menu(app, &name_resp, person.id, &person.name);
                    if let Some(role) = roles.iter().max_by_key(|r| r.position) {
                        ui.label(
                            RichText::new(&role.name)
                                .size(10.0)
                                .color(color32(role.color)),
                        );
                    }
                    let status = if !online {
                        state.label().to_owned()
                    } else if !profile.status.is_empty() {
                        profile.status.clone()
                    } else {
                        state.label().to_owned()
                    };
                    ui.add(
                        egui::Label::new(
                            RichText::new(status)
                                .small()
                                .color(theme::color(theme::TEXT_MUTED)),
                        )
                        .truncate(),
                    );
                });
            });
            ui.add_space(4.0);
        }
    });
}

fn contact_profile(app: &mut LanvoxApp, ui: &mut egui::Ui, user: UserId) {
    let person = app.view.as_ref().and_then(|v| v.users.get(&user)).cloned();
    let Some(person) = person else {
        return;
    };
    let profile = app
        .view
        .as_ref()
        .and_then(|v| v.workspace.profiles.iter().find(|p| p.user == user))
        .cloned()
        .unwrap_or_default();
    let bio = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .community
                .profile_extras
                .iter()
                .find(|p| p.user == user && p.guild.is_none())
        })
        .map(|p| p.bio.clone())
        .unwrap_or_default();
    let texture = app.avatar_texture(ui.ctx(), user);
    let state = PresenceState::of_user(app, user);
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            let avatar = Avatar::new(&person.name, AvatarSize::Xl)
                .user(user)
                .color(person.color)
                .texture(texture.as_ref())
                .presence(state)
                .show(ui);
            if avatar.clicked() {
                app.open_profile(user);
            }
            ui.add_space(10.0);
            ui.label(RichText::new(&person.name).strong().size(20.0));
            ui.label(RichText::new(state.label()).color(theme::color(theme::TEXT_MUTED)));
        });
        ui.add_space(20.0);
        if !profile.status.is_empty() {
            ui.label(&profile.status);
            ui.add_space(12.0);
        }
        if !bio.is_empty() {
            ui.label(RichText::new("Sobre mim").strong());
            ui.label(bio);
            ui.add_space(16.0);
        }
        let mutual = app
            .view
            .as_ref()
            .map(|v| {
                v.guilds
                    .iter()
                    .filter(|g| {
                        v.workspace
                            .memberships
                            .iter()
                            .any(|m| m.guild == g.id && m.user == user)
                    })
                    .count()
            })
            .unwrap_or(0);
        ui.label(
            RichText::new(match mutual {
                0 => "Nenhuma comunidade em comum".to_owned(),
                1 => "1 comunidade em comum".to_owned(),
                _ => format!("{mutual} comunidades em comum"),
            })
            .small()
            .color(theme::color(theme::TEXT_MUTED)),
        );
        ui.add_space(16.0);
        if ui.button("Ver perfil").clicked() {
            app.open_profile(user);
        }
        if ui.button("Iniciar chamada").clicked() {
            app.start_call(vec![user]);
        }
    });
}
