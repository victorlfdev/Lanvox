use super::*;

impl LanvoxApp {
    pub fn pick_avatar(&mut self) {
        if self.file_pending.is_some() {
            return;
        }
        let (tx, rx) = bounded(1);
        self.file_pending = Some(rx);
        std::thread::spawn(move || {
            let task = rfd::FileDialog::new()
                .add_filter("Imagem PNG/JPEG", &["png", "jpg", "jpeg"])
                .pick_file()
                .map(|path| -> Result<image::DynamicImage, String> {
                    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
                    if bytes.len() > 2 * 1024 * 1024 {
                        return Err("O avatar precisa ter até 2 MiB.".into());
                    }
                    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                        .with_guessed_format()
                        .map_err(|e| e.to_string())?;
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(4096);
                    limits.max_image_height = Some(4096);
                    limits.max_alloc = Some(64 * 1024 * 1024);
                    reader.limits(limits);
                    let image = reader.decode().map_err(|e| e.to_string())?;
                    // Keep the editor responsive on very large photos.
                    Ok(if image.width().max(image.height()) > 1600 {
                        image.thumbnail(1600, 1600)
                    } else {
                        image
                    })
                })
                .map(|result| match result {
                    Ok(bytes) => FileTask::Avatar(bytes),
                    Err(e) => FileTask::Error(e),
                })
                .unwrap_or(FileTask::Cancelled);
            let _ = tx.send(task);
        });
    }

    pub fn attachment_texture(
        &mut self,
        ctx: &egui::Context,
        attachment: &AttachmentInfo,
    ) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.attachment_textures.get(&attachment.id) {
            return Some(texture.clone());
        }
        if let Some((_, bytes)) = self.workspace_ui.downloads.get(&attachment.id) {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(4096);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let img = reader.decode().ok()?.thumbnail(640, 400).to_rgba8();
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            let texture = ctx.load_texture(
                format!("attachment-{}", attachment.id),
                color,
                egui::TextureOptions::LINEAR,
            );
            self.attachment_textures
                .insert(attachment.id, texture.clone());
            return Some(texture);
        }
        None
    }

    pub fn avatar_texture(
        &mut self,
        ctx: &egui::Context,
        user: UserId,
    ) -> Option<egui::TextureHandle> {
        let bytes = &self
            .view
            .as_ref()?
            .workspace
            .profiles
            .iter()
            .find(|p| p.user == user)?
            .avatar;
        if bytes.is_empty() {
            return None;
        }
        let hash = self
            .view
            .as_ref()?
            .avatar_revisions
            .get(&user)
            .copied()
            .unwrap_or(0);
        if let Some((existing, texture)) = self.avatar_textures.get(&user) {
            if *existing == hash {
                return Some(texture.clone());
            }
        }
        if bytes.len() > 2 * 1024 * 1024 {
            return None;
        }
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(1024);
        limits.max_image_height = Some(1024);
        limits.max_alloc = Some(8 * 1024 * 1024);
        reader.limits(limits);
        let img = reader.decode().ok()?.thumbnail(128, 128).to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        let texture = ctx.load_texture(
            format!("avatar-{user}"),
            color,
            egui::TextureOptions::LINEAR,
        );
        self.avatar_textures.insert(user, (hash, texture.clone()));
        Some(texture)
    }
}
