use super::*;

pub fn visuals(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    ui.label("Imagens da comunidade");
    let mut selection = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("Escolher ícone").clicked() {
            selection = Some(false);
        }
        if ui.button("Escolher banner").clicked() {
            selection = Some(true);
        }
        if ui.button("Remover imagens").clicked() {
            command(
                app,
                CommunityCommand::UpdateVisual {
                    guild,
                    icon: Vec::new(),
                    banner: Vec::new(),
                },
            );
        }
    });
    if let Some(banner) = selection {
        let (tx, rx) = crossbeam_channel::bounded(1);
        app.workspace_ui.extras.visual_pending = Some(rx);
        std::thread::spawn(move || {
            let result = (|| {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Imagens", &["png", "jpg", "jpeg", "webp"])
                    .pick_file()
                else {
                    return Ok(None);
                };
                let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                if metadata.len() > 8 * 1024 * 1024 {
                    return Err("Escolha uma imagem de até 8 MiB.".into());
                }
                let image = image::open(path).map_err(|e| e.to_string())?;
                let image = if banner {
                    image.thumbnail(1000, 375)
                } else {
                    image.thumbnail(128, 128)
                };
                let mut out = std::io::Cursor::new(Vec::new());
                image
                    .write_to(&mut out, image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
                if out.get_ref().len() > if banner { 512 * 1024 } else { 256 * 1024 } {
                    return Err(
                        "A imagem convertida ainda é muito grande. Escolha outra imagem.".into(),
                    );
                }
                Ok(Some((guild, banner, out.into_inner())))
            })();
            let _ = tx.send(result);
        });
    }
    if app.workspace_ui.extras.visual_pending.is_some() {
        ui.spinner();
    }
}

pub fn poll_visual(app: &mut LanvoxApp) {
    let conversation_result = app
        .workspace_ui
        .extras
        .conversation_image_pending
        .as_ref()
        .and_then(|r| r.try_recv().ok());
    if let Some(result) = conversation_result {
        app.workspace_ui.extras.conversation_image_pending = None;
        match result {
            Ok(Some((channel, icon))) => command(
                app,
                CommunityCommand::UpdateConversationIcon { channel, icon },
            ),
            Ok(None) => {}
            Err(error) => app.notify(error),
        }
    }
    let emoji_result = app
        .workspace_ui
        .extras
        .emoji_pending
        .as_ref()
        .and_then(|r| r.try_recv().ok());
    if let Some(result) = emoji_result {
        app.workspace_ui.extras.emoji_pending = None;
        match result {
            Ok(Some((guild, name, image))) => command(
                app,
                CommunityCommand::UpsertEmoji {
                    guild,
                    id: None,
                    name,
                    image,
                },
            ),
            Ok(None) => {}
            Err(error) => app.notify(error),
        }
    }
    let result = app
        .workspace_ui
        .extras
        .visual_pending
        .as_ref()
        .and_then(|r| r.try_recv().ok());
    let Some(result) = result else {
        return;
    };
    app.workspace_ui.extras.visual_pending = None;
    match result {
        Ok(Some((guild, banner, bytes))) => {
            let visual = app
                .view
                .as_ref()
                .and_then(|v| {
                    v.workspace
                        .community
                        .visuals
                        .iter()
                        .find(|v| v.guild == guild)
                })
                .cloned()
                .unwrap_or_default();
            command(
                app,
                CommunityCommand::UpdateVisual {
                    guild,
                    icon: if banner { visual.icon } else { bytes.clone() },
                    banner: if banner { bytes } else { visual.banner },
                },
            );
        }
        Ok(None) => {}
        Err(error) => app.notify(error),
    }
}

pub fn conversation_icon(app: &mut LanvoxApp, ui: &mut Ui, channel: ChannelId) {
    let bytes = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .community
                .conversation_visuals
                .iter()
                .find(|v| v.channel == channel)
        })
        .map(|v| v.icon.clone())
        .unwrap_or_default();
    if let Some(texture) = image_texture(ui.ctx(), &format!("group-icon-{channel}"), &bytes) {
        ui.add(egui::Image::new((texture.id(), egui::vec2(64.0, 64.0))).corner_radius(32));
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Escolher imagem do grupo").clicked() {
            let (tx, rx) = crossbeam_channel::bounded(1);
            app.workspace_ui.extras.conversation_image_pending = Some(rx);
            std::thread::spawn(move || {
                let result = (|| {
                    let Some(path) = rfd::FileDialog::new()
                        .add_filter("Imagens", &["png", "jpg", "jpeg"])
                        .pick_file()
                    else {
                        return Ok(None);
                    };
                    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024
                    {
                        return Err("Escolha uma imagem de até 8 MiB.".into());
                    }
                    let image = image::open(path)
                        .map_err(|e| e.to_string())?
                        .thumbnail(128, 128);
                    let mut out = std::io::Cursor::new(Vec::new());
                    image
                        .write_to(&mut out, image::ImageFormat::Png)
                        .map_err(|e| e.to_string())?;
                    if out.get_ref().len() > 128 * 1024 {
                        return Err("A imagem ultrapassa 128 KiB após conversão.".into());
                    }
                    Ok(Some((channel, out.into_inner())))
                })();
                let _ = tx.send(result);
            });
        }
        if !bytes.is_empty() && ui.button("Remover imagem").clicked() {
            command(
                app,
                CommunityCommand::UpdateConversationIcon {
                    channel,
                    icon: Vec::new(),
                },
            );
        }
    });
    if app.workspace_ui.extras.conversation_image_pending.is_some() {
        ui.spinner();
    }
}

pub fn banner(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    let bytes = app
        .view
        .as_ref()
        .and_then(|v| {
            v.workspace
                .community
                .visuals
                .iter()
                .find(|v| v.guild == guild)
        })
        .map(|v| v.banner.clone())
        .unwrap_or_default();
    if let Some(texture) = image_texture(ui.ctx(), &format!("community-banner-{guild}"), &bytes) {
        ui.add(
            egui::Image::new((texture.id(), egui::vec2(ui.available_width(), 72.0)))
                .corner_radius(6),
        );
        ui.add_space(10.0);
    }
}

pub fn emojis(app: &mut LanvoxApp, ui: &mut Ui, guild: GuildId) {
    ui.label("Adicione expressões que fazem parte da sua comunidade.");
    components::text_field(
        ui,
        &mut app.workspace_ui.extras.emoji_name,
        "Nome do emoji (letras, números e _)",
        32,
    );
    if ui
        .add_enabled(
            !app.workspace_ui.extras.emoji_name.trim().is_empty()
                && app.workspace_ui.extras.emoji_pending.is_none(),
            egui::Button::new("Escolher imagem e adicionar"),
        )
        .clicked()
    {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let name = app.workspace_ui.extras.emoji_name.trim().to_owned();
        app.workspace_ui.extras.emoji_pending = Some(rx);
        std::thread::spawn(move || {
            let result = (|| {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("Imagens", &["png", "jpg", "jpeg"])
                    .pick_file()
                else {
                    return Ok(None);
                };
                if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
                    return Err("Escolha uma imagem de até 8 MiB.".into());
                }
                let image = image::open(path)
                    .map_err(|e| e.to_string())?
                    .thumbnail(128, 128);
                let mut out = std::io::Cursor::new(Vec::new());
                image
                    .write_to(&mut out, image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
                if out.get_ref().len() > 256 * 1024 {
                    return Err("A imagem ultrapassa 256 KiB após conversão.".into());
                }
                Ok(Some((guild, name, out.into_inner())))
            })();
            let _ = tx.send(result);
        });
    }
    if app.workspace_ui.extras.emoji_pending.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Preparando expressão…");
        });
    }
    let emojis = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .community
                .emojis
                .iter()
                .filter(|e| e.guild == guild)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ui.add_space(12.0);
    if emojis.is_empty() {
        ui.label("A comunidade ainda não tem emojis próprios.");
    }
    for emoji in emojis {
        ui.horizontal(|ui| {
            if let Some(texture) = image_texture(
                ui.ctx(),
                &format!("custom-emoji-{}", emoji.id),
                &emoji.image,
            ) {
                ui.add(egui::Image::new((texture.id(), egui::vec2(32.0, 32.0))));
            }
            ui.label(format!(":{}:", emoji.name));
            if ui.small_button("Copiar código").clicked() {
                ui.ctx()
                    .copy_text(format!("<:{}:{}>", emoji.name, emoji.id));
            }
            if ui.small_button("Excluir").clicked() {
                command(
                    app,
                    CommunityCommand::DeleteEmoji {
                        guild,
                        id: emoji.id,
                    },
                );
            }
        });
    }
}

/// Cached decoded community imagery. The key includes its content so edits replace it.
pub fn image_texture(
    ctx: &egui::Context,
    scope: &str,
    bytes: &[u8],
) -> Option<egui::TextureHandle> {
    use std::hash::{Hash, Hasher};
    if bytes.is_empty() {
        return None;
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    let id = egui::Id::unique((scope, hash.finish()));
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let texture = ctx.load_texture(
        scope,
        egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}
