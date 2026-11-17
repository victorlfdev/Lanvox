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
                // Keep the editor responsive on very large photos.
                let image = if image.width().max(image.height()) > 1600 {
                    image.thumbnail(1600, 1600)
                } else {
                    image
                };
                Ok(Some((guild, banner, image)))
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
        Ok(Some((guild, banner, image))) => {
            let target = if banner {
                VisualTarget::Banner(guild)
            } else {
                VisualTarget::Icon(guild)
            };
            app.workspace_ui.extras.visual_editor = Some(VisualEditor::new(target, image));
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
        // Crop to fill the strip instead of stretching the image.
        let size = egui::vec2(ui.available_width(), 72.0);
        let tex = texture.size_vec2();
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let frame = size.x / size.y;
        let uv = if tex.x / tex.y > frame {
            let w = frame * tex.y / tex.x;
            egui::Rect::from_min_max(
                egui::pos2((1.0 - w) / 2.0, 0.0),
                egui::pos2((1.0 + w) / 2.0, 1.0),
            )
        } else {
            let h = tex.x / frame / tex.y;
            egui::Rect::from_min_max(
                egui::pos2(0.0, (1.0 - h) / 2.0),
                egui::pos2(1.0, (1.0 + h) / 2.0),
            )
        };
        ui.painter().add(
            egui::epaint::RectShape::filled(rect, 6u8, egui::Color32::WHITE)
                .with_texture(texture.id(), uv),
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

/// Crop/position editor shown after the person picks a community image.
#[derive(Clone, Copy)]
pub enum VisualTarget {
    Banner(GuildId),
    Icon(GuildId),
    /// The signed-in user's own profile picture.
    Avatar,
}

pub struct VisualEditor {
    target: VisualTarget,
    image: image::DynamicImage,
    texture: Option<egui::TextureHandle>,
    /// 1.0 shows the largest crop that fits; larger values zoom in.
    zoom: f32,
    /// Crop centre as a fraction of the image.
    center: (f32, f32),
}

impl VisualEditor {
    pub fn new(target: VisualTarget, image: image::DynamicImage) -> Self {
        Self {
            target,
            image,
            texture: None,
            zoom: 1.0,
            center: (0.5, 0.5),
        }
    }

    fn banner(&self) -> bool {
        matches!(self.target, VisualTarget::Banner(_))
    }

    fn aspect(&self) -> f32 {
        if self.banner() {
            1000.0 / 375.0
        } else {
            1.0
        }
    }

    /// Crop rectangle in image pixels: (x, y, w, h), always inside the image.
    fn crop(&self) -> (f32, f32, f32, f32) {
        let (iw, ih) = (self.image.width() as f32, self.image.height() as f32);
        let aspect = self.aspect();
        let (bw, bh) = if iw / ih > aspect {
            (ih * aspect, ih)
        } else {
            (iw, iw / aspect)
        };
        let (w, h) = (bw / self.zoom, bh / self.zoom);
        let x = (self.center.0 * iw - w / 2.0).clamp(0.0, iw - w);
        let y = (self.center.1 * ih - h / 2.0).clamp(0.0, ih - h);
        (x, y, w, h)
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        let (x, y, w, h) = self.crop();
        let (ow, oh) = match self.target {
            VisualTarget::Banner(_) => (1000, 375),
            VisualTarget::Icon(_) => (128, 128),
            VisualTarget::Avatar => (256, 256),
        };
        let out = self
            .image
            .crop_imm(x as u32, y as u32, (w as u32).max(1), (h as u32).max(1))
            .resize_exact(ow, oh, image::imageops::FilterType::Lanczos3);
        let mut bytes = std::io::Cursor::new(Vec::new());
        out.write_to(&mut bytes, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        let limit = if self.banner() { 512 * 1024 } else { 256 * 1024 };
        if bytes.get_ref().len() > limit {
            return Err("A imagem convertida ainda é muito grande. Escolha outra imagem.".into());
        }
        Ok(bytes.into_inner())
    }
}

pub fn visual_editor(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some(mut editor) = app.workspace_ui.extras.visual_editor.take() else {
        return;
    };
    if editor.texture.is_none() {
        let rgba = editor.image.to_rgba8();
        editor.texture = Some(ctx.load_texture(
            "visual-editor",
            egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            ),
            egui::TextureOptions::LINEAR,
        ));
    }
    let title = match editor.target {
        VisualTarget::Banner(_) => "Ajustar banner",
        VisualTarget::Icon(_) => "Ajustar ícone",
        VisualTarget::Avatar => "Ajustar foto de perfil",
    };
    let width = components::modal_width(ctx, 460.0);
    let result = components::modal(
        ctx,
        "visual-editor",
        title,
        components::ModalOptions::new(460.0)
            .primary("Salvar")
            .secondary("Cancelar"),
        |ui| {
            ui.label("Arraste a imagem para posicionar e use o zoom para ajustar o tamanho.");
            ui.add_space(8.0);
            let preview_w = (width - 40.0).min(400.0);
            let size = egui::vec2(preview_w, preview_w / editor.aspect());
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::drag());
            let (iw, ih) = (editor.image.width() as f32, editor.image.height() as f32);
            if resp.dragged() {
                let (_, _, w, h) = editor.crop();
                let d = resp.drag_delta();
                editor.center.0 = (editor.center.0 - d.x * w / size.x / iw).clamp(0.0, 1.0);
                editor.center.1 = (editor.center.1 - d.y * h / size.y / ih).clamp(0.0, 1.0);
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    editor.zoom = (editor.zoom * (1.0 + scroll * 0.002)).clamp(1.0, 5.0);
                }
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            let (x, y, w, h) = editor.crop();
            let uv = egui::Rect::from_min_max(
                egui::pos2(x / iw, y / ih),
                egui::pos2((x + w) / iw, (y + h) / ih),
            );
            let radius: u8 = match editor.target {
                VisualTarget::Banner(_) => 6,
                VisualTarget::Icon(_) => 18,
                VisualTarget::Avatar => 128,
            };
            if let Some(t) = &editor.texture {
                ui.painter().add(
                    egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE)
                        .with_texture(t.id(), uv),
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Zoom");
                ui.add(egui::Slider::new(&mut editor.zoom, 1.0..=5.0).show_value(false));
                if ui.button("Redefinir").clicked() {
                    editor.zoom = 1.0;
                    editor.center = (0.5, 0.5);
                }
            });
        },
    );
    if result.primary {
        match editor.encode() {
            Ok(bytes) => match editor.target {
                VisualTarget::Avatar => app.workspace_ui.profile_avatar = bytes,
                VisualTarget::Banner(guild) | VisualTarget::Icon(guild) => {
                    let banner = editor.banner();
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
            },
            Err(error) => {
                app.notify(error);
                app.workspace_ui.extras.visual_editor = Some(editor);
            }
        }
    } else if !result.cancelled() {
        app.workspace_ui.extras.visual_editor = Some(editor);
    }
}
