//! Desktop state for durable conversations and community administration.
use super::*;
#[path = "app_workspace/downloads.rs"]
mod downloads;
#[path = "app_workspace/events.rs"]
mod events;
#[path = "app_workspace/history.rs"]
mod history;
#[path = "app_workspace/identity.rs"]
mod identity;
#[path = "app_workspace/search.rs"]
mod search;
#[path = "app_workspace/textures.rs"]
mod textures;
#[path = "app_workspace/uploads.rs"]
mod uploads;

#[derive(Default)]
pub struct WorkspaceUi {
    pub extras: crate::ui::workspace::UiExtras,
    pub inbox_open: bool,
    pub inbox_results: Vec<Message>,
    pub inbox_loading: bool,
    pub inbox_dirty: bool,
    pub inbox_error: Option<String>,
    pub inbox_has_more: bool,
    pub inbox_requested: Option<bool>,
    pub inbox_before: Option<MessageId>,
    pub search_request: SearchRequest,
    pub search_submitted: SearchRequest,
    pub search_loading: bool,
    pub search_has_more: bool,
    pub search_offset: u32,
    pub search_error: Option<String>,
    pub first_unread: HashMap<ChannelId, MessageId>,
    pub reading_positions: HashMap<ChannelId, MessageId>,
    pub history_has_after: HashMap<ChannelId, bool>,
    pub highlighted: Option<(MessageId, Instant)>,
    pub staged: HashMap<ChannelId, Vec<StagedAttachment>>,
    pub cancelled_uploads: HashSet<String>,
    pub upload_names: HashMap<String, String>,
    pub upload_sources: HashMap<String, StagedAttachment>,
    pub send_after_upload: HashSet<ChannelId>,
    pub pending_batches: HashMap<ChannelId, PendingBatch>,
    pub emoji_query: String,
    pub custom_emoji_textures: HashMap<u64, egui::TextureHandle>,
    pub poll_channel: Option<ChannelId>,
    pub poll_question: String,
    pub poll_options: String,
    pub poll_multiple: bool,
    pub poll_choices: HashMap<MessageId, Vec<u8>>,
    pub forwarding: Option<Message>,
    pub forwarding_target: Option<ChannelId>,
    pub forwarding_note: String,
    pub gallery: Option<u64>,
    pub gallery_zoom: f32,
    pub gallery_texture: Option<(u64, egui::TextureHandle)>,
    pub thread_name: String,
    pub thread_from: Option<Message>,
    pub pending_thread: Option<ChannelId>,
    pub forum_pending_draft: Option<(ChannelId, String)>,
    pub forum_failed_draft: Option<(ChannelId, String)>,
    pub reply_to: Option<MessageId>,
    /// Channel or category being dragged in the sidebar.
    pub sidebar_drag: Option<crate::categories::SidebarDrag>,
    /// Caret (char index) of the composer while it has focus, for `@` autocomplete.
    pub mention_cursor: Option<usize>,
    pub mention_sel: usize,
    pub attachments: HashMap<ChannelId, Vec<AttachmentInfo>>,
    pub uploads: HashMap<String, ChannelId>,
    pub downloads: HashMap<u64, (AttachmentInfo, Vec<u8>)>,
    pub preview_requests: HashSet<u64>,
    pub download_requests: HashSet<u64>,
    /// Chunked uploads in flight, keyed by operation.
    pub chunk_uploads: HashMap<String, ChunkUpload>,
    /// Chunked downloads being reassembled, keyed by attachment id.
    pub chunk_downloads: HashMap<u64, ChunkDownload>,
    /// Large videos/audio already downloaded into the media cache, by attachment id.
    pub media_files: HashMap<u64, std::path::PathBuf>,
    pub search_results: Vec<Message>,
    pub search_open: bool,
    pub search_query: String,
    pub pinned_only: bool,
    pub pinned_messages: HashMap<ChannelId, Vec<Message>>,
    pub editing: Option<(MessageId, String)>,
    pub deleting: Option<MessageId>,
    pub managing: bool,
    pub profile: Option<UserId>,
    pub profile_status: String,
    pub profile_presence: Presence,
    pub profile_avatar: Vec<u8>,
    pub group_open: bool,
    pub group_name: String,
    pub group_members: HashSet<UserId>,
    pub pending_conversation: Option<(String, Vec<UserId>, bool)>,
    pub invite_input: String,
    pub invite_created: String,
    pub channel_name: String,
    pub channel_topic: String,
    /// Category chosen in the channel editor (None = no category) and the one it started with.
    pub channel_category: Option<CategoryId>,
    pub channel_category_before: Option<CategoryId>,
    pub channel_allowed: HashSet<UserId>,
    pub editing_channel: Option<ChannelId>,
    pub guild_name: String,
    pub delete_channel_confirm: bool,
    pub delete_guild_confirm: bool,
    pub rotation_confirm: bool,
    pub host_access_confirmation: Option<(UserId, bool)>,
    pub call_seen: HashMap<ChannelId, Instant>,
    pub sound_call: Option<ChannelId>,
    pub message_heights: HashMap<MessageId, f32>,
    pub message_width: f32,
    pub history_anchor: Option<MessageId>,
    pub recent_history: HashSet<ChannelId>,
    pub last_audio_error: Option<String>,
}

/// What a chunked download is for.
pub enum DownloadPurpose {
    /// Cached to play inline.
    Play,
    /// Saved where the user chose.
    Save,
}

/// A large attachment being written to disk piece by piece (never held whole in memory).
pub struct ChunkDownload {
    pub info: AttachmentInfo,
    pub file: std::fs::File,
    pub path: std::path::PathBuf,
    pub written: u64,
    pub purpose: DownloadPurpose,
    pub last_progress: Instant,
}

/// Progress of one chunked upload: `sent` bytes written to the connection, `acked` bytes the
/// host confirmed storing. At most `UPLOAD_WINDOW_CHUNKS` chunks are in flight.
pub struct ChunkUpload {
    pub total: u64,
    pub sent: u64,
    pub acked: u64,
    pub finishing: bool,
}

pub struct StagedAttachment {
    pub operation: String,
    pub name: String,
    pub bytes: Vec<u8>,
    pub preview: Option<egui::TextureHandle>,
}

pub struct PendingBatch {
    pub operation: String,
    pub text: String,
    pub reply_to: Option<MessageId>,
}

pub enum FileTask {
    Uploads {
        channel: ChannelId,
        files: Vec<(String, Vec<u8>)>,
    },
    Upload {
        channel: ChannelId,
        name: String,
        bytes: Vec<u8>,
    },
    Avatar(Vec<u8>),
    /// Where the user wants a large attachment saved; it is then streamed straight there.
    SaveTarget {
        info: AttachmentInfo,
        path: std::path::PathBuf,
    },
    Error(String),
    Saved(String),
    IdentityImported,
    Cancelled,
}

impl LanvoxApp {}
