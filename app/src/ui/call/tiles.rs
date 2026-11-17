use super::*;
use crate::ui::avatar::{Avatar, AvatarSize};

/// Call panel embedded at the top of a conversation (group or DM).
pub fn embedded(app: &mut LanvoxApp, ui: &mut egui::Ui, call: ChannelId) {
    let in_call = app.view.as_ref().and_then(|v| v.my_voice().channel) == Some(call);
    if !in_call {
        let n = app
            .view
            .as_ref()
            .map(|v| v.in_voice(call).len())
            .unwrap_or(0);
        Panel::top("dm-call-banner")
            .exact_size(52.0)
            .frame(
                Frame::new()
                    .fill(theme::color(theme::CALL_BG))
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                    icons::paint(ui.painter(), r, Icon::Speaker, theme::GREEN);
                    ui.label(
                        RichText::new(if n == 0 {
                            "Chamada tocando".to_string()
                        } else {
                            format!("Chamada em andamento · {n} na chamada")
                        })
                        .color(theme::TEXT_BRIGHT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, "Entrar na chamada").clicked() {
                            app.join_call(call);
                        }
                    });
                });
            });
        return;
    }
    Panel::top("dm-call")
        .exact_size(340.0)
        .frame(Frame::new().fill(theme::color(theme::CALL_BG)))
        .show(ui, |ui| {
            Panel::bottom("dm-call-controls")
                .exact_size(84.0)
                .frame(Frame::new().fill(theme::color(theme::CALL_BG)))
                .show(ui, |ui| controls(app, ui, None));
            egui::CentralPanel::default()
                .frame(
                    Frame::new()
                        .fill(theme::color(theme::CALL_BG))
                        .inner_margin(Margin::same(12)),
                )
                .show(ui, |ui| {
                    if let Some(key) = app.focused {
                        let strip_h = 110.0;
                        let stage = Rect::from_min_max(
                            ui.max_rect().min,
                            pos2(
                                ui.max_rect().right(),
                                ui.max_rect().bottom() - strip_h - 8.0,
                            ),
                        );
                        let mut stage_ui = ui.new_child(egui::UiBuilder::new().max_rect(stage));
                        stream_stage(app, &mut stage_ui, key, false);
                        let strip = Rect::from_min_max(
                            pos2(ui.max_rect().left(), stage.bottom() + 8.0),
                            ui.max_rect().max,
                        );
                        let mut strip_ui = ui.new_child(egui::UiBuilder::new().max_rect(strip));
                        tiles(app, &mut strip_ui, call, true);
                    } else {
                        tiles(app, ui, call, false);
                    }
                });
        });
}

pub(super) fn tiles(app: &mut LanvoxApp, ui: &mut egui::Ui, channel: ChannelId, strip: bool) {
    let ctx = ui.ctx().clone();
    let Some(view) = &app.view else { return };
    let people = view.in_voice(channel);
    let mut items: Vec<TileKind> = Vec::new();
    for id in &people {
        items.push(TileKind::Person(*id));
        if view.voice.get(id).map(|v| v.streaming).unwrap_or(false) {
            items.push(TileKind::Stream(*id));
        }
    }
    let area = ui.max_rect();
    let n = items.len().max(1);
    let gap = 8.0;
    if strip {
        // Keep the tiles legible and make every participant reachable when the
        // total width is larger than the strip, including at the minimum window size.
        let h = area.height().max(1.0);
        let w = h * 16.0 / 9.0;
        let content_width = (n as f32 * w + (n.saturating_sub(1)) as f32 * gap).max(area.width());
        egui::ScrollArea::horizontal()
            .id_salt(("call-participants", channel))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_size(vec2(content_width, h));
                let left = ui.min_rect().left()
                    + ((content_width - (n as f32 * w + (n.saturating_sub(1)) as f32 * gap)) / 2.0)
                        .max(0.0);
                let top = ui.min_rect().top();
                let mut actions = Vec::new();
                for (i, item) in items.iter().enumerate() {
                    let rect =
                        Rect::from_min_size(pos2(left + i as f32 * (w + gap), top), vec2(w, h));
                    match item {
                        TileKind::Person(id) => person_tile(app, ui, &ctx, rect, *id),
                        TileKind::Stream(id) => {
                            if let Some(action) = stream_tile(app, ui, &ctx, rect, *id) {
                                actions.push(action);
                            }
                        }
                    }
                }
                for (key, watch) in actions {
                    if watch {
                        app.watch(key);
                    }
                }
            });
        return;
    }
    // Choose the grid that makes 16:9 tiles as large as possible.
    let (cols, tile) = {
        let mut best = (1usize, vec2(0.0, 0.0));
        for cols in 1..=n {
            let rows = n.div_ceil(cols);
            let w = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
            let h = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32;
            let (tw, th) = if w / h > 16.0 / 9.0 {
                (h * 16.0 / 9.0, h)
            } else {
                (w, w * 9.0 / 16.0)
            };
            if tw > best.1.x {
                best = (cols, vec2(tw, th));
            }
        }
        best
    };
    let rows = n.div_ceil(cols);
    let total = vec2(
        cols as f32 * tile.x + (cols as f32 - 1.0) * gap,
        rows as f32 * tile.y + (rows as f32 - 1.0) * gap,
    );
    let origin = pos2(
        area.center().x - total.x / 2.0,
        area.center().y - total.y / 2.0,
    );

    let mut actions: Vec<(StreamKey, bool)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let (c, r) = (i % cols, i / cols);
        let rect = Rect::from_min_size(
            origin + vec2(c as f32 * (tile.x + gap), r as f32 * (tile.y + gap)),
            tile,
        );
        match item {
            TileKind::Person(id) => person_tile(app, ui, &ctx, rect, *id),
            TileKind::Stream(id) => {
                if let Some(a) = stream_tile(app, ui, &ctx, rect, *id) {
                    actions.push(a);
                }
            }
        }
    }
    for (key, watch) in actions {
        if watch {
            app.watch(key);
        }
    }
}

pub(super) fn person_tile(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: Rect,
    id: UserId,
) {
    let Some(view) = &app.view else { return };
    let me = view.me;
    let name = view.user_name(id);
    let avatar_color = view.user_color(id);
    let color = color32(avatar_color);
    let state = view.voice.get(&id).cloned().unwrap_or_default();
    let speaking = view.speaking.contains(&id)
        || (id == me && app.audio_shared.speaking.load(Ordering::Relaxed));
    let resp = ui.interact(rect, ui.scope_id().with(("tile", id)), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!("{}{}", name, if speaking { " falando" } else { "" }),
        )
    });
    if resp.gained_focus() {
        resp.scroll_to_me(Some(Align::Center));
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        8.0,
        color
            .gamma_multiply(0.55)
            .lerp_to_gamma(theme::color(theme::TILE_BG), 0.35),
    );
    let tex = if state.camera {
        app.texture_for(ctx, (id, StreamKind::Camera))
    } else {
        None
    };
    if let Some((tex, size)) = tex {
        let fit = cover(rect, size);
        painter.image(
            tex,
            fit,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        let d = (rect.height() * 0.38).clamp(28.0, 96.0);
        let av = Rect::from_center_size(rect.center(), vec2(d, d));
        let texture = app.avatar_texture(ctx, id);
        Avatar::new(&name, AvatarSize::Xl)
            .user(id)
            .color(avatar_color)
            .texture(texture.as_ref())
            .speaking(speaking)
            .paint(ui, av);
    }
    // Name tag.
    let galley = painter.layout_no_wrap(name.clone(), FontId::proportional(13.0), Color32::WHITE);
    let extra = if state.muted || state.deafened {
        20.0
    } else {
        0.0
    };
    let tag = Rect::from_min_size(
        pos2(rect.left() + 8.0, rect.bottom() - 30.0),
        vec2(galley.size().x + 16.0 + extra, 22.0),
    );
    painter.rect_filled(tag, 6.0, Color32::from_black_alpha(150));
    painter.galley(
        pos2(tag.left() + 8.0, tag.center().y - galley.size().y / 2.0),
        galley,
        Color32::WHITE,
    );
    if state.muted || state.deafened {
        let icon = if state.deafened {
            Icon::HeadphonesOff
        } else {
            Icon::MicOff
        };
        icons::paint(
            &painter,
            Rect::from_min_size(pos2(tag.right() - 22.0, tag.top() + 3.0), vec2(16.0, 16.0)),
            icon,
            Color32::WHITE,
        );
    }
    if speaking {
        painter.rect_stroke(
            rect.shrink(1.0),
            8.0,
            Stroke::new(3.0, theme::GREEN),
            egui::StrokeKind::Inside,
        );
    }
    if resp.has_focus() {
        painter.rect_stroke(
            rect.shrink(2.0),
            8.0,
            Stroke::new(2.0, Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }
    if id != me {
        app.person_audio(id, &name);
        let resp = resp.on_hover_text("Botão direito para ajustar volume");
        person_audio_menu(app, &resp, id, &name);
    }
}

/// Attach the per-person audio menu to `resp` (right click). Shared by every place that lists
/// call participants.
///
/// Unlike `Response::context_menu` this does not rely only on `resp.secondary_clicked()`: if the
/// pointer is over `resp.rect` and no other layer covers it, a right click opens the menu even
/// when another widget in the same layer overlaps the rect and steals the click. The menu also
/// stays open for clicks inside it (egui's default closes it on the first click, which made the
/// slider and checkbox unusable).
pub fn person_audio_menu(app: &mut LanvoxApp, resp: &egui::Response, id: UserId, name: &str) {
    if id == app.me() {
        return;
    }
    audio_popup(resp, |ui| person_audio_controls(app, ui, id, name));
}

/// Right-click menu with the local volume of someone's screen share (not their voice).
pub fn stream_audio_menu(app: &mut LanvoxApp, resp: &egui::Response, id: UserId, name: &str) {
    if id == app.me() {
        return;
    }
    audio_popup(resp, |ui| stream_audio_controls(app, ui, id, name));
}

fn audio_popup(resp: &egui::Response, add_contents: impl FnOnce(&mut egui::Ui)) {
    let ctx = resp.ctx.clone();
    let right_click_here = ctx.input(|i| {
        i.pointer.button_clicked(egui::PointerButton::Secondary)
            && i.pointer
                .interact_pos()
                .is_some_and(|p| resp.rect.contains(p) && resp.interact_rect.contains(p))
    }) && ctx
        .pointer_interact_pos()
        .and_then(|p| ctx.layer_id_at(p))
        .is_none_or(|l| l == resp.layer_id);
    let open =
        (resp.secondary_clicked() || right_click_here).then_some(egui::SetOpenCommand::Bool(true));
    egui::Popup::menu(resp)
        .open_memory(open)
        .at_pointer_fixed()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(add_contents);
}

/// Local-only volume and mute for one person's screen-share audio.
pub fn stream_audio_controls(app: &mut LanvoxApp, ui: &mut egui::Ui, id: UserId, name: &str) {
    ui.set_min_width(200.0);
    ui.label(RichText::new(format!("Transmissão de {name}")).strong());
    ui.separator();
    ui.label("Volume da transmissão");
    let (mut vol, mut muted) = app.stream_audio(id, name);
    let mut pct = (vol * 100.0).round();
    let mut changed = ui
        .add(egui::Slider::new(&mut pct, 0.0..=200.0).suffix("%"))
        .changed();
    vol = pct / 100.0;
    changed |= ui.checkbox(&mut muted, "Silenciar transmissão").changed();
    if ui.small_button("Redefinir volume").clicked() {
        vol = 1.0;
        muted = false;
        changed = true;
    }
    if changed {
        app.set_stream_audio(id, name, vol, muted);
    }
    if ui.input(|i| i.pointer.any_released()) {
        app.save_preferences();
    }
}

/// Local-only volume and mute for one person; shared by the call tiles and the member list.
pub fn person_audio_controls(app: &mut LanvoxApp, ui: &mut egui::Ui, id: UserId, name: &str) {
    ui.set_min_width(200.0);
    ui.label(RichText::new(name).strong());
    ui.separator();
    ui.label("Volume do usuário");
    let (mut vol, mut muted) = app.person_audio(id, name);
    let mut pct = (vol * 100.0).round();
    let mut changed = ui
        .add(egui::Slider::new(&mut pct, 0.0..=200.0).suffix("%"))
        .changed();
    vol = pct / 100.0;
    changed |= ui.checkbox(&mut muted, "Silenciar").changed();
    if ui.small_button("Redefinir volume").clicked() {
        vol = 1.0;
        muted = false;
        changed = true;
    }
    if changed {
        app.set_person_audio(id, name, vol, muted);
    }
    if ui.input(|i| i.pointer.any_released()) {
        app.save_preferences();
    }
}

/// A tile for someone's screen share. Returns a watch request when clicked.
pub(super) fn stream_tile(
    app: &mut LanvoxApp,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: Rect,
    owner: UserId,
) -> Option<(StreamKey, bool)> {
    let me = app.me();
    let key = (owner, StreamKind::Screen);
    let name = app
        .view
        .as_ref()
        .map(|v| v.user_name(owner))
        .unwrap_or_default();
    let resp = ui.interact(
        rect,
        ui.scope_id().with(("stream-tile", owner)),
        Sense::click(),
    );
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled(),
            format!("Assistir transmissão de {name}"),
        )
    });
    if resp.gained_focus() {
        resp.scroll_to_me(Some(Align::Center));
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, theme::color(theme::TILE_BG));
    let showing = owner == me || app.watching.contains(&key);
    let mut tex = None;
    if showing {
        tex = app.texture_for(ctx, key);
    }
    if let Some((tex, size)) = tex {
        let fit = contain(rect, size);
        painter.image(
            tex,
            fit,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        app.paint_cursor(ctx, &painter, fit, owner);
    } else {
        painter.text(
            rect.center() - vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            format!("{name} está transmitindo"),
            FontId::proportional(15.0),
            theme::TEXT_BRIGHT,
        );
        let btn = Rect::from_center_size(rect.center() + vec2(0.0, 18.0), vec2(124.0, 30.0));
        let hover = resp.hovered();
        painter.rect_filled(
            btn,
            4.0,
            if hover {
                theme::ACTION_FILL.gamma_multiply(1.1)
            } else {
                theme::ACTION_FILL
            },
        );
        painter.text(
            btn.center(),
            Align2::CENTER_CENTER,
            "Assistir transmissão",
            FontId::proportional(13.0),
            Color32::WHITE,
        );
    }
    let live = Rect::from_min_size(rect.left_top() + vec2(8.0, 8.0), vec2(58.0, 18.0));
    painter.rect_filled(live, 4.0, theme::RED);
    painter.text(
        live.center(),
        Align2::CENTER_CENTER,
        "AO VIVO",
        FontId::proportional(11.0),
        Color32::WHITE,
    );
    if resp.hovered() || resp.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.0),
            8.0,
            Stroke::new(2.0, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    if owner != me {
        app.stream_audio(owner, &name);
        let resp = resp
            .clone()
            .on_hover_text("Botão direito para ajustar volume");
        stream_audio_menu(app, &resp, owner, &name);
    }
    if resp.clicked() {
        return Some((key, true));
    }
    None
}

/// Fit `size` inside `rect`, keeping the aspect ratio.
pub(super) fn contain(rect: Rect, size: egui::Vec2) -> Rect {
    let s = (rect.width() / size.x).min(rect.height() / size.y);
    Rect::from_center_size(rect.center(), size * s)
}

/// Fill `rect` with `size`, keeping the aspect ratio (parts may be cut).
pub(super) fn cover(rect: Rect, size: egui::Vec2) -> Rect {
    let s = (rect.width() / size.x).max(rect.height() / size.y);
    Rect::from_center_size(rect.center(), size * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn participant_avatar_tracks_profile_changes_even_while_waiting_for_camera() {
        let (mut app, ctx) = crate::app::ui_regressions::fixture();
        let mut previous_texture = None;
        for (avatar, camera) in [
            (crate::app::ui_regressions::avatar_png(30), false),
            (crate::app::ui_regressions::avatar_png(200), true),
            (Vec::new(), false),
            (vec![0, 1, 2], false),
        ] {
            let view = app.view.as_mut().unwrap();
            view.apply_workspace(&WorkspaceEvent::ProfileUpdated(Profile {
                user: 2,
                avatar,
                ..Default::default()
            }));
            view.voice.get_mut(&2).unwrap().camera = camera;
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                person_tile(
                    &mut app,
                    ui,
                    &ctx,
                    Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 180.0)),
                    2,
                );
            });
            let texture = app.avatar_texture(&ctx, 2);
            if let Some(texture) = texture {
                assert_ne!(previous_texture, Some(texture.id()));
                assert!(crate::app::ui_regressions::renders_texture(
                    &ctx,
                    output,
                    texture.id()
                ));
                previous_texture = Some(texture.id());
            } else {
                assert!(!crate::app::ui_regressions::renders_texture(
                    &ctx,
                    output,
                    previous_texture.unwrap()
                ));
            }
        }
    }
}
