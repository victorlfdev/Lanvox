//! Durable conversations, bounded cache, rich interactions and virtualized rows.

use super::avatar::{Avatar, AvatarSize, PresenceState};

use super::{icons::Icon, theme, widgets};

use crate::{app::LanvoxApp, model::*};

use chrono::{Local, TimeZone};

use egui::{vec2, Align, Frame, Layout, Margin, Panel, RichText};

mod composer;
mod emoji;
mod extras;
mod header;
mod message;
mod poll;
use composer::*;
use emoji::*;
pub use extras::*;
pub use header::*;
pub use message::*;
use poll::*;

fn time_label(t: i64) -> String {
    Local
        .timestamp_opt(t, 0)
        .single()
        .map(|dt| {
            if dt.date_naive() == Local::now().date_naive() {
                dt.format("Hoje às %H:%M").to_string()
            } else {
                dt.format("%d/%m/%Y %H:%M").to_string()
            }
        })
        .unwrap_or_default()
}

pub fn show(app: &mut LanvoxApp, ui: &mut egui::Ui) {
    let Some(channel) = app.selected else { return };
    let Some(ch) = app.view.as_ref().and_then(|v| v.channel(channel)).cloned() else {
        return;
    };
    Panel::bottom(("composer", channel))
        .frame(
            Frame::new()
                .fill(theme::color(theme::CHAT))
                .inner_margin(Margin {
                    left: 16,
                    right: 16,
                    top: 10,
                    bottom: 8,
                }),
        )
        .show(ui, |ui| composer(app, ui, &ch));
    egui::CentralPanel::default().frame(Frame::new().fill(theme::color(theme::CHAT))).show(ui, |ui| {
        let messages = app.view.as_ref().and_then(|v|v.messages.get(&channel)).cloned().unwrap_or_default();
        let width = ui.available_width();
        if (app.workspace_ui.message_width-width).abs()>2.0 { app.workspace_ui.message_width=width; app.workspace_ui.message_heights.clear(); }
        let jump=app.scroll_to_bottom>0;
        app.scroll_to_bottom=app.scroll_to_bottom.saturating_sub(1);
        let loading=app.view.as_ref().is_some_and(|v|v.history_loading.contains(&channel));
        let more=app.view.as_ref().and_then(|v|v.history_more.get(&channel)).copied().unwrap_or(!messages.is_empty());
        let first_unread=app.workspace_ui.first_unread.get(&channel).copied();
        let mut load_older=false;
        let mut first_visible=None;
        let out=egui::ScrollArea::vertical().id_salt(("chat",channel)).auto_shrink(false).stick_to_bottom(true).show_viewport(ui,|ui,viewport| {
            let origin=ui.cursor().min.y;
            ui.add_space(12.0);
            if more { ui.horizontal(|ui| { if loading {ui.spinner();ui.label("Carregando histórico…");} else if ui.small_button("Mensagens anteriores").clicked(){load_older=true;} }); }
            if !jump && !app.demo && viewport.min.y<70.0 && more && !loading {load_older=true;}
            if messages.is_empty() { Frame::new().inner_margin(24).show(ui,|ui| {ui.label(RichText::new("A conversa começa aqui").size(24.0).strong());ui.label("Envie uma mensagem, compartilhe um arquivo ou mencione alguém com @.");}); }
            for (index,m) in messages.iter().enumerate() {
                let day=message_day(m.time);
                let new_day=index==0 || message_day(messages[index-1].time)!=day;
                if new_day {ui.add_space(10.0);ui.horizontal(|ui|{ui.separator();ui.label(RichText::new(day).small().color(theme::TEXT_MUTED));ui.separator();});}
                if first_unread==Some(m.id) {ui.horizontal(|ui|{ui.separator();ui.label(RichText::new("Novas mensagens").small().strong().color(theme::ACCENT));ui.separator();});}
                let grouped=!new_day && index>0 && messages[index-1].author==m.author && m.time-messages[index-1].time<GROUP_WINDOW_SECS;
                let estimate=*app.workspace_ui.message_heights.get(&m.id).unwrap_or(&if grouped{44.0}else{76.0});
                let top=ui.cursor().min.y-origin;
                if first_visible.is_none() && top+estimate>=viewport.min.y {first_visible=Some(m.id);}
                let anchor=app.workspace_ui.history_anchor==Some(m.id);
                if !anchor && (top+estimate<viewport.min.y-250.0 || top>viewport.max.y+250.0) {ui.allocate_space(vec2(width,estimate));continue;}
                let start=ui.cursor().min.y;
                let response=message_row(app,ui,m,grouped);
                app.workspace_ui.message_heights.insert(m.id,(ui.cursor().min.y-start).max(1.0));
                if anchor {response.scroll_to_me_animation(Some(Align::TOP),egui::style::ScrollAnimation::none());app.workspace_ui.history_anchor=None;}
            }
            pending_rows(app,ui,channel);ui.add_space(10.0);
            let end=ui.allocate_response(vec2(1.0,1.0),egui::Sense::hover());
            if jump {end.scroll_to_me_animation(Some(Align::BOTTOM),egui::style::ScrollAnimation::none());}
        });
        let has_after=app.workspace_ui.history_has_after.get(&channel).copied().unwrap_or(false);
        let page_bottom=out.state.clamped_offset().y+out.inner_rect.height()>=out.content_size.y-32.0;
        app.chat_at_bottom=!has_after && page_bottom;
        if app.chat_at_bottom {
            app.workspace_ui.reading_positions.remove(&channel);
            let scope=format!("{}|{}|{channel}",app.local_identity,app.current_host());
            app.settings.read_positions.remove(&scope);
            if app.notification_focused && !has_blocking_surface(app,ui.ctx()) {app.mark_read(channel);}
        } else {
            if let Some(first)=first_visible {
                app.workspace_ui.reading_positions.insert(channel,first);
                let scope=format!("{}|{}|{channel}",app.local_identity,app.current_host());
                app.settings.read_positions.insert(scope,first);
            }
            let count=app.view.as_ref().and_then(|v|v.unread.get(&channel)).copied().unwrap_or(0);
            ui.horizontal(|ui| {
                if has_after && ui.add_enabled(!loading,egui::Button::new("Carregar posteriores")).clicked(){app.load_history_after(channel);}
                if ui.button(if count>0 {format!("{count} novas · Ir para o presente")}else{"Ir para o presente".into()}).clicked(){app.load_history(channel,false);app.scroll_to_bottom=3;}
                if count>0 && ui.small_button("Marcar como lidas").clicked(){app.mark_read(channel);}
            });
        }
        if has_after && page_bottom && !loading && !jump && !app.demo && !has_blocking_surface(app,ui.ctx()){app.load_history_after(channel);}
        if load_older {app.load_history(channel,true);}
    });
}

fn message_day(time: i64) -> String {
    Local
        .timestamp_opt(time, 0)
        .single()
        .map(|d| d.format("%d/%m/%Y").to_string())
        .unwrap_or_default()
}

fn has_blocking_surface(app: &LanvoxApp, ctx: &egui::Context) -> bool {
    !matches!(app.modal, crate::app::Modal::None)
        || app.workspace_ui.profile.is_some()
        || app.workspace_ui.group_open
        || app.workspace_ui.managing
        || app.workspace_ui.deleting.is_some()
        || app.workspace_ui.host_access_confirmation.is_some()
        || app.workspace_ui.poll_channel.is_some()
        || app.workspace_ui.forwarding.is_some()
        || app.workspace_ui.thread_from.is_some()
        || app.workspace_ui.gallery.is_some()
        || app.quick.open
        || ctx.memory(|m| m.top_modal_layer().is_some())
}

fn image_texture(
    ctx: &egui::Context,
    key: &str,
    bytes: &[u8],
    thumbnail: Option<u32>,
) -> Option<egui::TextureHandle> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let image = if let Some(side) = thumbnail {
        image.thumbnail(side, side)
    } else {
        image
    }
    .to_rgba8();
    Some(ctx.load_texture(
        key,
        egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        ),
        egui::TextureOptions::LINEAR,
    ))
}

fn custom_emoji_texture(
    app: &mut LanvoxApp,
    ctx: &egui::Context,
    id: u64,
) -> Option<egui::TextureHandle> {
    if let Some(texture) = app.workspace_ui.custom_emoji_textures.get(&id) {
        return Some(texture.clone());
    }
    let bytes = app
        .view
        .as_ref()?
        .workspace
        .community
        .emojis
        .iter()
        .find(|e| e.id == id)?
        .image
        .clone();
    let texture = image_texture(ctx, &format!("custom-emoji-{id}"), &bytes, Some(128))?;
    app.workspace_ui
        .custom_emoji_textures
        .insert(id, texture.clone());
    Some(texture)
}

const EMOJI: &[(&str, &str)] = &[
    ("😀", "sorriso feliz smile"),
    ("😃", "alegre smile"),
    ("😄", "sorrindo feliz"),
    ("😁", "dentes grin"),
    ("😆", "rindo laugh"),
    ("😂", "rindo lagrimas laugh"),
    ("🤣", "gargalhada laugh"),
    ("😊", "feliz carinho"),
    ("🙂", "sorriso"),
    ("😉", "piscada wink"),
    ("😍", "apaixonado amor love"),
    ("🥰", "amor carinho"),
    ("😘", "beijo kiss"),
    ("😎", "oculos cool"),
    ("🤔", "pensando think"),
    ("🤨", "duvida"),
    ("😐", "neutro"),
    ("😮", "surpresa wow"),
    ("😢", "triste choro sad"),
    ("😭", "chorando cry"),
    ("😡", "raiva angry"),
    ("🤯", "explosao mente"),
    ("🥳", "festa party"),
    ("😴", "dormindo sleep"),
    ("🤗", "abraco hug"),
    ("🤝", "acordo aperto maos"),
    ("👍", "positivo gostei sim like"),
    ("👎", "negativo nao dislike"),
    ("👏", "palmas clap"),
    ("🙌", "celebrar maos"),
    ("👋", "tchau ola wave"),
    ("👌", "ok"),
    ("✌️", "paz peace"),
    ("🤞", "sorte"),
    ("💪", "forca muscle"),
    ("🙏", "obrigado pray"),
    ("❤️", "coracao amor heart"),
    ("🧡", "laranja coracao"),
    ("💛", "amarelo coracao"),
    ("💚", "verde coracao"),
    ("💙", "azul coracao"),
    ("💜", "roxo coracao"),
    ("💔", "coracao partido"),
    ("💕", "amor coracoes"),
    ("🔥", "fogo fire"),
    ("✨", "brilho sparkles"),
    ("⭐", "estrela star"),
    ("🎉", "festa parabens party"),
    ("🎊", "confete"),
    ("🎂", "bolo aniversario"),
    ("🎁", "presente gift"),
    ("🏆", "trofeu win"),
    ("🎮", "jogo controle game"),
    ("🎯", "alvo target"),
    ("🎲", "dado dice"),
    ("🎵", "musica music"),
    ("🎧", "fone headphones"),
    ("🎤", "microfone mic"),
    ("📷", "camera foto"),
    ("💻", "computador pc"),
    ("📱", "celular phone"),
    ("🔗", "link"),
    ("👀", "olhos vendo eyes"),
    ("✅", "feito aprovado check"),
    ("❌", "erro nao cross"),
    ("⚠️", "aviso cuidado warning"),
    ("❓", "pergunta question"),
    ("💡", "ideia idea"),
    ("🚀", "foguete rocket"),
    ("🌈", "arco iris rainbow"),
    ("☀️", "sol sun"),
    ("🌙", "lua moon"),
    ("🐱", "gato cat"),
    ("🐶", "cachorro dog"),
    ("🐸", "sapo frog"),
    ("🐧", "pinguim penguin"),
    ("🦊", "raposa fox"),
    ("🍕", "pizza"),
    ("🍔", "hamburguer burger"),
    ("☕", "cafe coffee"),
    ("🍻", "cerveja beer"),
    ("🥤", "bebida drink"),
];
