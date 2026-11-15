//! The app controller: owns the connection, the media sessions and the
//! interface state, and turns button presses into actions.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver};
use parking_lot::Mutex;

use crate::media::audio::{AudioEngine, AudioShared, Mixer};
use crate::media::camera::CameraSession;
use crate::media::screen::{ScreenShare, ShareSource};
use crate::media::video::{Decoder, PictureStore, StreamKey};
use crate::model::*;
use crate::net::client::{Client, NetEvent};
use crate::net::discovery::Discovery;
use crate::net::server::{self, ServerHandle};
use crate::protocol::ClientMsg;
use crate::settings::Settings;
use crate::state::ServerView;
use crate::ui;
#[path = "app_workspace.rs"]
mod workspace;
pub use workspace::{FileTask, WorkspaceUi};
#[path = "app_platform.rs"]
mod platform;
#[path = "app_sessions.rs"]
mod sessions;
pub use sessions::BackgroundHost;
mod connection;
mod server_msgs;
mod streaming;
mod voice;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    Account,
    Voice,
    Video,
    Streaming,
    Appearance,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    None,
    SharePicker,
    Settings(SettingsTab),
    CreateChannel {
        kind: ChannelKind,
        name: String,
        /// Create it directly inside this category.
        category: Option<CategoryId>,
    },
    CreateCategory {
        name: String,
    },
    RenameCategory {
        category: CategoryId,
        name: String,
    },
    DeleteCategory {
        category: CategoryId,
    },
    CreateGuild {
        name: String,
    },
    JoinInvite {
        invite: String,
    },
    ConnectHost {
        address: String,
    },
}

/// The screen-share picker's state.
#[derive(Default)]
pub struct PickerState {
    pub loading: Option<Arc<Mutex<Option<Vec<ShareSource>>>>>,
    pub sources: Vec<ShareSource>,
    pub thumbs: HashMap<usize, egui::TextureHandle>,
    pub windows_tab: bool,
    pub selected: Option<usize>,
}

pub struct ScreenshotJob {
    pub path: PathBuf,
    pub frames: u32,
    pub requested: bool,
    pub done: Arc<AtomicBool>,
}

/// Private recording/preview, scoped to the host and conversation that started it.
pub struct VoiceNote {
    pub host: String,
    pub channel: ChannelId,
    pub recorder: Option<crate::media::audio::voice_note::Recorder>,
    pub bytes: Option<Vec<u8>>,
    pub preview: Option<crate::media::player::VideoPlayer>,
}

pub struct LanvoxApp {
    pub settings: Settings,
    pub discovery: Option<Discovery>,
    pub hosted: Option<ServerHandle>,
    pub client: Option<Client>,
    pub view: Option<ServerView>,
    pub selected: Option<ChannelId>,
    /// The server shown in the channel list.
    pub guild: Option<GuildId>,
    /// Showing friends and calls instead of a server.
    pub home: bool,
    pub friend_input: String,
    pub group_pick: HashSet<UserId>,
    pub dismissed_calls: HashSet<ChannelId>,
    pub compose: HashMap<ChannelId, String>,
    pub audio_shared: Arc<AudioShared>,
    pub mixer: Arc<Mixer>,
    pub audio: Option<AudioEngine>,
    /// Set while a voice join is sent but the server has not yet confirmed it.
    pub voice_join: Option<(Instant, ChannelId)>,
    pub pictures: PictureStore,
    pub decoder: Decoder,
    pub textures: HashMap<StreamKey, (egui::TextureHandle, u64)>,
    /// A shared screen's pointer image per sharer, with the shape version it was built from.
    pub cursor_textures: HashMap<UserId, (u64, egui::TextureHandle)>,
    pub share: Option<ScreenShare>,
    pub camera: Option<CameraSession>,
    pub camera_preview: Option<CameraSession>,
    pub watching: HashSet<StreamKey>,
    /// Streams the user stopped watching on purpose; they are not auto-started again
    /// until the sharer stops and starts a new stream.
    pub unwatched: HashSet<StreamKey>,
    pub focused: Option<StreamKey>,
    pub fullscreen: bool,
    pub popout: Option<StreamKey>,
    pub modal: Modal,
    pub picker: PickerState,
    pub notice: Option<(String, Instant)>,
    pub screenshot: Option<ScreenshotJob>,
    pub demo: bool,
    pub last_typing_sent: Instant,
    pub host_input: String,
    pub join_input: String,
    pub connecting: bool,
    pub host_error: Option<String>,
    pub join_error: Option<String>,
    pub local_host_busy: bool,
    pub show_members: bool,
    /// Side chat panel inside a voice-channel call.
    pub call_chat_open: bool,
    pub device_cache: Option<(Vec<String>, Vec<String>, Vec<String>)>,
    pub scroll_to_bottom: u8,
    pub connection_pending: Option<Receiver<Result<Client, String>>>,
    pub reconnect_address: Option<String>,
    pub reconnect_at: Option<Instant>,
    pub reconnect_attempt: u32,
    pub connection_status: String,
    pub new_host_pending: bool,
    pub draft_saved_at: Instant,
    pub search_input: String,
    pub friend_filter: String,
    pub chat_at_bottom: bool,
    pub workspace_ui: WorkspaceUi,
    pub voice_note: Option<VoiceNote>,
    pub attachment_textures: HashMap<u64, egui::TextureHandle>,
    pub videos: crate::ui::video::VideoUi,
    pub avatar_textures: HashMap<UserId, (u64, egui::TextureHandle)>,
    pub file_pending: Option<Receiver<FileTask>>,
    pub notification_focused: bool,
    pub desktop_events: std::sync::mpsc::Receiver<crate::notifications::DesktopAction>,
    pub local_identity: String,
    pub sfx_watch: crate::sfx::Watcher,
    pub quick: ui::quick_switcher::QuickSwitcher,
    pub pending_message: Option<(String, ChannelId, MessageId)>,
    pub background_hosts: Vec<BackgroundHost>,
}

impl LanvoxApp {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<PathBuf>) -> Self {
        ui::theme::apply(&cc.egui_ctx);
        crate::media_cache::reset();
        let settings = if screenshot.is_some() {
            Settings::default()
        } else {
            Settings::load()
        };
        if screenshot.is_none() {
            let _ = crate::settings::identity();
        }
        // Screenshots never reach the network; otherwise verify or fetch Cisco's codec.
        crate::media::openh264_runtime::configure(screenshot.is_none() && settings.h264_codec);
        let local_identity = crate::notifications::identity_fingerprint().unwrap_or_default();
        let pictures = PictureStore::default();
        let decoder = Decoder::start(pictures.clone(), cc.egui_ctx.clone());
        let audio_shared = Arc::new(AudioShared::default());
        audio_shared.set_mode(settings.input_mode);
        audio_shared
            .processor
            .configure(settings.echo_cancellation, settings.noise_suppression);
        audio_shared.processor.set_rnnoise(settings.rnnoise);
        audio_shared.processor.set_delay_ms(settings.echo_delay_ms);
        audio_shared.set_threshold_db(settings.threshold_db);
        audio_shared.set_smart_margin_db(settings.smart_margin_db);
        audio_shared.set_input_gain(settings.input_gain);
        audio_shared
            .ptt_vk
            .store(settings.ptt_key.vk(), Ordering::Relaxed);
        let mixer = Arc::new(Mixer::new());
        mixer.set_output_volume(settings.output_volume);
        let demo = screenshot.is_some();
        let mut app = Self {
            host_input: if settings.host_name.is_empty() {
                format!("{}'s server", settings.name)
            } else {
                settings.host_name.clone()
            },
            join_input: settings.last_address.clone(),
            settings,
            discovery: None,
            hosted: None,
            client: None,
            view: None,
            selected: None,
            guild: None,
            home: false,
            friend_input: String::new(),
            group_pick: HashSet::new(),
            dismissed_calls: HashSet::new(),
            compose: HashMap::new(),
            audio_shared,
            mixer,
            audio: None,
            voice_join: None,
            pictures,
            decoder,
            textures: HashMap::new(),
            cursor_textures: HashMap::new(),
            share: None,
            camera: None,
            camera_preview: None,
            watching: HashSet::new(),
            unwatched: HashSet::new(),
            focused: None,
            fullscreen: false,
            popout: None,
            modal: Modal::None,
            picker: PickerState::default(),
            notice: None,
            screenshot: screenshot.map(|path| ScreenshotJob {
                path,
                frames: 0,
                requested: false,
                done: Arc::new(AtomicBool::new(false)),
            }),
            demo,
            last_typing_sent: Instant::now() - Duration::from_secs(10),
            connecting: false,
            host_error: None,
            join_error: None,
            local_host_busy: false,
            show_members: true,
            call_chat_open: false,
            device_cache: None,
            scroll_to_bottom: 3,
            connection_pending: None,
            reconnect_address: None,
            reconnect_at: None,
            reconnect_attempt: 0,
            connection_status: String::new(),
            new_host_pending: false,
            draft_saved_at: Instant::now(),
            search_input: String::new(),
            friend_filter: String::new(),
            chat_at_bottom: true,
            workspace_ui: WorkspaceUi::default(),
            voice_note: None,
            attachment_textures: HashMap::new(),
            videos: Default::default(),
            avatar_textures: HashMap::new(),
            file_pending: None,
            notification_focused: true,
            desktop_events: if demo {
                std::sync::mpsc::channel().1
            } else {
                crate::notifications::start()
            },
            local_identity,
            sfx_watch: Default::default(),
            quick: Default::default(),
            pending_message: None,
            background_hosts: Vec::new(),
        };
        crate::notifications::set_hotkeys(app.settings.mute_hotkey, app.settings.deafen_hotkey);
        if demo {
            crate::demo::fill(&mut app);
        } else {
            app.discovery = Some(Discovery::start(cc.egui_ctx.clone()));
            if let Some(invite) = std::env::args().find(|arg| crate::branding::is_link(arg)) {
                app.stage_invite(invite);
            }
        }
        app
    }

    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    pub fn connected(&self) -> bool {
        self.view.is_some()
    }

    pub fn me(&self) -> UserId {
        self.view.as_ref().map(|v| v.me).unwrap_or(0)
    }

    pub fn send(&self, msg: ClientMsg) {
        if let Some(c) = &self.client {
            c.send(msg);
        }
    }

    pub fn send_file(&self, msg: ClientMsg) {
        if let Some(c) = &self.client {
            c.send_file(msg);
        }
    }

    // ----- connection -----

    pub fn tick(&mut self, ctx: &egui::Context) {
        self.notification_focused = ctx.input(|i| i.focused);
        self.auto_watch_streams();
        let actions: Vec<_> = self.desktop_events.try_iter().collect();
        for action in actions {
            use crate::notifications::DesktopAction;
            match action {
                DesktopAction::Open => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                DesktopAction::OpenMessage {
                    host,
                    channel,
                    message,
                } => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.open_notification(ctx, host, channel, message);
                }
                DesktopAction::OpenInvite(invite) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.stage_invite(invite);
                }
                DesktopAction::ToggleMute => self.toggle_mute(),
                DesktopAction::ToggleDeafen => self.toggle_deafen(),
                DesktopAction::Settings => {
                    self.modal = Modal::Settings(SettingsTab::Account);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                }
                DesktopAction::Exit => {
                    self.save_drafts();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if !self.demo {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::K)) {
            self.quick.toggle();
        }
        // Configurable shortcuts while the window has focus (global ones arrive as desktop actions).
        let (mute_key, deafen_key) = (self.settings.mute_hotkey, self.settings.deafen_hotkey);
        if !matches!(self.modal, Modal::Settings(_)) {
            if ctx.input(|i| mute_key.pressed(i)) {
                self.toggle_mute();
            }
            if ctx.input(|i| deafen_key.pressed(i)) {
                self.toggle_deafen();
            }
        }
        // Join/leave sounds for the call I am in, and a repeating ring for incoming calls.
        if let Some(view) = &self.view {
            let channel = view.my_voice().channel;
            let members = channel.map(|c| view.in_voice(c)).unwrap_or_default();
            if let Some(sound) = self.sfx_watch.members(view.me, channel, &members) {
                if !self.settings.do_not_disturb {
                    crate::sfx::play(sound, &self.settings);
                }
            }
        }
        let ringing = !self.settings.do_not_disturb && !self.ringing().is_empty();
        if self.sfx_watch.ring(ringing) {
            crate::sfx::play(crate::sfx::Sfx::Ring, &self.settings);
        }
        self.poll_file_task(ctx);
        self.poll_connection(ctx);
        self.poll_network();
        self.poll_background_hosts(ctx);
        let discard_voice_note = self.voice_note.as_ref().is_some_and(|note| {
            note.host != self.current_host()
                || self.selected != Some(note.channel)
                || !self.client.as_ref().is_some_and(|client| client.is_alive())
                || (self
                    .view
                    .as_ref()
                    .and_then(|view| view.channel(note.channel))
                    .is_some_and(|channel| channel.kind == ChannelKind::Voice)
                    && !self.call_chat_open)
                || (note.recorder.is_some()
                    && (self.audio_shared.muted.load(Ordering::Relaxed)
                        || self.audio_shared.deafened.load(Ordering::Relaxed)))
        });
        if discard_voice_note {
            self.voice_note = None;
            self.notify("Gravação de voz descartada ao mudar de conversa, desconectar ou silenciar o microfone.");
        }
        if let Some((host, channel, message)) = self.pending_message.clone() {
            if self
                .client
                .as_ref()
                .is_some_and(|client| crate::settings::host_address(&client.address) == host)
                && !self.connecting
                && self.view.is_some()
            {
                if self
                    .view
                    .as_ref()
                    .is_some_and(|view| view.channel(channel).is_some())
                {
                    self.open_notification(ctx, host, channel, message);
                } else {
                    self.pending_message = None;
                    self.notify("A conversa deste link não está disponível para esta identidade.");
                }
            }
        }
        let audio_error = self
            .share
            .as_ref()
            .and_then(|share| share.stats.lock().audio_error.clone());
        if audio_error.is_some() && audio_error != self.workspace_ui.last_audio_error {
            if let Some(error) = &audio_error {
                self.notify(format!("Tela sem áudio compartilhado: {error}"));
            }
        }
        self.workspace_ui.last_audio_error = audio_error;
        if !self.demo {
            self.tick_pending();
        }
        if !self.demo && self.draft_saved_at.elapsed() >= Duration::from_secs(2) {
            self.save_drafts();
        }
        // A shared window that closed ends the share.
        let ended = self
            .share
            .as_ref()
            .and_then(|s| s.stats.lock().ended.clone());
        if let Some(reason) = ended {
            self.stop_share();
            self.notify(format!("Sua transmissão terminou: {reason}"));
        }
        let cam_err = self.camera.as_ref().and_then(|c| c.error.lock().take());
        if let Some(err) = cam_err {
            self.notify(err);
            self.camera = None;
            self.send(ClientMsg::StopStream(StreamKind::Camera));
        }
        let preview_err = self
            .camera_preview
            .as_ref()
            .and_then(|c| c.error.lock().take());
        if let Some(err) = preview_err {
            self.notify(err);
            self.camera_preview = None;
        }
        let audio_err = self.audio_shared.error.lock().take();
        if let Some(err) = audio_err {
            self.notify(err);
        }
        // Push-to-talk key while this window has focus (the background check is in the audio thread).
        let ptt_down = self
            .settings
            .ptt_key
            .egui_key()
            .map(|k| ctx.input(|i| i.key_down(k)))
            .unwrap_or(false)
            && !ctx.egui_wants_keyboard_input();
        self.audio_shared
            .ptt_window
            .store(ptt_down, Ordering::Relaxed);
        // Keep the speaking ring and meters lively while in a call.
        if self.audio.is_some() || self.share.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.client.is_some() && !self.client.as_ref().map(|c| c.is_alive()).unwrap_or(true) {
            self.connection_lost("O servidor encerrou a conexão.".into());
        }
    }
}

impl eframe::App for LanvoxApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.voice_note = None;
        if !self.demo {
            self.save_drafts();
            crate::notifications::shutdown();
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tick(&ctx);
        let scope = self.notification_scope();
        ui.push_id(scope, |ui| ui::root(self, ui));
        self.screenshot_step(&ctx);
    }
}

impl LanvoxApp {
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.screenshot else {
            return;
        };
        job.frames += 1;
        ctx.request_repaint();
        if job.done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if job.frames >= 120 && !job.requested {
            job.requested = true;
            let path = job.path.clone();
            let done = job.done.clone();
            let ctx2 = ctx.clone();
            ctx.request_screenshot(move |image| {
                let [w, h] = image.size;
                let mut bytes = Vec::with_capacity(w * h * 4);
                for px in &image.pixels {
                    bytes.extend_from_slice(&px.to_array());
                }
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                    if let Err(e) = img.save(&path) {
                        eprintln!("could not save screenshot: {e}");
                    }
                }
                done.store(true, Ordering::SeqCst);
                ctx2.request_repaint();
            });
        }
        // Safety net: never hang in screenshot mode.
        if job.frames > 600 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod voice_join_tests {
    use super::*;

    #[test]
    fn switching_into_a_call_is_not_torn_down_by_the_left_broadcast() {
        let now = Instant::now();
        // Already in channel 3, then joining call 1_000_000.
        let mut pending = Some((now, 1_000_000));
        // Unrelated message while still reported in the old channel.
        assert!(!LanvoxApp::resolve_out_of_call(&mut pending, Some(3), now));
        assert!(pending.is_some(), "old channel must not confirm the join");
        // Server's "left" broadcast before the new join.
        assert!(!LanvoxApp::resolve_out_of_call(&mut pending, None, now));
        // Confirmation of the call clears it.
        assert!(!LanvoxApp::resolve_out_of_call(
            &mut pending,
            Some(1_000_000),
            now
        ));
        assert!(pending.is_none());
        // Afterwards a real leave is honoured.
        assert!(LanvoxApp::resolve_out_of_call(&mut pending, None, now));
    }

    #[test]
    fn a_join_that_never_arrives_times_out() {
        let at = Instant::now();
        let mut pending = Some((at, 7));
        let later = at + Duration::from_secs(11);
        assert!(LanvoxApp::resolve_out_of_call(&mut pending, None, later));
        assert!(pending.is_none());
    }
}
