//! The person's saved preferences.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::media::audio::InputMode;
use crate::media::screen::StreamSettings;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupDestination {
    pub host: String,
    pub guild: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupConnection {
    Disabled,
    #[default]
    LastConnected,
    Community(StartupDestination),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationLevel {
    #[default]
    All,
    Mentions,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationPreference {
    pub level: NotificationLevel,
    /// Unix seconds. i64::MAX means muted until explicitly enabled again.
    pub muted_until: i64,
}

impl NotificationPreference {
    pub fn allows(self, mentioned: bool, now: i64) -> bool {
        self.muted_until <= now
            && match self.level {
                NotificationLevel::All => true,
                NotificationLevel::Mentions => mentioned,
                NotificationLevel::None => false,
            }
    }
}

/// Canonical scope shared by favorites, credentials and local message recovery.
pub fn host_address(address: &str) -> String {
    let value = crate::branding::strip_scheme(address.trim())
        .unwrap_or(address.trim())
        .split('?')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if value.is_empty() {
        return value;
    }
    if value.starts_with('[') && value.ends_with(']') {
        format!("{value}:{}", crate::protocol::DEFAULT_PORT)
    } else if value
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_ipv6())
    {
        format!("[{value}]:{}", crate::protocol::DEFAULT_PORT)
    } else if value.contains(':') {
        value
    } else {
        format!("{value}:{}", crate::protocol::DEFAULT_PORT)
    }
}

pub fn key_fingerprint(public_key: [u8; 32]) -> String {
    public_key
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityMigration {
    pub host: String,
    pub user: u32,
    pub old_identity: String,
    pub new_identity: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostCredentials {
    pub access_token: String,
    pub expected_fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingMessage {
    pub operation: String,
    pub host: String,
    #[serde(default)]
    pub identity: String,
    pub channel: u32,
    pub text: String,
    pub reply_to: Option<u64>,
    pub attachments: Vec<u64>,
    #[serde(default)]
    pub attachment_drafts: Vec<String>,
    pub error: Option<String>,
    pub attempted_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PttKey {
    Backquote,
    CapsLock,
    V,
    B,
    F8,
    F9,
    F10,
    Mouse4,
    Mouse5,
}

impl PttKey {
    pub const ALL: [PttKey; 9] = [
        PttKey::Backquote,
        PttKey::CapsLock,
        PttKey::V,
        PttKey::B,
        PttKey::F8,
        PttKey::F9,
        PttKey::F10,
        PttKey::Mouse4,
        PttKey::Mouse5,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PttKey::Backquote => "` (acento grave)",
            PttKey::CapsLock => "Caps Lock",
            PttKey::V => "V",
            PttKey::B => "B",
            PttKey::F8 => "F8",
            PttKey::F9 => "F9",
            PttKey::F10 => "F10",
            PttKey::Mouse4 => "Botão lateral anterior do mouse",
            PttKey::Mouse5 => "Botão lateral seguinte do mouse",
        }
    }

    /// Windows virtual-key code.
    pub fn vk(self) -> u32 {
        match self {
            PttKey::Backquote => 0xC0,
            PttKey::CapsLock => 0x14,
            PttKey::V => 0x56,
            PttKey::B => 0x42,
            PttKey::F8 => 0x77,
            PttKey::F9 => 0x78,
            PttKey::F10 => 0x79,
            PttKey::Mouse4 => 0x05,
            PttKey::Mouse5 => 0x06,
        }
    }

    pub fn egui_key(self) -> Option<egui::Key> {
        match self {
            PttKey::Backquote => Some(egui::Key::Backtick),
            PttKey::V => Some(egui::Key::V),
            PttKey::B => Some(egui::Key::B),
            PttKey::F8 => Some(egui::Key::F8),
            PttKey::F9 => Some(egui::Key::F9),
            PttKey::F10 => Some(egui::Key::F10),
            _ => None,
        }
    }
}

/// A keyboard shortcut: modifiers plus a Windows virtual-key code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub vk: u32,
}

impl Hotkey {
    pub const fn new(ctrl: bool, shift: bool, alt: bool, vk: u32) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            vk,
        }
    }

    pub const fn none() -> Self {
        Self::new(false, false, false, 0)
    }

    pub fn is_set(&self) -> bool {
        self.vk != 0
    }

    pub fn label(&self) -> String {
        if !self.is_set() {
            return "Nenhum".into();
        }
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        parts.push(match self.vk {
            0x30..=0x39 | 0x41..=0x5A => (self.vk as u8 as char).to_string(),
            0xA0 => "Shift Esquerdo".into(),
            0xA1 => "Shift Direito".into(),
            0xA2 => "Ctrl Esquerdo".into(),
            0xA3 => "Ctrl Direito".into(),
            0xA4 => "Alt Esquerdo".into(),
            0xA5 => "Alt Direito".into(),
            0x70..=0x87 => format!("F{}", self.vk - 0x6F),
            _ => format!("0x{:02X}", self.vk),
        });
        parts.join("+")
    }

    /// Windows virtual-key code for the keys we let people bind (A-Z, 0-9, F1-F24).
    pub fn vk_for_key(key: egui::Key) -> Option<u32> {
        let name = key.name();
        let bytes = name.as_bytes();
        if bytes.len() == 1 && (bytes[0].is_ascii_uppercase() || bytes[0].is_ascii_digit()) {
            return Some(bytes[0] as u32);
        }
        let number: u32 = name.strip_prefix('F')?.parse().ok()?;
        (1..=24).contains(&number).then(|| 0x6F + number)
    }

    pub fn egui_key(&self) -> Option<egui::Key> {
        egui::Key::ALL
            .iter()
            .copied()
            .find(|k| Self::vk_for_key(*k) == Some(self.vk))
    }

    /// True on the frame this shortcut is pressed while the window has focus.
    pub fn pressed(&self, input: &egui::InputState) -> bool {
        self.egui_key().is_some_and(|key| {
            input.key_pressed(key)
                && input.modifiers.ctrl == self.ctrl
                && input.modifiers.shift == self.shift
                && input.modifiers.alt == self.alt
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    pub color: u32,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub input_mode: InputMode,
    pub threshold_db: f32,
    pub ptt_key: PttKey,
    pub input_gain: f32,
    pub output_volume: f32,
    /// Local volume and mute keyed by identity, host and user ID.
    pub user_audio: HashMap<String, (f32, bool)>,
    /// Local volume and mute for other people's screen-share audio, same keys as `user_audio`.
    pub stream_audio: HashMap<String, (f32, bool)>,
    pub camera_index: u32,
    pub camera: crate::media::camera::CameraSettings,
    pub stream: StreamSettings,
    pub last_address: String,
    pub startup_connection: StartupConnection,
    pub last_destination: Option<StartupDestination>,
    pub host_name: String,
    pub favorites: Vec<String>,
    pub hosts: HashMap<String, HostCredentials>,
    pub identity_migrations: Vec<IdentityMigration>,
    pub drafts: HashMap<String, String>,
    /// Last channel visited per community, keyed "identity|host|guild".
    pub last_channels: HashMap<String, u32>,
    /// Quick switcher recents (newest first), keyed "identity|host".
    pub recents: HashMap<String, Vec<String>>,
    /// Collapsed sidebar categories per community, keyed "identity|host|guild" (category ids).
    pub collapsed_categories: HashMap<String, Vec<u32>>,
    pub emoji_recents: Vec<String>,
    pub read_positions: HashMap<String, u64>,
    pub blocked_users: HashMap<String, Vec<u32>>,
    pub channel_notifications: HashMap<String, NotificationPreference>,
    pub guild_notifications: HashMap<String, NotificationPreference>,
    pub notification_preview: bool,
    /// Use Cisco's OpenH264 binary (downloaded on this device) for motion video.
    pub h264_codec: bool,
    /// Fetch thumbnails for video links from the linked site (YouTube or any direct URL).
    /// Off by default: the request reveals this device's IP address to that site.
    pub remote_link_previews: bool,
    /// Check, download and install signed updates automatically. On by default; the request reveals this
    /// device's IP address to GitHub and can be turned off in Settings.
    pub check_updates_on_start: bool,
    pub ui_scale: f32,
    /// 0 compact, 1 default, 2 spacious.
    pub ui_density: u8,
    /// Chat text size and space before a new author's message group, in logical points.
    pub chat_text_size: f32,
    pub chat_group_spacing: f32,
    /// 0 dark, 1 onyx, 2 ash.
    pub theme: u8,
    pub channel_width: f32,
    pub member_width: f32,
    pub auto_reconnect: bool,
    pub notifications: bool,
    pub do_not_disturb: bool,
    pub access_token: String,
    pub expected_host: String,
    pub host_bind: String,
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    pub rnnoise: bool,
    pub smart_margin_db: f32,
    pub echo_delay_ms: u32,
    pub pending: Vec<PendingMessage>,
    pub sounds: bool,
    pub sounds_volume: f32,
    pub mute_hotkey: Hotkey,
    pub deafen_hotkey: Hotkey,
}

impl Default for Settings {
    fn default() -> Self {
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        let name = if user.trim().is_empty() {
            "Friend".to_string()
        } else {
            user
        };
        Self {
            name,
            color: crate::model::AVATAR_COLORS[0],
            input_device: None,
            output_device: None,
            input_mode: InputMode::SmartVoice,
            threshold_db: crate::media::audio::VAD_THRESHOLD_DB,
            ptt_key: PttKey::Backquote,
            input_gain: 1.0,
            output_volume: 1.0,
            user_audio: HashMap::new(),
            stream_audio: HashMap::new(),
            camera_index: 0,
            camera: Default::default(),
            stream: StreamSettings::default(),
            last_address: String::new(),
            startup_connection: StartupConnection::default(),
            last_destination: None,
            host_name: String::new(),
            favorites: Vec::new(),
            hosts: HashMap::new(),
            identity_migrations: Vec::new(),
            drafts: HashMap::new(),
            last_channels: HashMap::new(),
            recents: HashMap::new(),
            collapsed_categories: HashMap::new(),
            emoji_recents: Vec::new(),
            read_positions: HashMap::new(),
            blocked_users: HashMap::new(),
            channel_notifications: HashMap::new(),
            guild_notifications: HashMap::new(),
            notification_preview: false,
            h264_codec: true,
            remote_link_previews: false,
            check_updates_on_start: true,
            ui_scale: 1.0,
            ui_density: 1,
            chat_text_size: 15.0,
            chat_group_spacing: 14.0,
            theme: 0,
            channel_width: 240.0,
            member_width: 240.0,
            auto_reconnect: true,
            notifications: true,
            do_not_disturb: false,
            access_token: String::new(),
            expected_host: String::new(),
            host_bind: "127.0.0.1".into(),
            echo_cancellation: true,
            noise_suppression: true,
            rnnoise: true,
            smart_margin_db: crate::media::audio::SMART_DEFAULT_MARGIN_DB,
            echo_delay_ms: 40,
            pending: Vec::new(),
            sounds: true,
            sounds_volume: 0.6,
            mute_hotkey: Hotkey::new(true, true, false, 0x4D),
            deafen_hotkey: Hotkey::new(true, true, false, 0x44),
        }
    }
}

fn base_dir() -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("Lanvox");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

pub fn server_save_path() -> Option<PathBuf> {
    base_dir().map(|d| d.join("hosted-server.json"))
}

pub fn identity() -> std::io::Result<crate::auth::Identity> {
    let dir = base_dir()
        .ok_or_else(|| std::io::Error::other("Could not open the Lanvox configuration folder"))?;
    crate::auth::Identity::load_or_create(&dir.join("identity.key"))
}
static SETTINGS_WRITE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
static SETTINGS_PENDING: parking_lot::Mutex<Option<(u64, Settings)>> =
    parking_lot::Mutex::new(None);
static SETTINGS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SETTINGS_BYTES: parking_lot::Mutex<Vec<u8>> = parking_lot::Mutex::new(Vec::new());
pub fn flush_saves() {
    let _guard = SETTINGS_WRITE.lock();
    SETTINGS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let pending = SETTINGS_PENDING.lock().take();
    if let Some((_, settings)) = pending {
        let _ = settings.write_settings_file();
    }
}
fn preference_worker() -> &'static crossbeam_channel::Sender<()> {
    static WORKER: std::sync::OnceLock<crossbeam_channel::Sender<()>> = std::sync::OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            while rx.recv().is_ok() {
                let snapshot = SETTINGS_PENDING.lock().clone();
                if let Some((generation, settings)) = snapshot {
                    let _guard = SETTINGS_WRITE.lock();
                    if generation == SETTINGS_GENERATION.load(std::sync::atomic::Ordering::SeqCst) {
                        let result = settings.write_settings_file();
                        let mut pending = SETTINGS_PENDING.lock();
                        if result.is_ok()
                            && pending
                                .as_ref()
                                .is_some_and(|(token, _)| *token == generation)
                        {
                            *pending = None;
                        }
                    }
                }
            }
        });
        tx
    })
}

impl Settings {
    pub fn startup_destination(&self) -> Option<StartupDestination> {
        let mut destination = match &self.startup_connection {
            StartupConnection::Disabled => return None,
            StartupConnection::Community(destination) => destination.clone(),
            StartupConnection::LastConnected => {
                self.last_destination
                    .clone()
                    .unwrap_or_else(|| StartupDestination {
                        host: self.last_address.clone(),
                        guild: None,
                    })
            }
        };
        destination.host = host_address(&destination.host);
        (!destination.host.is_empty()).then_some(destination)
    }

    pub fn draft_is_queued(&self, identity: &str, host: &str, operation: &str) -> bool {
        self.pending.iter().any(|pending| {
            pending.identity == identity
                && pending.host == host
                && pending
                    .attachment_drafts
                    .iter()
                    .any(|draft| draft == operation)
        })
    }

    pub fn credentials_for(&self, address: &str) -> HostCredentials {
        self.hosts
            .get(&host_address(address))
            .cloned()
            .unwrap_or_default()
    }

    pub fn migrate_identity_scope(&mut self, migration: &IdentityMigration) {
        let prefix = format!("{}|{}|", migration.old_identity, migration.host);
        let replacement = format!("{}|{}|", migration.new_identity, migration.host);
        rekey_preferences(&mut self.read_positions, &prefix, &replacement);
        rekey_preferences(&mut self.channel_notifications, &prefix, &replacement);
        rekey_preferences(&mut self.guild_notifications, &prefix, &replacement);
        rekey_preferences(&mut self.last_channels, &prefix, &replacement);
        rekey_preferences(&mut self.collapsed_categories, &prefix, &replacement);
        rekey_preferences(&mut self.user_audio, &prefix, &replacement);
        rekey_preferences(&mut self.stream_audio, &prefix, &replacement);
        let old_scope = format!("{}|{}", migration.old_identity, migration.host);
        let new_scope = format!("{}|{}", migration.new_identity, migration.host);
        if let Some(value) = self.blocked_users.remove(&old_scope) {
            self.blocked_users.entry(new_scope.clone()).or_insert(value);
        }
        if let Some(value) = self.recents.remove(&old_scope) {
            self.recents.entry(new_scope).or_insert(value);
        }
        let drafts: Vec<_> = self
            .drafts
            .iter()
            .filter_map(|(key, text)| {
                key.strip_prefix(&prefix).map(|channel| {
                    (
                        format!("{}|{}|{channel}", migration.new_identity, migration.host),
                        text.clone(),
                        key.clone(),
                    )
                })
            })
            .collect();
        for (key, text, old_key) in drafts {
            self.drafts.entry(key).or_insert(text);
            self.drafts.remove(&old_key);
        }
        for message in &mut self.pending {
            if host_address(&message.host) == migration.host
                && message.identity == migration.old_identity
            {
                message.host = migration.host.clone();
                message.identity = migration.new_identity.clone();
            }
        }
        self.identity_migrations
            .retain(|receipt| receipt != migration);
    }

    pub fn load() -> Settings {
        base_dir()
            .and_then(|d| std::fs::read(d.join("settings.json")).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn try_save(&self) -> std::io::Result<()> {
        let _guard = SETTINGS_WRITE.lock();
        let generation = SETTINGS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let mut pending = SETTINGS_PENDING.lock();
        if pending
            .as_ref()
            .is_some_and(|(token, _)| *token < generation)
        {
            *pending = None;
        }
        drop(pending);
        self.write_settings_file()
    }
    fn write_settings_file(&self) -> std::io::Result<()> {
        use std::io::Write;
        let d = base_dir()
            .ok_or_else(|| std::io::Error::other("Pasta de configuração indisponível"))?;
        let json = serde_json::to_vec_pretty(self)?;
        if *SETTINGS_BYTES.lock() == json {
            return Ok(());
        }
        let path = d.join("settings.json");
        let mut file = crate::file_transfer::AtomicFile::new(&path)?;
        file.write_all(&json)?;
        file.commit()?;
        *SETTINGS_BYTES.lock() = json;
        Ok(())
    }

    pub fn save(&self) {
        let mut pending = SETTINGS_PENDING.lock();
        let generation = SETTINGS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        *pending = Some((generation, self.clone()));
        drop(pending);
        let _ = preference_worker().try_send(());
    }
}

fn rekey_preferences<T: Clone>(
    values: &mut HashMap<String, T>,
    old_prefix: &str,
    new_prefix: &str,
) {
    let moved: Vec<_> = values
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix(old_prefix)
                .map(|suffix| (key.clone(), format!("{new_prefix}{suffix}"), value.clone()))
        })
        .collect();
    for (old, new, value) in moved {
        values.remove(&old);
        values.entry(new).or_insert(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_migrates_legacy_settings_without_invite_secrets() {
        let settings: Settings =
            serde_json::from_str(r#"{"last_address":"lanvox://Friends?token=secret"}"#).unwrap();
        assert_eq!(
            settings.startup_destination(),
            Some(StartupDestination {
                host: "friends:47800".into(),
                guild: None
            })
        );
        assert!(Settings::default().startup_destination().is_none());
    }

    #[test]
    fn startup_pinned_destination_is_independent_of_the_last_connection() {
        let pinned = StartupDestination {
            host: "friends:47800".into(),
            guild: Some(7),
        };
        let mut settings = Settings {
            startup_connection: StartupConnection::Community(pinned.clone()),
            last_destination: Some(StartupDestination {
                host: "other:47800".into(),
                guild: Some(9),
            }),
            ..Default::default()
        };
        let reloaded: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(reloaded.startup_destination(), Some(pinned));
        settings.startup_connection = StartupConnection::LastConnected;
        assert_eq!(settings.startup_destination(), settings.last_destination);
        settings.startup_connection = StartupConnection::Disabled;
        assert!(settings.startup_destination().is_none());
    }
    #[test]
    fn favorite_scope_is_canonical_and_never_contains_invite_secrets() {
        assert_eq!(host_address("FRIENDS"), "friends:47800");
        assert_eq!(
            host_address("lanvox://Friends:47800?token=secret&fingerprint=abc"),
            "friends:47800"
        );
        assert_eq!(host_address("::1"), "[::1]:47800");
        assert_eq!(host_address("[::1]"), "[::1]:47800");
        assert_eq!(host_address("[::1]:5000"), "[::1]:5000");
        assert!(host_address("").is_empty());
    }

    #[test]
    fn rotation_preserves_other_hosts_and_moves_only_the_confirmed_identity() {
        let mut settings = Settings::default();
        settings
            .blocked_users
            .insert("old|friends:47800".into(), vec![9]);
        settings
            .blocked_users
            .insert("old|other:47800".into(), vec![12]);
        settings
            .read_positions
            .insert("old|friends:47800|1".into(), 50);
        settings.channel_notifications.insert(
            "old|friends:47800|1".into(),
            NotificationPreference {
                level: NotificationLevel::Mentions,
                muted_until: 999,
            },
        );
        settings.channel_notifications.insert(
            "old|other:47800|1".into(),
            NotificationPreference::default(),
        );
        settings
            .drafts
            .insert("old|friends:47800|1".into(), "my draft".into());
        settings
            .drafts
            .insert("old|other:47800|1".into(), "other host draft".into());
        settings.drafts.insert(
            "different|friends:47800|1".into(),
            "different identity".into(),
        );
        let receipt = IdentityMigration {
            host: "friends:47800".into(),
            user: 7,
            old_identity: "old".into(),
            new_identity: "new".into(),
        };
        settings.identity_migrations.push(receipt.clone());
        for (host, identity) in [
            ("friends:47800", "old"),
            ("other:47800", "old"),
            ("friends:47800", "different"),
        ] {
            settings.pending.push(PendingMessage {
                operation: format!("{host}-{identity}"),
                host: host.into(),
                identity: identity.into(),
                channel: 1,
                text: "pending".into(),
                reply_to: None,
                attachments: Vec::new(),
                attachment_drafts: Vec::new(),
                error: None,
                attempted_at: 0,
            });
        }
        settings.migrate_identity_scope(&receipt);
        assert_eq!(settings.drafts["new|friends:47800|1"], "my draft");
        assert!(!settings.drafts.contains_key("old|friends:47800|1"));
        assert_eq!(settings.drafts["old|other:47800|1"], "other host draft");
        assert_eq!(
            settings.drafts["different|friends:47800|1"],
            "different identity"
        );
        assert_eq!(settings.pending[0].identity, "new");
        assert_eq!(settings.pending[1].identity, "old");
        assert_eq!(settings.pending[2].identity, "different");
        assert!(settings.identity_migrations.is_empty());
        assert_eq!(settings.blocked_users["new|friends:47800"], vec![9]);
        assert_eq!(settings.blocked_users["old|other:47800"], vec![12]);
        assert!(!settings.blocked_users.contains_key("old|friends:47800"));
        assert_eq!(settings.read_positions["new|friends:47800|1"], 50);
        assert_eq!(
            settings.channel_notifications["new|friends:47800|1"].level,
            NotificationLevel::Mentions
        );
        assert!(settings
            .channel_notifications
            .contains_key("old|other:47800|1"));
    }
    #[test]
    fn connection_credentials_are_selected_by_host_instead_of_active_form_fields() {
        let mut settings = Settings {
            access_token: "residual-b".into(),
            expected_host: "fingerprint-b".into(),
            ..Default::default()
        };
        settings.hosts.insert(
            "a:47800".into(),
            HostCredentials {
                access_token: "token-a".into(),
                expected_fingerprint: "fingerprint-a".into(),
            },
        );
        assert_eq!(settings.credentials_for("A").access_token, "token-a");
        assert_eq!(
            settings.credentials_for("a:47800").expected_fingerprint,
            "fingerprint-a"
        );
        assert!(settings.credentials_for("new-host").access_token.is_empty());
    }
    #[test]
    fn pending_attachment_recovery_is_scoped_and_backward_compatible() {
        let mut settings = Settings::default();
        let pending = PendingMessage {
            operation: "message".into(),
            host: "a:47800".into(),
            identity: "alice".into(),
            channel: 1,
            text: "file".into(),
            reply_to: None,
            attachments: vec![9],
            attachment_drafts: vec!["draft".into()],
            error: None,
            attempted_at: 0,
        };
        let mut legacy = serde_json::to_value(&pending).unwrap();
        legacy.as_object_mut().unwrap().remove("attachment_drafts");
        assert!(serde_json::from_value::<PendingMessage>(legacy)
            .unwrap()
            .attachment_drafts
            .is_empty());
        settings.pending.push(pending);
        assert!(settings.draft_is_queued("alice", "a:47800", "draft"));
        assert!(!settings.draft_is_queued("bob", "a:47800", "draft"));
        assert!(!settings.draft_is_queued("alice", "b:47800", "draft"));
        settings.pending.clear();
        assert!(!settings.draft_is_queued("alice", "a:47800", "draft"));
    }
}
