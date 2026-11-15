//! Inline video players for attachments and direct video links in chat.
use super::{icons::Icon, theme, widgets};
use crate::app::LanvoxApp;
use crate::media::player::{PlayState, VideoPlayer, VideoSource};
use crate::model::*;
use egui::{pos2, vec2, Color32, Rect, RichText, Sense, Stroke};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv"];
const MAX_LINKS_PER_MESSAGE: usize = 3;
/// Players that stay loaded at once (the rest are dropped, oldest first).
const MAX_PLAYERS: usize = 4;

struct Entry {
    player: VideoPlayer,
    texture: Option<egui::TextureHandle>,
    seen: u64,
    started: u64,
}

type ThumbResult = Result<([usize; 2], Vec<u8>), String>;

enum Thumb {
    Loading(std::sync::mpsc::Receiver<ThumbResult>),
    Ready(egui::TextureHandle),
    Failed,
}

#[derive(Default)]
pub struct VideoUi {
    thumbs: HashMap<String, Thumb>,
    players: HashMap<String, Entry>,
    /// Attachments whose bytes were requested to start playing on arrival.
    pub wanted: HashSet<u64>,
    counter: u64,
    /// Key of the player currently shown fullscreen.
    fullscreen: Option<String>,
}

impl VideoUi {
    /// Whether a player for attachment `id` is loaded (so its cached file must be kept).
    pub fn is_active(&self, id: u64) -> bool {
        self.players.contains_key(&format!("a{id}"))
            || self.players.contains_key(&format!("audio{id}"))
    }
}

/// Large files cannot be previewed from memory: they are cached on disk and played from there.
fn is_large(a: &AttachmentInfo) -> bool {
    a.size > MAX_INLINE_ATTACHMENT_BYTES as u64
}

/// Shared by video and audio: plays a large attachment from its cached file, waits for the
/// download (with progress) or offers the click that starts it. Returns false when the caller
/// should draw its own idle state (poster / play button) and `start_request` is wired to it.
fn large_file_ready(app: &mut LanvoxApp, ui: &mut egui::Ui, a: &AttachmentInfo, key: &str) -> bool {
    if let Some(path) = app.workspace_ui.media_files.get(&a.id).cloned() {
        if path.exists() && app.videos.wanted.remove(&a.id) {
            start(app, key, VideoSource::File(path));
            ui.ctx().request_repaint();
            return true;
        }
    }
    if app.videos.wanted.contains(&a.id) {
        let progress = app.download_fraction(a.id).unwrap_or(0.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new(format!("Carregando {} · {:.0}%", a.name, progress * 100.0))
                    .color(theme::TEXT_MUTED),
            );
        });
        // The request may have been dropped (disconnect, stalled host): ask again.
        app.request_media_file(a);
        ui.ctx().request_repaint_after(Duration::from_millis(200));
        return true;
    }
    false
}

fn extension_is_video(name: &str) -> bool {
    let name = name.split(['?', '#']).next().unwrap_or("");
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn is_video_attachment(a: &AttachmentInfo) -> bool {
    a.media_type.starts_with("video/") || extension_is_video(&a.name)
}

pub fn is_audio_attachment(a: &AttachmentInfo) -> bool {
    a.media_type.starts_with("audio/")
        || a.name.rsplit_once('.').is_some_and(|(_, ext)| {
            ["mp3", "wav", "ogg", "m4a", "aac", "flac", "wma"]
                .contains(&ext.to_ascii_lowercase().as_str())
        })
}

pub fn audio_attachment(app: &mut LanvoxApp, ui: &mut egui::Ui, a: &AttachmentInfo) {
    let key = format!("audio{}", a.id);
    if app.videos.players.contains_key(&key) {
        draw_player(app, ui, &key, None);
        return;
    }
    if is_large(a) && large_file_ready(app, ui, a, &key) {
        return;
    }
    if app.videos.wanted.contains(&a.id) {
        if let Some((_, bytes)) = app.workspace_ui.downloads.get(&a.id) {
            let source = VideoSource::Bytes {
                name: a.name.clone(),
                data: bytes.clone(),
            };
            app.videos.wanted.remove(&a.id);
            start(app, &key, source);
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Carregando áudio: {}", a.name));
            });
        }
        return;
    }
    egui::Frame::new()
        .fill(theme::color(theme::SIDEBAR))
        .corner_radius(6)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Reproduzir áudio").clicked() {
                    if is_large(a) {
                        app.videos.wanted.insert(a.id);
                        app.request_media_file(a);
                    } else {
                        app.videos.wanted.insert(a.id);
                        if app.workspace_ui.preview_requests.insert(a.id) {
                            app.workspace(WorkspaceCommand::Download { attachment: a.id });
                        }
                    }
                }
                ui.add(egui::Label::new(&a.name).truncate());
                ui.label(
                    RichText::new(format!("{:.1} KiB", a.size as f64 / 1024.0))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        });
}

/// Direct http(s) links to video files found in a message.
pub fn video_links(text: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let word = word
            .trim_matches(|c: char| matches!(c, '<' | '>' | '(' | ')' | ',' | ';' | '"' | '\''));
        let lower = word.to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://")) || word.len() > 2048 {
            continue;
        }
        let path = word.split(['?', '#']).next().unwrap_or("");
        if (extension_is_video(path.trim_end_matches(['.', '!'])) || youtube_id(word).is_some())
            && !links.iter().any(|l| l == word)
        {
            links.push(word.to_string());
            if links.len() == MAX_LINKS_PER_MESSAGE {
                break;
            }
        }
    }
    links
}

/// The video id of a YouTube watch/short/embed/youtu.be link.
fn youtube_id(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let (host, path) = rest.split_once('/')?;
    let host = host.trim_start_matches("www.").trim_start_matches("m.");
    let id = match host {
        "youtu.be" => path.split(['?', '#', '/']).next()?.to_string(),
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            if let Some(q) = path.strip_prefix("watch").and_then(|p| p.split_once('?')) {
                q.1.split('#')
                    .next()?
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("v="))?
                    .to_string()
            } else {
                let mut parts = path.split(['?', '#']).next()?.split('/');
                match parts.next()? {
                    "shorts" | "embed" | "live" | "v" => parts.next()?.to_string(),
                    _ => return None,
                }
            }
        }
        _ => return None,
    };
    (id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(id)
}

/// Starts a background job producing a thumbnail, unless one exists for `key`.
fn begin_thumb(app: &mut LanvoxApp, key: &str, job: impl FnOnce() -> ThumbResult + Send + 'static) {
    if app.videos.thumbs.contains_key(key) {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("video-thumb".into())
        .spawn(move || {
            let _ = tx.send(job());
        });
    app.videos.thumbs.insert(
        key.to_string(),
        if spawned.is_ok() {
            Thumb::Loading(rx)
        } else {
            Thumb::Failed
        },
    );
}

fn poll_thumb(app: &mut LanvoxApp, ctx: &egui::Context, key: &str) -> Option<egui::TextureHandle> {
    let entry = app.videos.thumbs.get_mut(key)?;
    match entry {
        Thumb::Ready(t) => Some(t.clone()),
        Thumb::Failed => None,
        Thumb::Loading(rx) => match rx.try_recv() {
            Ok(Ok((size, rgba))) => {
                let t = ctx.load_texture(
                    format!("thumb-{key}"),
                    egui::ColorImage::from_rgba_unmultiplied(size, &rgba),
                    egui::TextureOptions::LINEAR,
                );
                *entry = Thumb::Ready(t.clone());
                Some(t)
            }
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                *entry = Thumb::Failed;
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(150));
                None
            }
        },
    }
}

fn frame_job(source: VideoSource) -> impl FnOnce() -> ThumbResult + Send + 'static {
    move || crate::media::player::thumbnail(source).map(|f| (f.size, f.rgba))
}

fn youtube_job(id: String) -> impl FnOnce() -> ThumbResult + Send + 'static {
    move || {
        let bytes =
            crate::media::player::download(&format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))?;
        let img = image::load_from_memory(&bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        Ok((
            [img.width() as usize, img.height() as usize],
            img.into_raw(),
        ))
    }
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or(url)
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn play_triangle(painter: &egui::Painter, rect: Rect, color: Color32) {
    let r = rect.shrink(rect.width() * 0.28);
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(r.left() + r.width() * 0.1, r.top()),
            pos2(r.right(), r.center().y),
            pos2(r.left() + r.width() * 0.1, r.bottom()),
        ],
        color,
        Stroke::NONE,
    ));
}

fn transport_button(ui: &mut egui::Ui, playing: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, theme::color(theme::HOVER));
    }
    if playing {
        let r = rect.shrink(8.0);
        let w = r.width() * 0.32;
        for x in [r.left(), r.right() - w] {
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height())),
                1.0,
                theme::TEXT_BRIGHT,
            );
        }
    } else {
        play_triangle(ui.painter(), rect, theme::TEXT_BRIGHT);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
    resp.on_hover_text(tip)
}

/// A dark 16:9 box with a play button; returns true when clicked.
fn poster(
    ui: &mut egui::Ui,
    thumb: Option<&egui::TextureHandle>,
    title: &str,
    subtitle: &str,
) -> bool {
    let width = ui.available_width().clamp(160.0, 420.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, width * 9.0 / 16.0), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, Color32::from_rgb(12, 13, 16));
    painter.rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, theme::TEXT_MUTED.gamma_multiply(0.35)),
        egui::StrokeKind::Inside,
    );
    if let Some(t) = thumb {
        // Fit inside the box, keeping the aspect ratio.
        let s = t.size_vec2();
        let scale = (rect.width() / s.x).min(rect.height() / s.y);
        let fitted = Rect::from_center_size(rect.center(), s * scale);
        egui::Image::new((t.id(), fitted.size()))
            .corner_radius(6)
            .paint_at(ui, fitted);
        ui.painter()
            .rect_filled(rect, 6.0, Color32::from_black_alpha(70));
    }
    let painter = ui.painter();
    let button = Rect::from_center_size(rect.center(), vec2(52.0, 52.0));
    painter.circle_filled(
        button.center(),
        26.0,
        if hovered {
            theme::ACCENT
        } else {
            theme::ACCENT.gamma_multiply(0.8)
        },
    );
    play_triangle(painter, button, Color32::WHITE);
    let font = egui::FontId::proportional(12.0);
    painter.text(
        pos2(rect.left() + 10.0, rect.bottom() - 8.0),
        egui::Align2::LEFT_BOTTOM,
        title,
        font.clone(),
        theme::TEXT,
    );
    painter.text(
        pos2(rect.right() - 10.0, rect.top() + 8.0),
        egui::Align2::RIGHT_TOP,
        subtitle,
        font,
        theme::TEXT_MUTED,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn start(app: &mut LanvoxApp, key: &str, source: VideoSource) {
    let v = &mut app.videos;
    // Only one video plays at a time; finished or failed ones are released.
    for e in v.players.values() {
        e.player.pause();
    }
    v.players
        .retain(|_, e| !matches!(e.player.state(), PlayState::Ended | PlayState::Failed));
    while v.players.len() >= MAX_PLAYERS {
        let Some(oldest) = v
            .players
            .iter()
            .min_by_key(|(_, e)| e.started)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        v.players.remove(&oldest);
    }
    v.counter += 1;
    v.players.insert(
        key.to_string(),
        Entry {
            player: VideoPlayer::start(source),
            texture: None,
            seen: 0,
            started: v.counter,
        },
    );
}

/// Player (or poster) for a video file attached to a message.
pub fn attachment(app: &mut LanvoxApp, ui: &mut egui::Ui, a: &AttachmentInfo) {
    let key = format!("a{}", a.id);
    if app.videos.players.contains_key(&key) {
        return draw_player(app, ui, &key, None);
    }
    if is_large(a) {
        if large_file_ready(app, ui, a, &key) {
            return;
        }
        let size = format!("{:.1} MiB", a.size as f64 / (1024.0 * 1024.0));
        if poster(ui, None, &a.name, &size) {
            app.videos.wanted.insert(a.id);
            app.request_media_file(a);
        }
        return;
    }
    if app.videos.wanted.contains(&a.id) {
        let bytes = app
            .workspace_ui
            .downloads
            .get(&a.id)
            .map(|(_, b)| b.clone());
        if let Some(data) = bytes {
            app.videos.wanted.remove(&a.id);
            start(
                app,
                &key,
                VideoSource::Bytes {
                    name: a.name.clone(),
                    data,
                },
            );
            ui.ctx().request_repaint();
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("Carregando {}…", a.name)).color(theme::TEXT_MUTED));
            });
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        return;
    }
    let size = format!("{:.1} MiB", a.size as f64 / (1024.0 * 1024.0));
    let thumb_key = format!("t{}", a.id);
    if !app.videos.thumbs.contains_key(&thumb_key) {
        let bytes = app
            .workspace_ui
            .downloads
            .get(&a.id)
            .map(|(_, b)| b.clone());
        if let Some(data) = bytes {
            begin_thumb(
                app,
                &thumb_key,
                frame_job(VideoSource::Bytes {
                    name: a.name.clone(),
                    data,
                }),
            );
        } else if a.size <= MAX_INLINE_ATTACHMENT_BYTES as u64
            && app.workspace_ui.preview_requests.insert(a.id)
        {
            app.workspace(WorkspaceCommand::Download { attachment: a.id });
        }
    }
    let thumb = poll_thumb(app, ui.ctx(), &thumb_key);
    if poster(ui, thumb.as_ref(), &a.name, &size) {
        app.videos.wanted.insert(a.id);
        if !app.workspace_ui.downloads.contains_key(&a.id)
            && app.workspace_ui.preview_requests.insert(a.id)
        {
            app.workspace(WorkspaceCommand::Download { attachment: a.id });
        }
    }
}

/// Player (or poster) for a direct video link. Nothing is fetched until clicked, unless the
/// user enabled remote link previews.
pub fn link(app: &mut LanvoxApp, ui: &mut egui::Ui, url: &str) {
    let key = format!("u{url}");
    if app.videos.players.contains_key(&key) {
        return draw_player(app, ui, &key, Some(url));
    }
    let thumb_key = format!("t{url}");
    let youtube = youtube_id(url);
    // Thumbnails come from the linked site, which would learn this device's IP address.
    if app.settings.remote_link_previews {
        match &youtube {
            Some(id) => begin_thumb(app, &thumb_key, youtube_job(id.clone())),
            None => begin_thumb(
                app,
                &thumb_key,
                frame_job(VideoSource::Url(url.to_string())),
            ),
        }
    }
    let thumb = poll_thumb(app, ui.ctx(), &thumb_key);
    let title = match &youtube {
        Some(_) => "YouTube · abrir no navegador",
        None => url
            .rsplit('/')
            .next()
            .unwrap_or(url)
            .split(['?', '#'])
            .next()
            .unwrap_or(url),
    };
    if poster(ui, thumb.as_ref(), title, host_of(url)) {
        if youtube.is_some() {
            widgets::open_url(url);
        } else {
            start(app, &key, VideoSource::Url(url.to_string()));
        }
    }
}

#[derive(Default)]
struct Actions {
    close: bool,
    toggle_fullscreen: bool,
}

/// Four corner brackets: outwards to enter fullscreen, inwards to leave it.
fn fullscreen_icon(painter: &egui::Painter, rect: Rect, color: Color32, exit: bool) {
    let r = rect.shrink(8.0);
    let arm = r.width() * 0.4;
    let stroke = Stroke::new(1.6, color);
    for (corner, dx, dy) in [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.left_bottom(), 1.0, -1.0),
        (r.right_bottom(), -1.0, -1.0),
    ] {
        // "Exit" brackets point at the centre from inside the box.
        let c = if exit {
            corner + vec2(dx * arm, dy * arm)
        } else {
            corner
        };
        let (dx, dy) = if exit { (-dx, -dy) } else { (dx, dy) };
        painter.line_segment([c, c + vec2(dx * arm, 0.0)], stroke);
        painter.line_segment([c, c + vec2(0.0, dy * arm)], stroke);
    }
}

fn fullscreen_button(ui: &mut egui::Ui, exit: bool) -> egui::Response {
    let tip = if exit {
        "Sair da tela cheia (Esc)"
    } else {
        "Tela cheia"
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, theme::color(theme::HOVER));
    }
    fullscreen_icon(ui.painter(), rect, theme::TEXT_BRIGHT, exit);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
    resp.on_hover_text(tip)
}

/// Transport row: play/pause, time, seek bar, volume, fullscreen, close.
fn controls(
    ui: &mut egui::Ui,
    player: &VideoPlayer,
    state: PlayState,
    full: bool,
    act: &mut Actions,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let playing = matches!(state, PlayState::Playing | PlayState::Loading);
        if transport_button(ui, playing, if playing { "Pausar" } else { "Reproduzir" }).clicked() {
            player.toggle();
        }
        let duration = player.duration();
        let mut secs = player.position().as_secs_f64();
        ui.label(
            RichText::new(format!(
                "{} / {}",
                clock(player.position()),
                clock(duration)
            ))
            .size(11.0)
            .color(if full { theme::TEXT } else { theme::TEXT_MUTED }),
        );
        let vol_w = if full || ui.available_width() > 330.0 {
            70.0
        } else {
            44.0
        };
        let buttons = if full { 2.0 } else { 3.0 };
        let reserved = 28.0 * buttons + vol_w + 5.0 * 4.0 + 12.0;
        let max_seek = if full { 4000.0 } else { 220.0 };
        let seek_w = (ui.available_width() - reserved).clamp(40.0, max_seek);
        ui.spacing_mut().slider_width = seek_w;
        let slider = ui
            .add_enabled(
                duration > Duration::ZERO,
                egui::Slider::new(&mut secs, 0.0..=duration.as_secs_f64().max(0.001))
                    .show_value(false),
            )
            .on_hover_text("Buscar");
        if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
            player.seek(Duration::from_secs_f64(secs.max(0.0)));
        }

        // Volume: mute toggle (keeps the previous level) + slider.
        let volume = player.volume();
        let muted = player.muted() || volume < 0.005;
        if widgets::icon_button(
            ui,
            Icon::Speaker,
            28.0,
            if muted { theme::RED } else { theme::TEXT },
            if muted { "Ativar som" } else { "Silenciar" },
        )
        .clicked()
        {
            if muted {
                player.set_muted(false);
                if volume < 0.005 {
                    player.set_volume(0.5);
                }
            } else {
                player.set_muted(true);
            }
        }
        let mut shown = if muted { 0.0 } else { volume };
        ui.spacing_mut().slider_width = vol_w;
        let vs = ui
            .add(egui::Slider::new(&mut shown, 0.0..=1.0).show_value(false))
            .on_hover_text(format!("Volume: {}%", (shown * 100.0).round() as u32));
        if vs.changed() {
            player.set_volume(shown);
            player.set_muted(false);
        }

        if !player.audio_only() && fullscreen_button(ui, full).clicked() {
            act.toggle_fullscreen = true;
        }
        if !full
            && widgets::icon_button(ui, Icon::Close, 28.0, theme::TEXT, "Fechar vídeo").clicked()
        {
            act.close = true;
        }
    });
}

fn upload_frame(ui: &egui::Ui, entry: &mut Entry, key: &str) {
    if let Some(frame) = entry.player.frame_after(entry.seen) {
        let image = egui::ColorImage::from_rgba_unmultiplied(frame.size, &frame.rgba);
        match &mut entry.texture {
            Some(t) => t.set(image, egui::TextureOptions::LINEAR),
            None => {
                entry.texture = Some(ui.ctx().load_texture(
                    format!("video-{key}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ))
            }
        }
        entry.seen = frame.seq;
    }
}

/// Full-window overlay with the video scaled to fit, plus controls.
fn fullscreen_overlay(
    ctx: &egui::Context,
    entry: &mut Entry,
    key: &str,
    state: PlayState,
    act: &mut Actions,
) {
    let screen = ctx.content_rect();
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        act.toggle_fullscreen = true;
    }
    egui::Area::new(egui::Id::unique(("video-fullscreen", key)))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
            match &entry.texture {
                Some(t) => {
                    let s = t.size_vec2();
                    let scale = (rect.width() / s.x.max(1.0)).min(rect.height() / s.y.max(1.0));
                    let fitted = Rect::from_center_size(rect.center(), s * scale);
                    egui::Image::new((t.id(), fitted.size())).paint_at(ui, fitted);
                }
                None => {
                    ui.put(
                        Rect::from_center_size(rect.center(), vec2(32.0, 32.0)),
                        egui::Spinner::new(),
                    );
                }
            }
            if resp.double_clicked() {
                act.toggle_fullscreen = true;
            } else if resp.clicked() {
                entry.player.toggle();
            }
            let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 48.0), rect.max);
            ui.painter()
                .rect_filled(bar, 0.0, Color32::from_black_alpha(170));
            let mut bar_ui =
                ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(16.0, 10.0))));
            controls(&mut bar_ui, &entry.player, state, true, act);
        });
}

fn draw_player(app: &mut LanvoxApp, ui: &mut egui::Ui, key: &str, url: Option<&str>) {
    let mut act = Actions::default();
    let full = app.videos.fullscreen.as_deref() == Some(key);
    {
        let Some(entry) = app.videos.players.get_mut(key) else {
            return;
        };
        entry.player.touch();
        let state = entry.player.state();
        if state == PlayState::Failed {
            ui.label(RichText::new(entry.player.error()).color(theme::YELLOW));
            ui.horizontal(|ui| {
                if let Some(url) = url {
                    widgets::link(ui, "Abrir no navegador", url);
                }
                if ui.small_button("Fechar").clicked() {
                    act.close = true;
                }
            });
        } else if key.starts_with("audio") || entry.player.audio_only() {
            egui::Frame::new()
                .fill(theme::color(theme::SIDEBAR))
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_max_width(480.0);
                    ui.label(RichText::new("Áudio").small().color(theme::TEXT_MUTED));
                    controls(ui, &entry.player, state, false, &mut act);
                });
            if matches!(state, PlayState::Playing | PlayState::Loading) {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        } else {
            upload_frame(ui, entry, key);
            let max_w = ui.available_width().clamp(160.0, 480.0);
            let video_size = match &entry.texture {
                Some(t) => {
                    let s = t.size_vec2();
                    vec2(max_w, max_w * s.y / s.x.max(1.0))
                }
                None => vec2(max_w, max_w * 9.0 / 16.0),
            };
            let (rect, resp) = ui.allocate_exact_size(video_size, Sense::click());
            if full {
                ui.painter()
                    .rect_filled(rect, 6.0, Color32::from_rgb(12, 13, 16));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Em tela cheia",
                    egui::FontId::proportional(12.0),
                    theme::TEXT_MUTED,
                );
            } else {
                match &entry.texture {
                    Some(t) => {
                        egui::Image::new((t.id(), video_size))
                            .corner_radius(6)
                            .paint_at(ui, rect);
                    }
                    None => {
                        ui.painter()
                            .rect_filled(rect, 6.0, Color32::from_rgb(12, 13, 16));
                        ui.put(
                            Rect::from_center_size(rect.center(), vec2(24.0, 24.0)),
                            egui::Spinner::new(),
                        );
                    }
                }
                if resp.double_clicked() {
                    act.toggle_fullscreen = true;
                } else if resp.clicked() {
                    entry.player.toggle();
                }
            }
            if full {
                // The overlay owns the controls while fullscreen.
                fullscreen_overlay(ui.ctx(), entry, key, state, &mut act);
            } else {
                controls(ui, &entry.player, state, false, &mut act);
            }
            if matches!(state, PlayState::Playing | PlayState::Loading) {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
    }
    if act.toggle_fullscreen {
        app.videos.fullscreen = if full { None } else { Some(key.to_string()) };
    }
    if act.close {
        if full {
            app.videos.fullscreen = None;
        }
        app.videos.players.remove(key);
    }
}

#[cfg(test)]
mod hit_tests {
    use egui::*;

    #[derive(Clone, Copy)]
    enum Row {
        None,
        After,
        Before,
    }

    /// Mirrors a message row: content in a Frame plus a row-wide click sense.
    fn run(ctx: &Context, events: Vec<Event>, row: Row) -> bool {
        let mut clicked = false;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let id = Id::unique("row");
            let known: Option<Rect> = ui.data(|d| d.get_temp(id));
            if let (Row::Before, Some(rect)) = (row, known) {
                ui.interact(rect, id, Sense::click());
            }
            let frame = Frame::new()
                .show(ui, |ui| {
                    let (_, resp) = ui.allocate_exact_size(vec2(300.0, 150.0), Sense::click());
                    clicked |= resp.clicked();
                })
                .response;
            ui.data_mut(|d| d.insert_temp(id, frame.rect));
            if let Row::After = row {
                let _ = frame.interact(Sense::click());
            }
        });
        out.textures_delta.clear();
        clicked
    }

    fn click(row: Row) -> bool {
        let ctx = Context::default();
        let p = pos2(60.0, 60.0);
        let button = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run(&ctx, vec![], row); // first frame learns the row's rect
        run(&ctx, vec![Event::PointerMoved(p)], row);
        run(&ctx, vec![button(true)], row);
        run(&ctx, vec![button(false)], row)
    }

    /// A message row's own click sense must not swallow the video poster / buttons inside it.
    #[test]
    fn row_click_sense_registered_before_content_keeps_inner_clicks() {
        assert!(click(Row::None));
        assert!(click(Row::Before));
        assert!(
            !click(Row::After),
            "egui no longer lets a later row sense steal clicks"
        );
    }
}
