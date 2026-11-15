use super::*;

pub fn extras(app: &mut LanvoxApp, ctx: &egui::Context) {
    forwarding(app, ctx);
    create_thread(app, ctx);
    create_poll(app, ctx);
    gallery(app, ctx);
}

pub(super) fn forwarding(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some(message) = app.workspace_ui.forwarding.clone() else {
        return;
    };
    let channels: Vec<_> = app
        .view
        .as_ref()
        .map(|v| {
            v.channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Text && v.can_send(c.id))
                .map(|c| (c.id, c.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let selected = app.workspace_ui.forwarding_target;
    let result = crate::ui::components::modal(
        ctx,
        "forward-message",
        "Encaminhar mensagem",
        crate::ui::components::ModalOptions::new(460.0)
            .primary("Encaminhar")
            .enabled(selected.is_some())
            .secondary("Cancelar"),
        |ui| {
            ui.label(RichText::new(&message.text).color(theme::TEXT_MUTED));
            ui.add_space(10.0);
            egui::ComboBox::from_id_salt("forward-target")
                .selected_text(
                    channels
                        .iter()
                        .find(|(id, _)| Some(*id) == selected)
                        .map(|(_, name)| name.as_str())
                        .unwrap_or("Escolha uma conversa"),
                )
                .show_ui(ui, |ui| {
                    for (id, name) in &channels {
                        ui.selectable_value(
                            &mut app.workspace_ui.forwarding_target,
                            Some(*id),
                            name,
                        );
                    }
                });
            ui.add(
                egui::TextEdit::multiline(&mut app.workspace_ui.forwarding_note)
                    .hint_text("Adicionar uma observação (opcional)")
                    .desired_rows(2)
                    .char_limit(500)
                    .desired_width(f32::INFINITY),
            );
            ui.label(
                RichText::new(
                    "O host verifica o acesso à origem e ao destino antes de copiar os anexos.",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
        },
    );
    if result.primary {
        if let Some(target) = app.workspace_ui.forwarding_target {
            app.workspace(WorkspaceCommand::Community(CommunityCommand::Forward {
                operation: format!("forward-{:032x}", rand::random::<u128>()),
                message: message.id,
                target,
                note: app.workspace_ui.forwarding_note.clone(),
            }));
        }
        app.workspace_ui.forwarding = None;
    } else if result.cancelled() {
        app.workspace_ui.forwarding = None;
    }
}

pub(super) fn create_thread(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some(message) = app.workspace_ui.thread_from.clone() else {
        return;
    };
    let result = crate::ui::components::modal(
        ctx,
        "create-thread",
        "Criar discussão",
        crate::ui::components::ModalOptions::new(420.0)
            .primary("Criar discussão")
            .enabled(!app.workspace_ui.thread_name.trim().is_empty())
            .secondary("Cancelar"),
        |ui| {
            ui.label("Continue este assunto em uma conversa separada.");
            ui.add(
                egui::TextEdit::singleline(&mut app.workspace_ui.thread_name)
                    .hint_text("Nome da discussão")
                    .char_limit(100)
                    .desired_width(f32::INFINITY),
            );
            ui.label(
                RichText::new(&message.text)
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        },
    );
    if result.primary {
        app.workspace(WorkspaceCommand::Community(
            CommunityCommand::CreateThread {
                parent: message.channel,
                message: Some(message.id),
                name: app.workspace_ui.thread_name.trim().into(),
            },
        ));
        app.workspace_ui.thread_from = None;
    } else if result.cancelled() {
        app.workspace_ui.thread_from = None;
    }
}

pub(super) fn gallery(app: &mut LanvoxApp, ctx: &egui::Context) {
    let Some(id) = app.workspace_ui.gallery else {
        return;
    };
    let images: Vec<_> = app
        .view
        .as_ref()
        .map(|v| {
            v.workspace
                .details
                .iter()
                .flat_map(|d| d.attachments.iter())
                .filter(|a| a.media_type.starts_with("image/") && Some(a.channel) == app.selected)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let Some(index) = images.iter().position(|a| a.id == id) else {
        app.workspace_ui.gallery = None;
        return;
    };
    let info = images[index].clone();
    if app
        .workspace_ui
        .gallery_texture
        .as_ref()
        .is_none_or(|(existing, _)| *existing != id)
    {
        app.workspace_ui.gallery_texture = app
            .workspace_ui
            .downloads
            .get(&id)
            .and_then(|(_, bytes)| image_texture(ctx, &format!("gallery-{id}"), bytes, None))
            .map(|texture| (id, texture));
        if app.workspace_ui.gallery_texture.is_none()
            && info.size <= MAX_INLINE_ATTACHMENT_BYTES as u64
            && app.workspace_ui.preview_requests.insert(id)
        {
            app.workspace(WorkspaceCommand::Download { attachment: id });
        }
    }
    let texture = app
        .workspace_ui
        .gallery_texture
        .as_ref()
        .map(|(_, t)| t.clone());
    let mut close = false;
    let mut next = None;
    let result=egui::Modal::new(egui::Id::unique("image-gallery")).frame(Frame::new().fill(theme::color(theme::PANEL)).corner_radius(8).inner_margin(16)).show(ctx,|ui|{
        ui.set_width((ctx.content_rect().width()-80.0).max(300.0));
        ui.horizontal(|ui|{ui.label(RichText::new(&info.name).strong());ui.with_layout(Layout::right_to_left(Align::Center),|ui|{if widgets::icon_button(ui,Icon::Close,28.0,theme::TEXT,"Fechar imagem").clicked(){close=true;}});});
        ui.horizontal(|ui|{
            if ui.add_enabled(index>0,egui::Button::new("Anterior")).clicked(){next=Some(images[index-1].id);}
            ui.label(format!("{} de {}",index+1,images.len()));
            if ui.add_enabled(index+1<images.len(),egui::Button::new("Próxima")).clicked(){next=Some(images[index+1].id);}
            ui.add(egui::Slider::new(&mut app.workspace_ui.gallery_zoom,0.5..=4.0).text("Zoom"));
            if ui.button("Baixar").clicked(){app.download_attachment(&info);}
        });
        egui::ScrollArea::both().id_salt("gallery-scroll").max_height((ctx.content_rect().height()-180.0).max(180.0)).show(ui,|ui|{
            if let Some(texture)=&texture{let size=texture.size_vec2();let fit=((ui.available_width()-8.0)/size.x).min(1.0);ui.add(egui::Image::new((texture.id(),size*fit*app.workspace_ui.gallery_zoom)));}
            else if app.workspace_ui.downloads.contains_key(&id){ui.label("Esta imagem não pode ser exibida. Use Baixar para abrir o arquivo original.");}
            else{ui.spinner();ui.label("Carregando imagem…");}
        });
    });
    if result.should_close() {
        close = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) && index > 0 {
        next = Some(images[index - 1].id);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) && index + 1 < images.len() {
        next = Some(images[index + 1].id);
    }
    if close {
        app.workspace_ui.gallery = None;
        app.workspace_ui.gallery_texture = None;
    } else if let Some(next) = next {
        app.workspace_ui.gallery = Some(next);
        app.workspace_ui.gallery_zoom = 1.0;
    }
}
