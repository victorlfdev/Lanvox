use super::*;

pub(super) fn share_picker(app: &mut LanvoxApp, ctx: &egui::Context) {
    let ready = app
        .picker
        .loading
        .as_ref()
        .and_then(|slot| slot.lock().take());
    if let Some(sources) = ready {
        app.picker.sources = sources;
        app.picker.loading = None;
        app.picker.thumbs.clear();
        for (index, source) in app.picker.sources.iter().enumerate() {
            if let Some((width, height, rgba)) = &source.thumb {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [*width as usize, *height as usize],
                    rgba,
                );
                app.picker.thumbs.insert(
                    index,
                    ctx.load_texture(
                        format!("source-{index}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    ),
                );
            }
        }
    }
    let mut close = false;
    let mut go = None;
    let mut refresh = false;
    let height = ctx.content_rect().height();
    let can_go = app.picker.loading.is_none()
        && app
            .picker
            .selected
            .and_then(|index| app.picker.sources.get(index))
            .is_some_and(|source| {
                matches!(source.id, SourceId::Window(_)) == app.picker.windows_tab
            });
    let response = components::modal(
        ctx,
        "share-picker",
        "Compartilhar sua tela",
        components::ModalOptions::new(720.0)
            .primary("Iniciar transmissão")
            .enabled(can_go)
            .secondary("Cancelar"),
        |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(!app.picker.windows_tab, "Telas")
                    .clicked()
                {
                    if app.picker.windows_tab {
                        app.picker.selected = None;
                    }
                    app.picker.windows_tab = false;
                }
                if ui
                    .selectable_label(app.picker.windows_tab, "Aplicativos")
                    .clicked()
                {
                    if !app.picker.windows_tab {
                        app.picker.selected = None;
                    }
                    app.picker.windows_tab = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            app.picker.loading.is_none(),
                            egui::Button::new("Atualizar lista"),
                        )
                        .clicked()
                    {
                        refresh = true;
                    }
                });
            });
            ui.label(
                RichText::new("Selecione a tela ou o aplicativo que os participantes poderão ver.")
                    .small()
                    .color(theme::color(theme::TEXT_MUTED)),
            );
            Frame::new()
                .fill(theme::color(theme::SIDEBAR))
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("share-sources")
                        .max_height((height * 0.25).clamp(116.0, 220.0))
                        .show(ui, |ui| {
                            if app.picker.loading.is_some() {
                                ui.spinner();
                                ui.label("Buscando telas e janelas…");
                                ctx.request_repaint_after(Duration::from_millis(100));
                            }
                            let indices: Vec<_> = app
                                .picker
                                .sources
                                .iter()
                                .enumerate()
                                .filter(|(_, source)| {
                                    matches!(source.id, SourceId::Window(_))
                                        == app.picker.windows_tab
                                })
                                .map(|(index, _)| index)
                                .collect();
                            if indices.is_empty() && app.picker.loading.is_none() {
                                ui.label(if app.picker.windows_tab {
                                    "Nenhum aplicativo disponível. Abra uma janela e atualize a lista."
                                } else {
                                    "Nenhuma tela disponível. Atualize a lista e confira as permissões de captura."
                                });
                            }
                            let columns = ((ui.available_width() / 180.0).floor() as usize).clamp(1, 3);
                            let cell_width = ((ui.available_width() - (columns.saturating_sub(1) as f32 * ui.spacing().item_spacing.x)) / columns as f32).max(100.0);
                            for row in indices.chunks(columns) {
                                ui.horizontal(|ui| {
                                    for &index in row {
                                        let (rect, response) = ui.allocate_exact_size(
                                            vec2(cell_width, 112.0),
                                            Sense::click(),
                                        );
                                        let thumbnail =
                                            Rect::from_min_size(rect.min, vec2(cell_width, 88.0));
                                        ui.painter().rect_filled(
                                            thumbnail,
                                            6,
                                            theme::color(theme::RAIL),
                                        );
                                        if let Some(texture) = app.picker.thumbs.get(&index) {
                                            let size = texture.size_vec2();
                                            let scale = (thumbnail.width() / size.x)
                                                .min(thumbnail.height() / size.y);
                                            ui.painter().image(
                                                texture.id(),
                                                Rect::from_center_size(
                                                    thumbnail.center(),
                                                    size * scale,
                                                ),
                                                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                                Color32::WHITE,
                                            );
                                        }
                                        let selected = app.picker.selected == Some(index);
                                        if selected || response.hovered() || response.has_focus() {
                                            ui.painter().rect_stroke(
                                                thumbnail,
                                                6,
                                                Stroke::new(2.0, theme::color(theme::ACCENT)),
                                                egui::StrokeKind::Inside,
                                            );
                                        }
                                        let source = &app.picker.sources[index];
                                        let label = if source.app.is_empty() {
                                            source.name.clone()
                                        } else {
                                            format!("{} — {}", source.name, source.app)
                                        };
                                        let response = response.on_hover_text(&label);
                                        if response.gained_focus() {
                                            response.scroll_to_me(Some(Align::Center));
                                        }
                                        let galley = ui.painter().layout_no_wrap(
                                            label.clone(),
                                            FontId::proportional(12.0),
                                            theme::color(theme::TEXT),
                                        );
                                        ui.painter().with_clip_rect(rect).galley(
                                            pos2(rect.left(), thumbnail.bottom() + 4.0),
                                            galley,
                                            theme::color(theme::TEXT),
                                        );
                                        response.widget_info(|| {
                                            egui::WidgetInfo::selected(
                                                egui::Role::RadioButton,
                                                ui.is_enabled(),
                                                selected,
                                                format!("{}: {}, {} por {} pixels", if app.picker.windows_tab { "Aplicativo" } else { "Tela" }, label, source.width, source.height),
                                            )
                                        });
                                        if response.clicked() {
                                            app.picker.selected = Some(index);
                                        }
                                        if response.double_clicked() {
                                            go = Some(index);
                                        }
                                    }
                                });
                            }
                        });
                });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let text = !app.settings.stream.motion_video;
                if ui.selectable_label(!text, "Vídeo e jogos").clicked() {
                    app.settings.stream.resolution = Resolution::P720;
                    app.settings.stream.fps = 60;
                    app.settings.stream.motion_video = true;
                    // Games are better captured through their own window: this
                    // keeps desktop changes out of the stream and selects the
                    // Windows Graphics Capture path when it is available.
                    app.picker.windows_tab = true;
                    app.picker.selected = None;
                }
                if ui.selectable_label(text, "Texto nítido").clicked() {
                    app.settings.stream.resolution = TEXT_PRESET.0;
                    app.settings.stream.fps = TEXT_PRESET.1;
                    app.settings.stream.motion_video = false;
                }
                egui::ComboBox::from_id_salt("share-resolution")
                    .selected_text(app.settings.stream.resolution.label())
                    .show_ui(ui, |ui| {
                        for resolution in Resolution::ALL {
                            ui.selectable_value(
                                &mut app.settings.stream.resolution,
                                resolution,
                                resolution.label(),
                            );
                        }
                    });
                egui::ComboBox::from_id_salt("share-fps")
                    .selected_text(format!("{} fps", app.settings.stream.fps))
                    .show_ui(ui, |ui| {
                        for fps in FRAME_RATES {
                            ui.selectable_value(
                                &mut app.settings.stream.fps,
                                fps,
                                format!("{fps} fps"),
                            );
                        }
                    });
            });
            ui.checkbox(
                &mut app.settings.stream.share_audio,
                "Incluir áudio do aplicativo/sistema",
            );
            if app.settings.stream.share_audio {
                ui.label(
                    RichText::new(
                        "No Windows, o áudio compartilhado não inclui a própria chamada do Lanvox.",
                    )
                    .small()
                    .color(theme::color(theme::TEXT_MUTED)),
                );
            }
            ui.collapsing("Qualidade avançada", |ui| {
                ui.checkbox(
                    &mut app.settings.stream.motion_video,
                    "Otimizar movimento com H.264 (GPU quando disponível)",
                );
                ui.checkbox(
                    &mut app.settings.stream.adaptive,
                    "Adaptar resolução e FPS à rede",
                );
            });
            if let Some(index) = app.picker.selected {
                if let Some(source) = app.picker.sources.get(index) {
                    let (width, height) =
                        app.settings.stream.dimensions(source.width, source.height);
                    ui.label(RichText::new(format!("Selecionado: {}", source.name)).strong());
                    ui.label(RichText::new(format!("Solicitado: {width}×{height}, até {} fps. A qualidade efetiva depende da rede.",app.settings.stream.fps)).small().color(theme::color(theme::TEXT_MUTED)));
                }
            } else if app.picker.loading.is_none() {
                ui.label(
                    RichText::new("Selecione uma fonte para iniciar.")
                        .small()
                        .color(theme::color(theme::TEXT_MUTED)),
                );
            }
        },
    );
    if response.primary {
        go = app.picker.selected;
    }
    if response.cancelled() {
        close = true;
    }
    if refresh {
        let windows_tab = app.picker.windows_tab;
        app.open_share_picker();
        app.picker.windows_tab = windows_tab;
    } else if let Some(index) = go {
        app.go_live(ctx, index);
    } else if close {
        app.modal = Modal::None;
        app.picker = Default::default();
    }
}
