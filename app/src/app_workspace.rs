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
    pub file_read_reserved: usize,
    pub queries: HashMap<String, u64>,
    pub draft_pending: Vec<PendingDraft>,
    pub draft_restore: Option<Receiver<Vec<(ChannelId, StagedAttachment)>>>,
    pub extras: crate::ui::workspace::UiExtras,
    pub inbox_open: bool,
    pub members_overlay_open: bool,
    pub operation_errors: std::collections::VecDeque<String>,
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
    pub forum_pending_draft: Option<(ChannelId, crate::ui::workspace::ForumDraft)>,
    pub forum_failed_draft: Option<(ChannelId, crate::ui::workspace::ForumDraft)>,
    pub reply_to: Option<MessageId>,
    /// Channel or category being dragged in the sidebar.
    pub sidebar_drag: Option<crate::categories::SidebarDrag>,
    /// Caret (char index) of the composer while it has focus, for `@` autocomplete.
    pub mention_cursor: Option<usize>,
    pub mention_sel: usize,
    pub upload_receipts: HashMap<u64, String>,
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
    pub delete_guild_confirm: Option<GuildId>,
    pub management_guild: Option<GuildId>,
    pub management_conversation: Option<ChannelId>,
    pub rotation_confirm: bool,
    pub host_access_confirmation: Option<(UserId, bool)>,
    pub call_seen: HashMap<ChannelId, Instant>,
    pub sound_call: Option<ChannelId>,
    pub message_days: HashMap<MessageId, String>,
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
    pub file: crate::file_transfer::AtomicFile,
    pub path: std::path::PathBuf,
    pub written: u64,
    pub purpose: DownloadPurpose,
    pub last_progress: Instant,
    pub retry_at: Option<Instant>,
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
    pub digest: [u8; 32],
}

pub struct PendingDraft {
    pub channel: ChannelId,
    pub bytes: usize,
    pub receiver: Receiver<Result<StagedAttachment, String>>,
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

impl WorkspaceUi {
    pub fn attachment_bytes(&self) -> usize {
        self.staged
            .values()
            .flatten()
            .chain(self.upload_sources.values())
            .map(|f| f.bytes.len())
            .sum::<usize>()
            + self.draft_pending.iter().map(|p| p.bytes).sum::<usize>()
            + self.file_read_reserved
    }
    pub fn attachment_count(&self, channel: ChannelId) -> usize {
        self.staged.get(&channel).map_or(0, Vec::len)
            + self.attachments.get(&channel).map_or(0, Vec::len)
            + self.uploads.values().filter(|ch| **ch == channel).count()
            + self
                .draft_pending
                .iter()
                .filter(|p| p.channel == channel)
                .count()
    }
    pub fn can_stage(&self, channel: ChannelId, bytes: usize) -> bool {
        self.draft_restore.is_none()
            && bytes > 0
            && bytes <= MAX_ATTACHMENT_BYTES
            && self.attachment_count(channel) < MAX_MESSAGE_ATTACHMENTS
            && self.attachment_bytes().saturating_add(bytes) <= 2 * MAX_ATTACHMENT_BYTES
    }
    pub fn accept_query(&mut self, id: u64) -> Option<String> {
        let scope = self
            .queries
            .iter()
            .find(|(_, token)| **token == id)
            .map(|(scope, _)| scope.clone())?;
        self.queries.remove(&scope);
        Some(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn obsolete_query_responses_do_not_consume_the_current_request() {
        let mut workspace = WorkspaceUi::default();
        workspace.queries.insert("search".into(), 41);
        workspace.queries.insert("search".into(), 42);
        assert_eq!(workspace.accept_query(41), None);
        assert_eq!(workspace.accept_query(42), Some("search".into()));
        assert_eq!(workspace.accept_query(42), None);
    }
    #[test]
    fn reservations_and_pending_preparations_share_the_attachment_budget() {
        let mut workspace = WorkspaceUi {
            file_read_reserved: 2 * MAX_ATTACHMENT_BYTES,
            ..Default::default()
        };
        assert!(!workspace.can_stage(1, 1));
        workspace.file_read_reserved = 0;
        for _ in 0..MAX_MESSAGE_ATTACHMENTS {
            let (_tx, receiver) = bounded(1);
            workspace.draft_pending.push(PendingDraft {
                channel: 1,
                bytes: 1,
                receiver,
            });
        }
        assert!(!workspace.can_stage(1, 1));
        assert!(workspace.can_stage(2, 1));
    }
}
