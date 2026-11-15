use super::*;

pub(super) fn emoji_picker(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    channel: ChannelId,
    reaction: bool,
) -> Option<String> {
    ui.set_width(300.0);
    ui.add(
        egui::TextEdit::singleline(&mut app.workspace_ui.emoji_query)
            .hint_text("Buscar emoji")
            .desired_width(f32::INFINITY),
    );
    let query = app.workspace_ui.emoji_query.to_lowercase();
    let mut picked = None;
    let guild = app
        .view
        .as_ref()
        .and_then(|v| v.channel(channel))
        .map(|c| c.guild);
    let custom: Vec<_> = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .emojis
                .iter()
                .filter(|e| {
                    Some(e.guild) == guild
                        && (query.is_empty() || e.name.to_lowercase().contains(&query))
                })
                .map(|e| (e.id, e.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    if !custom.is_empty() {
        ui.label(
            RichText::new("Da comunidade")
                .small()
                .color(theme::TEXT_MUTED),
        );
        ui.horizontal_wrapped(|ui| {
            for (id, name) in custom {
                if let Some(texture) = custom_emoji_texture(app, ui.ctx(), id) {
                    if ui
                        .add(egui::Button::image(egui::Image::new((
                            texture.id(),
                            vec2(26.0, 26.0),
                        ))))
                        .on_hover_text(format!(":{name}:"))
                        .clicked()
                    {
                        picked = Some(if reaction {
                            format!("custom:{id}")
                        } else {
                            format!("<:{name}:{id}>")
                        });
                    }
                }
            }
        });
        ui.separator();
    }
    if query.is_empty() && !app.settings.emoji_recents.is_empty() {
        ui.label(RichText::new("Recentes").small().color(theme::TEXT_MUTED));
        ui.horizontal_wrapped(|ui| {
            for emoji in app
                .settings
                .emoji_recents
                .clone()
                .iter()
                .filter(|e| !e.starts_with("custom:") && !e.starts_with("<:"))
                .take(16)
            {
                if ui.button(RichText::new(emoji).size(20.0)).clicked() {
                    picked = Some(emoji.clone());
                }
            }
        });
        ui.separator();
    }
    egui::ScrollArea::vertical()
        .id_salt("emoji-list")
        .max_height(220.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (emoji, name) in EMOJI
                    .iter()
                    .filter(|(e, n)| query.is_empty() || n.contains(&query) || e.contains(&query))
                {
                    if ui
                        .add_sized(
                            [32.0, 32.0],
                            egui::Button::new(RichText::new(*emoji).size(21.0)),
                        )
                        .on_hover_text(*name)
                        .clicked()
                    {
                        picked = Some((*emoji).to_owned());
                    }
                }
            });
        });
    if let Some(emoji) = &picked {
        app.settings.emoji_recents.retain(|e| e != emoji);
        app.settings.emoji_recents.insert(0, emoji.clone());
        app.settings.emoji_recents.truncate(24);
        app.settings.save();
        app.workspace_ui.emoji_query.clear();
    }
    picked
}
