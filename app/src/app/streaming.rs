use super::*;

impl LanvoxApp {
    pub fn toggle_camera(&mut self, ctx: &egui::Context) {
        let me = self.me();
        if self.camera.take().is_some() {
            self.send(ClientMsg::StopStream(StreamKind::Camera));
            self.pictures.remove((me, StreamKind::Camera));
            return;
        }
        let Some(client) = &self.client else { return };
        self.camera_preview = None;
        let camera = self.settings.camera.normalized();
        let (width, height) = camera.resolution.size();
        client.send(ClientMsg::StartStream(StreamInfo {
            owner: me,
            kind: StreamKind::Camera,
            label: "Câmera".into(),
            width,
            height,
            fps: camera.fps,
        }));
        self.camera = Some(CameraSession::start_configured(
            self.settings.camera_index,
            Some(client.sender()),
            self.pictures.clone(),
            me,
            ctx.clone(),
            camera,
        ));
    }

    pub fn restart_camera(&mut self, ctx: &egui::Context) {
        if self.camera.is_some() {
            self.toggle_camera(ctx);
            self.toggle_camera(ctx);
        } else if self.camera_preview.take().is_some() {
            self.camera_preview = Some(CameraSession::start_configured(
                self.settings.camera_index,
                None,
                self.pictures.clone(),
                self.me(),
                ctx.clone(),
                self.settings.camera,
            ));
        }
    }

    // ----- screen share -----

    pub fn open_share_picker(&mut self) {
        self.modal = Modal::SharePicker;
        self.picker = PickerState::default();
        let slot = Arc::new(Mutex::new(None));
        self.picker.loading = Some(slot.clone());
        std::thread::spawn(move || {
            let sources = crate::media::screen::list_sources(true);
            *slot.lock() = Some(sources);
        });
    }

    pub fn go_live(&mut self, ctx: &egui::Context, source_index: usize) {
        let Some(source) = self.picker.sources.get(source_index).cloned() else {
            return;
        };
        let me = self.me();
        self.stop_share();
        let st = self.settings.stream;
        let (w, h) = st.dimensions(source.width, source.height);
        if let Some(client) = &self.client {
            client.keyframes.screen_viewers.store(0, Ordering::Relaxed);
            client.send(ClientMsg::StartStream(StreamInfo {
                owner: me,
                kind: StreamKind::Screen,
                label: source.name.clone(),
                width: w,
                height: h,
                fps: st.fps,
            }));
            self.share = Some(ScreenShare::start(
                &source,
                st,
                client.sender(),
                client.keyframes.clone(),
                self.pictures.clone(),
                me,
                ctx.clone(),
            ));
            self.focused = Some((me, StreamKind::Screen));
        }
        self.modal = Modal::None;
        self.settings.save();
    }

    pub fn stop_share(&mut self) {
        if self.share.take().is_some() {
            self.send(ClientMsg::StopStream(StreamKind::Screen));
            let me = self.me();
            self.pictures.remove((me, StreamKind::Screen));
            self.textures.remove(&(me, StreamKind::Screen));
            if self.focused == Some((me, StreamKind::Screen)) {
                self.focused = None;
            }
        }
    }

    /// Start receiving every screen share in the call I am in, without any click. Streams the
    /// user closed on purpose stay closed until the sharer starts a new one.
    pub fn auto_watch_streams(&mut self) {
        let Some(view) = &self.view else { return };
        let Some(channel) = view.my_voice().channel else {
            return;
        };
        let me = view.me;
        let keys: Vec<StreamKey> = view
            .in_voice(channel)
            .into_iter()
            .filter(|id| *id != me && view.voice.get(id).is_some_and(|v| v.streaming))
            .map(|id| (id, StreamKind::Screen))
            .filter(|key| !self.watching.contains(key) && !self.unwatched.contains(key))
            .collect();
        for key in keys {
            self.watching.insert(key);
            self.send(ClientMsg::Watch {
                owner: key.0,
                kind: key.1,
            });
        }
    }

    pub fn watch(&mut self, key: StreamKey) {
        let me = self.me();
        self.unwatched.remove(&key);
        if key.0 != me && key.1 == StreamKind::Screen && self.watching.insert(key) {
            self.send(ClientMsg::Watch {
                owner: key.0,
                kind: key.1,
            });
        }
        self.focused = Some(key);
    }

    pub fn stop_watching(&mut self, key: StreamKey) {
        if key.1 == StreamKind::Screen {
            self.unwatched.insert(key);
        }
        if self.watching.remove(&key) {
            if key.1 == StreamKind::Screen {
                self.mixer.stop_stream(key.0);
            }
            self.send(ClientMsg::Unwatch {
                owner: key.0,
                kind: key.1,
            });
            self.pictures.remove(key);
            self.textures.remove(&key);
        }
        if self.focused == Some(key) {
            self.focused = None;
            self.fullscreen = false;
        }
        if self.popout == Some(key) {
            self.popout = None;
        }
    }

    /// Upload a stream's newest picture as a texture (only when it changed).
    pub fn texture_for(
        &mut self,
        ctx: &egui::Context,
        key: StreamKey,
    ) -> Option<(egui::TextureId, egui::Vec2)> {
        let known = self.textures.get(&key).map(|(_, v)| *v);
        let update = self.pictures.with(|map| {
            let p = map.get(&key)?;
            if Some(p.version) == known {
                return Some(None);
            }
            Some(Some((
                egui::ColorImage::from_rgba_unmultiplied(
                    [p.width as usize, p.height as usize],
                    &p.rgba,
                ),
                p.version,
            )))
        });
        match update {
            None => {
                self.textures.remove(&key);
                return None;
            }
            Some(Some((image, version))) => {
                let opts = egui::TextureOptions::LINEAR;
                match self.textures.get_mut(&key) {
                    Some((tex, v)) => {
                        tex.set(image, opts);
                        *v = version;
                    }
                    None => {
                        let tex =
                            ctx.load_texture(format!("stream-{}-{:?}", key.0, key.1), image, opts);
                        self.textures.insert(key, (tex, version));
                    }
                }
            }
            Some(None) => {}
        }
        self.textures
            .get(&key)
            .map(|(t, _)| (t.id(), t.size_vec2()))
    }

    /// Draws a sharer's pointer over their picture, from the separately sent position.
    /// `picture` is where the picture ended up on screen.
    pub fn paint_cursor(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        picture: egui::Rect,
        owner: UserId,
    ) {
        let Some(view) = self.pictures.cursor(owner) else {
            return;
        };
        if !view.visible || view.area_w == 0 {
            self.cursor_textures.remove(&owner);
            return;
        }
        // Pointer pixels per shared-area pixel, so it is the size the sharer sees.
        let scale = picture.width() / view.area_w as f32;
        let at = picture.min + egui::vec2(view.x * picture.width(), view.y * picture.height());
        let Some(shape) = view.shape.as_deref() else {
            // The image has not arrived yet: a plain arrow until it does.
            let s = (scale * 18.0).clamp(10.0, 28.0) / 18.0;
            let p = |x: f32, y: f32| at + egui::vec2(x, y) * s;
            painter.add(egui::Shape::convex_polygon(
                vec![p(0.0, 0.0), p(0.0, 16.0), p(11.0, 12.0)],
                egui::Color32::WHITE,
                egui::Stroke::new(1.0, egui::Color32::BLACK),
            ));
            return;
        };
        let fresh = !matches!(
            self.cursor_textures.get(&owner),
            Some((version, _)) if *version == view.shape_version
        );
        if fresh {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [shape.w as usize, shape.h as usize],
                &shape.rgba,
            );
            let texture = ctx.load_texture(
                format!("cursor-{owner}"),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.cursor_textures
                .insert(owner, (view.shape_version, texture));
        }
        let Some((_, texture)) = self.cursor_textures.get(&owner) else {
            return;
        };
        let size = egui::vec2(f32::from(shape.w), f32::from(shape.h)) * scale;
        let hot = egui::vec2(f32::from(shape.hot_x), f32::from(shape.hot_y)) * scale;
        painter.image(
            texture.id(),
            egui::Rect::from_min_size(at - hot, size),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    pub fn stream_stats(&self, key: StreamKey) -> Option<(u32, u32, f32, f32)> {
        self.pictures
            .with(|map| map.get(&key).map(|p| (p.width, p.height, p.fps, p.kbps)))
    }
}
