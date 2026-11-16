use super::*;

pub(super) fn poll_row(app: &mut LanvoxApp, ui: &mut egui::Ui, message: MessageId) {
    let Some(poll) = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .community
                .polls
                .iter()
                .find(|p| p.message == message)
        })
        .cloned()
    else {
        return;
    };
    let mine = poll
        .votes
        .iter()
        .find(|v| v.user == app.me())
        .map(|v| v.choices.clone())
        .unwrap_or_default();
    let mut choices = app
        .workspace_ui
        .poll_choices
        .get(&message)
        .cloned()
        .unwrap_or_else(|| mine.clone());
    let mut vote = false;
    let mut remove = false;
    let mut close = false;
    Frame::new()
        .fill(theme::color(theme::SIDEBAR))
        .corner_radius(6)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.label(RichText::new(&poll.question).strong().size(17.0));
            ui.label(
                RichText::new(if poll.closed {
                    "Enquete encerrada"
                } else if poll.multiple {
                    "Escolha uma ou mais opções"
                } else {
                    "Escolha uma opção"
                })
                .small()
                .color(theme::TEXT_MUTED),
            );
            for (index, option) in poll.options.iter().enumerate() {
                let index = index as u8;
                let count = poll
                    .votes
                    .iter()
                    .filter(|v| v.choices.contains(&index))
                    .count();
                let label = format!("{option}  ·  {count}");
                if poll.closed {
                    ui.label(label);
                } else if poll.multiple {
                    let mut checked = choices.contains(&index);
                    if ui.checkbox(&mut checked, label).changed() {
                        if checked {
                            choices.push(index);
                        } else {
                            choices.retain(|c| *c != index);
                        }
                    }
                } else if ui.radio(choices.contains(&index), label).clicked() {
                    choices = vec![index];
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(if poll.votes.len() == 1 {
                        "1 participante".into()
                    } else {
                        format!("{} participantes", poll.votes.len())
                    })
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                if !poll.closed {
                    vote = ui
                        .add_enabled(
                            !choices.is_empty(),
                            egui::Button::new(if mine.is_empty() {
                                "Votar"
                            } else {
                                "Atualizar voto"
                            }),
                        )
                        .clicked();
                    if !mine.is_empty() {
                        remove = ui.small_button("Retirar voto").clicked();
                    }
                    let manager = poll.creator == app.me()
                        || app.view.as_ref().is_some_and(|v| {
                            v.has_permission(poll.channel, permission::MANAGE_MESSAGES)
                        });
                    if manager {
                        close = ui.small_button("Encerrar enquete").clicked();
                    }
                }
            });
        });
    choices.sort_unstable();
    choices.dedup();
    app.workspace_ui
        .poll_choices
        .insert(message, choices.clone());
    if vote || remove {
        app.workspace(WorkspaceCommand::Community(CommunityCommand::VotePoll {
            message,
            choices: if remove { Vec::new() } else { choices },
        }));
        if remove {
            app.workspace_ui.poll_choices.remove(&message);
        }
    }
    if close {
        app.workspace(WorkspaceCommand::Community(CommunityCommand::ClosePoll {
            message,
        }));
    }
}

pub(super) fn create_poll(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some(channel) = app.workspace_ui.poll_channel else {
        return;
    };
    let options: Vec<String> = app
        .workspace_ui
        .poll_options
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let unique: std::collections::HashSet<_> = options.iter().map(|s| s.to_lowercase()).collect();
    let valid = !app.workspace_ui.poll_question.trim().is_empty()
        && (2..=10).contains(&options.len())
        && unique.len() == options.len()
        && options.iter().all(|s| s.chars().count() <= 100);
    let result = crate::ui::components::modal(
        ctx,
        "create-poll",
        "Criar enquete",
        crate::ui::components::ModalOptions::new(480.0)
            .primary("Publicar enquete")
            .enabled(valid)
            .secondary("Cancelar"),
        |ui| {
            ui.label("Pergunta");
            ui.add(
                egui::TextEdit::singleline(&mut app.workspace_ui.poll_question)
                    .char_limit(200)
                    .hint_text("O que vamos decidir?")
                    .desired_width(f32::INFINITY),
            );
            ui.label("Opções · uma por linha");
            ui.add(
                egui::TextEdit::multiline(&mut app.workspace_ui.poll_options)
                    .desired_rows(5)
                    .char_limit(1100)
                    .desired_width(f32::INFINITY),
            );
            ui.label(
                RichText::new("De 2 a 10 opções diferentes, com até 100 caracteres cada.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.checkbox(
                &mut app.workspace_ui.poll_multiple,
                "Permitir várias escolhas",
            );
        },
    );
    if result.primary {
        app.workspace(WorkspaceCommand::Community(CommunityCommand::CreatePoll {
            operation: format!("poll-{:032x}", rand::random::<u128>()),
            channel,
            question: app.workspace_ui.poll_question.trim().into(),
            options,
            multiple: app.workspace_ui.poll_multiple,
        }));
        app.workspace_ui.poll_channel = None;
    } else if result.cancelled() {
        app.workspace_ui.poll_channel = None;
    }
}
