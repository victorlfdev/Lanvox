//! Domain types shared by the server, the client and the interface.

use serde::{Deserialize, Serialize};

pub type UserId = u32;
pub type ChannelId = u32;
pub type MessageId = u64;
pub type GuildId = u32;
pub type CategoryId = u32;
/// Private calls use channel ids from this number up, so they never clash with server channels.
pub const CALL_ID_BASE: ChannelId = 1_000_000;

fn default_guild() -> GuildId {
    1
}

/// Longest message accepted.
pub const MAX_MESSAGE_CHARS: usize = 2000;
/// Messages from one person within this many seconds share one header (estimated).
pub const GROUP_WINDOW_SECS: i64 = 7 * 60;

pub const MAX_ATTACHMENT_BYTES: usize = 100 * 1024 * 1024;
/// Maximum attachments accepted in a single message, shared by client and host.
pub const MAX_MESSAGE_ATTACHMENTS: usize = 8;
/// Largest file the single-message `Upload`/`Download` commands still carry; anything bigger
/// travels in `ATTACHMENT_CHUNK_BYTES` pieces (`UploadBegin`/`UploadChunk`/`DownloadChunk`).
pub const MAX_INLINE_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// Size of one upload piece. It is also the host's storage unit (piece `n` holds bytes
/// `n * ATTACHMENT_CHUNK_BYTES ..`), so it must never change once data exists.
pub const ATTACHMENT_CHUNK_BYTES: usize = 128 * 1024;
/// How much of a file one `DownloadChunk` answer carries.
pub const DOWNLOAD_PIECE_BYTES: usize = 512 * 1024;
/// Total attachment storage a host accepts (completed files plus uploads in progress).
pub const ATTACHMENT_STORE_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
/// Pieces a client may have sent but not yet seen acknowledged (1 MiB in flight).
pub const UPLOAD_WINDOW_CHUNKS: u64 = 8;
pub const MAX_HISTORY_PAGE: usize = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberRole {
    Owner,
    Admin,
    #[default]
    Member,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Membership {
    pub guild: GuildId,
    pub user: UserId,
    pub role: MemberRole,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSettings {
    pub channel: ChannelId,
    /// Legacy free-text category (schema v2). Empty once the channel is migrated to a
    /// `Category` entity; only read to convert old data (see `categories::adopt_text_categories`).
    #[serde(default)]
    pub category: String,
    /// Position inside the channel's category (or inside the uncategorised group at the top).
    pub position: i32,
    /// Empty means all community members. Administrators can always access it.
    pub allowed: Vec<UserId>,
    /// The category this channel belongs to; None means "no category" (shown at the top).
    /// Appended last for wire/data compatibility.
    #[serde(default)]
    pub category_id: Option<CategoryId>,
}

/// A named, ordered group of channels (text and voice mixed) inside one community.
/// Permission inheritance is represented by CommunitySnapshot.overrides using this id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: CategoryId,
    #[serde(default = "default_guild")]
    pub guild: GuildId,
    pub name: String,
    pub position: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conversation {
    pub channel: ChannelId,
    pub name: String,
    pub owner: UserId,
    pub members: Vec<UserId>,
    pub direct: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    #[default]
    Online,
    Away,
    DoNotDisturb,
    Invisible,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Profile {
    pub user: UserId,
    pub status: String,
    pub presence: Presence,
    pub avatar: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub id: u64,
    pub channel: ChannelId,
    pub uploader: UserId,
    pub name: String,
    pub media_type: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reaction {
    pub emoji: String,
    pub users: Vec<UserId>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MessageDetails {
    pub message: MessageId,
    pub reply_to: Option<MessageId>,
    pub edited: Option<i64>,
    pub attachments: Vec<AttachmentInfo>,
    pub reactions: Vec<Reaction>,
    pub mentions: Vec<UserId>,
    pub pinned: bool,
    /// Author and excerpt of the replied-to message, filled by the server when it sends details
    /// (never persisted). Appended last, optional, for wire compatibility.
    #[serde(default)]
    pub reply_preview: Option<ReplyPreview>,
}

/// What the composer/row shows for the message being replied to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyPreview {
    pub author: UserId,
    pub author_name: String,
    pub text: String,
    /// The original no longer exists.
    pub deleted: bool,
}

pub const REPLY_EXCERPT_CHARS: usize = 120;

impl ReplyPreview {
    /// Builds the preview from the original message, or the "removed" state when it is gone.
    pub fn from_original(original: Option<&Message>) -> Self {
        match original {
            Some(m) => Self {
                author: m.author,
                author_name: m.author_name.clone(),
                text: m.text.chars().take(REPLY_EXCERPT_CHARS).collect(),
                deleted: false,
            },
            None => Self {
                deleted: true,
                ..Default::default()
            },
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub conversations: Vec<Conversation>,
    pub memberships: Vec<Membership>,
    pub channel_settings: Vec<ChannelSettings>,
    pub profiles: Vec<Profile>,
    pub details: Vec<MessageDetails>,
    pub read: Vec<(ChannelId, MessageId)>,
    pub host_owner: Option<UserId>,
    pub disabled_users: Vec<UserId>,
    /// Channel categories of the communities the person belongs to. Appended last.
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub community: Box<CommunitySnapshot>,
}

/// Both identities sign this domain-separated transcript when rotating a host-local identity.
pub fn identity_rotation_transcript(
    user: UserId,
    old: &[u8; 32],
    new: &[u8; 32],
    nonce: &[u8; 32],
) -> Vec<u8> {
    // Kept from the previous name on purpose; see branding.rs.
    let mut bytes = b"Huddle identity rotation v3\0".to_vec();
    bytes.extend_from_slice(&user.to_le_bytes());
    bytes.extend_from_slice(old);
    bytes.extend_from_slice(new);
    bytes.extend_from_slice(nonce);
    bytes
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorkspaceCommand {
    Refresh,
    RotateIdentity {
        old_public_key: [u8; 32],
        new_public_key: [u8; 32],
        nonce: [u8; 32],
        old_signature: Vec<u8>,
        new_signature: Vec<u8>,
    },
    SetHostAccess {
        user: UserId,
        allowed: bool,
    },
    Pinned {
        channel: ChannelId,
    },
    Send {
        operation: String,
        channel: ChannelId,
        text: String,
        reply_to: Option<MessageId>,
        attachments: Vec<u64>,
    },
    History {
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u16,
    },
    OpenConversation {
        name: String,
        members: Vec<UserId>,
        direct: bool,
    },
    UpdateConversation {
        channel: ChannelId,
        name: String,
        members: Vec<UserId>,
    },
    LeaveConversation {
        channel: ChannelId,
    },
    Edit {
        message: MessageId,
        text: String,
    },
    Delete {
        message: MessageId,
    },
    React {
        message: MessageId,
        emoji: String,
    },
    Pin {
        message: MessageId,
        pinned: bool,
    },
    Search {
        channel: Option<ChannelId>,
        query: String,
    },
    Read {
        channel: ChannelId,
        message: MessageId,
    },
    Upload {
        operation: String,
        channel: ChannelId,
        name: String,
        media_type: String,
        bytes: Vec<u8>,
    },
    Download {
        attachment: u64,
    },
    UpdateProfile {
        status: String,
        presence: Presence,
        avatar: Vec<u8>,
    },
    UpdateChannel {
        channel: ChannelId,
        name: String,
        topic: String,
        category: String,
        position: i32,
        allowed: Vec<UserId>,
    },
    DeleteChannel {
        channel: ChannelId,
    },
    UpdateGuild {
        guild: GuildId,
        name: String,
    },
    DeleteGuild {
        guild: GuildId,
    },
    CreateInvite {
        guild: GuildId,
        max_uses: u16,
        expires_hours: u16,
    },
    RedeemInvite {
        code: String,
    },
    RevokeInvite {
        code: String,
    },
    SetRole {
        guild: GuildId,
        user: UserId,
        role: MemberRole,
    },
    RemoveMember {
        guild: GuildId,
        user: UserId,
        ban: bool,
    },
    LeaveGuild {
        guild: GuildId,
    },
    Backup,
    /// Filtered, paged, indexed search. Appended last so existing variant indices (and old
    /// clients using `Search`) keep working on the bincode wire; answered with `SearchPage`.
    SearchAdvanced(SearchRequest),
    /// Category management. All of these need the same permission as editing channels
    /// (community administrator); appended last for wire compatibility.
    CreateCategory {
        guild: GuildId,
        name: String,
    },
    RenameCategory {
        category: CategoryId,
        name: String,
    },
    /// Channels of the category are kept and become uncategorised.
    DeleteCategory {
        category: CategoryId,
    },
    /// New order of the community's categories. Ids not listed keep their relative order after.
    ReorderCategories {
        guild: GuildId,
        order: Vec<CategoryId>,
    },
    /// Moves (or reorders) a channel: `index` counts the target group's channels without the
    /// moved one; None category means the uncategorised group at the top.
    MoveChannel {
        channel: ChannelId,
        category: Option<CategoryId>,
        index: u32,
    },
    /// Same as creating a channel, directly inside a category (at its end).
    CreateChannelIn {
        guild: GuildId,
        name: String,
        kind: ChannelKind,
        category: Option<CategoryId>,
    },
    Community(CommunityCommand),
    /// Chunked upload (appended last for wire compatibility): announce, stream, finish.
    /// The server acknowledges every chunk with `UploadProgress` and ends with `Uploaded`.
    UploadBegin {
        operation: String,
        channel: ChannelId,
        name: String,
        media_type: String,
        size: u64,
    },
    UploadChunk {
        operation: String,
        offset: u64,
        data: Vec<u8>,
    },
    UploadFinish {
        operation: String,
    },
    UploadAbort {
        operation: String,
    },
    /// Asks for one piece of an attachment; answered with `AttachmentChunk`.
    DownloadChunk {
        attachment: u64,
        offset: u64,
    },
    /// Page of unread messages or unread mentions across the caller's accessible channels.
    Inbox {
        mentions_only: bool,
        before: Option<MessageId>,
        limit: u16,
    },
    /// Begin or resume the same file, bound to its content digest and authenticated user.
    UploadBeginResumable {
        operation: String,
        channel: ChannelId,
        name: String,
        media_type: String,
        size: u64,
        digest: [u8; 32],
    },
    Query {
        id: u64,
        command: Box<QueryCommand>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum QueryCommand {
    History {
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u16,
    },
    SearchAdvanced(SearchRequest),
    Inbox {
        mentions_only: bool,
        before: Option<MessageId>,
        limit: u16,
    },
    Pinned {
        channel: ChannelId,
    },
    Context {
        channel: ChannelId,
        message: MessageId,
        limit: u16,
    },
    After {
        channel: ChannelId,
        after: MessageId,
        limit: u16,
    },
}
impl QueryCommand {
    pub fn into_command(self) -> WorkspaceCommand {
        match self {
            Self::History {
                channel,
                before,
                limit,
            } => WorkspaceCommand::History {
                channel,
                before,
                limit,
            },
            Self::SearchAdvanced(request) => WorkspaceCommand::SearchAdvanced(request),
            Self::Inbox {
                mentions_only,
                before,
                limit,
            } => WorkspaceCommand::Inbox {
                mentions_only,
                before,
                limit,
            },
            Self::Pinned { channel } => WorkspaceCommand::Pinned { channel },
            Self::Context {
                channel,
                message,
                limit,
            } => WorkspaceCommand::Community(CommunityCommand::MessageContext {
                channel,
                message,
                limit,
            }),
            Self::After {
                channel,
                after,
                limit,
            } => WorkspaceCommand::Community(CommunityCommand::HistoryAfter {
                channel,
                after,
                limit,
            }),
        }
    }
    pub fn from_command(command: WorkspaceCommand) -> Option<Self> {
        Some(match command {
            WorkspaceCommand::History {
                channel,
                before,
                limit,
            } => Self::History {
                channel,
                before,
                limit,
            },
            WorkspaceCommand::SearchAdvanced(request) => Self::SearchAdvanced(request),
            WorkspaceCommand::Inbox {
                mentions_only,
                before,
                limit,
            } => Self::Inbox {
                mentions_only,
                before,
                limit,
            },
            WorkspaceCommand::Pinned { channel } => Self::Pinned { channel },
            WorkspaceCommand::Community(CommunityCommand::MessageContext {
                channel,
                message,
                limit,
            }) => Self::Context {
                channel,
                message,
                limit,
            },
            WorkspaceCommand::Community(CommunityCommand::HistoryAfter {
                channel,
                after,
                limit,
            }) => Self::After {
                channel,
                after,
                limit,
            },
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchOrder {
    #[default]
    Newest,
    Oldest,
    /// Best text match first (BM25); falls back to newest when there is no text.
    Relevance,
}

/// Every filter is optional; they combine with AND. `after`/`before` are inclusive Unix seconds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SearchRequest {
    /// None searches every conversation the caller may read.
    pub channel: Option<ChannelId>,
    /// Words (each matches as a prefix). May be empty when another filter is set.
    pub query: String,
    pub author: Option<UserId>,
    pub after: Option<i64>,
    pub before: Option<i64>,
    /// Messages mentioning this user.
    pub mentions: Option<UserId>,
    /// Some(true) only with attachments, Some(false) only without.
    pub has_attachment: Option<bool>,
    pub order: SearchOrder,
    pub offset: u32,
    /// 0 means the default (50); capped at 100.
    pub limit: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorkspaceEvent {
    IdentityRotated {
        public_key: [u8; 32],
        nonce: [u8; 32],
    },
    Read {
        channel: ChannelId,
        message: MessageId,
    },
    Pinned {
        channel: ChannelId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
    },
    Resync {
        snapshot: ServerSnapshot,
    },
    State(WorkspaceSnapshot),
    History {
        channel: ChannelId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        has_more: bool,
    },
    Ack {
        operation: String,
        message: MessageId,
    },
    MessageChanged {
        message: Message,
        details: MessageDetails,
    },
    MessageDeleted {
        channel: ChannelId,
        message: MessageId,
    },
    SearchResults {
        query: String,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
    },
    Uploaded {
        operation: String,
        info: AttachmentInfo,
    },
    Attachment {
        info: AttachmentInfo,
        bytes: Vec<u8>,
    },
    Invite {
        guild: GuildId,
        code: String,
        expires: i64,
    },
    Notice(String),
    Error {
        operation: Option<String>,
        reason: String,
    },
    /// Reply to `SearchAdvanced`; appended last for wire compatibility.
    SearchPage {
        query: String,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        offset: u32,
        has_more: bool,
    },
    /// Broadcast after any category or channel-layout change in `guild`: the complete current
    /// categories and channel placement of that community. Appended last.
    CategoriesChanged {
        guild: GuildId,
        categories: Vec<Category>,
        channel_settings: Vec<ChannelSettings>,
    },
    Community(CommunityEvent),
    /// Bytes of a chunked upload stored so far (appended last for wire compatibility).
    UploadProgress {
        operation: String,
        received: u64,
    },
    AttachmentChunk {
        info: AttachmentInfo,
        offset: u64,
        data: Vec<u8>,
    },
    InboxPage {
        mentions_only: bool,
        before: Option<MessageId>,
        messages: Vec<Message>,
        has_more: bool,
    },
    UploadResumePoint {
        operation: String,
        received: u64,
    },
    /// Transfer bandwidth backpressure; the same offset may be retried after this delay.
    DownloadDeferred {
        attachment: u64,
        offset: u64,
        retry_millis: u16,
    },
    QueryReply {
        id: u64,
        event: Box<QueryResult>,
    },
    ProfileUpdated(Profile),
    DownloadFailed {
        attachment: u64,
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum QueryResult {
    History {
        channel: ChannelId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        has_more: bool,
    },
    SearchPage {
        query: String,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        offset: u32,
        has_more: bool,
    },
    Inbox {
        mentions_only: bool,
        before: Option<MessageId>,
        messages: Vec<Message>,
        has_more: bool,
    },
    Pinned {
        channel: ChannelId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
    },
    Context {
        channel: ChannelId,
        message: MessageId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        has_before: bool,
        has_after: bool,
    },
    After {
        channel: ChannelId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        has_after: bool,
    },
    Error(String),
}
impl QueryResult {
    pub fn from_event(event: WorkspaceEvent) -> Option<Self> {
        Some(match event {
            WorkspaceEvent::History {
                channel,
                messages,
                details,
                has_more,
            } => Self::History {
                channel,
                messages,
                details,
                has_more,
            },
            WorkspaceEvent::SearchPage {
                query,
                messages,
                details,
                offset,
                has_more,
            } => Self::SearchPage {
                query,
                messages,
                details,
                offset,
                has_more,
            },
            WorkspaceEvent::InboxPage {
                mentions_only,
                before,
                messages,
                has_more,
            } => Self::Inbox {
                mentions_only,
                before,
                messages,
                has_more,
            },
            WorkspaceEvent::Pinned {
                channel,
                messages,
                details,
            } => Self::Pinned {
                channel,
                messages,
                details,
            },
            WorkspaceEvent::Community(CommunityEvent::Context {
                channel,
                message,
                messages,
                details,
                has_before,
                has_after,
            }) => Self::Context {
                channel,
                message,
                messages,
                details,
                has_before,
                has_after,
            },
            WorkspaceEvent::Community(CommunityEvent::AfterHistory {
                channel,
                messages,
                details,
                has_after,
            }) => Self::After {
                channel,
                messages,
                details,
                has_after,
            },
            WorkspaceEvent::Error { reason, .. } => Self::Error(reason),
            _ => return None,
        })
    }
    pub fn into_event(self) -> WorkspaceEvent {
        match self {
            Self::History {
                channel,
                messages,
                details,
                has_more,
            } => WorkspaceEvent::History {
                channel,
                messages,
                details,
                has_more,
            },
            Self::SearchPage {
                query,
                messages,
                details,
                offset,
                has_more,
            } => WorkspaceEvent::SearchPage {
                query,
                messages,
                details,
                offset,
                has_more,
            },
            Self::Inbox {
                mentions_only,
                before,
                messages,
                has_more,
            } => WorkspaceEvent::InboxPage {
                mentions_only,
                before,
                messages,
                has_more,
            },
            Self::Pinned {
                channel,
                messages,
                details,
            } => WorkspaceEvent::Pinned {
                channel,
                messages,
                details,
            },
            Self::Context {
                channel,
                message,
                messages,
                details,
                has_before,
                has_after,
            } => WorkspaceEvent::Community(CommunityEvent::Context {
                channel,
                message,
                messages,
                details,
                has_before,
                has_after,
            }),
            Self::After {
                channel,
                messages,
                details,
                has_after,
            } => WorkspaceEvent::Community(CommunityEvent::AfterHistory {
                channel,
                messages,
                details,
                has_after,
            }),
            Self::Error(reason) => WorkspaceEvent::Error {
                operation: None,
                reason,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    Text,
    Voice,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    #[serde(default = "default_guild")]
    pub guild: GuildId,
    pub name: String,
    pub kind: ChannelKind,
    pub topic: String,
}

/// One server (a group of channels) hosted on the shared host.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Guild {
    pub id: GuildId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FriendStatus {
    Friend,
    /// They asked to be your friend.
    Incoming,
    /// You asked and are waiting.
    Outgoing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FriendEntry {
    pub user: UserId,
    pub status: FriendStatus,
}

/// A private one-on-one or group call.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CallInfo {
    pub id: ChannelId,
    pub host: UserId,
    pub members: Vec<UserId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub name: String,
    /// Avatar colour as 0xRRGGBB.
    pub color: u32,
    pub online: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub channel: ChannelId,
    pub author: UserId,
    pub author_name: String,
    pub author_color: u32,
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub text: String,
}

/// Where a person is in voice and what they are doing there.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct VoiceState {
    pub channel: Option<ChannelId>,
    pub muted: bool,
    pub deafened: bool,
    pub camera: bool,
    pub streaming: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamKind {
    Screen,
    Camera,
}

/// Announced information about a live stream.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StreamInfo {
    pub owner: UserId,
    pub kind: StreamKind,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// Everything a newcomer needs to draw the server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerSnapshot {
    pub name: String,
    pub guilds: Vec<Guild>,
    pub friends: Vec<FriendEntry>,
    pub calls: Vec<CallInfo>,
    pub channels: Vec<Channel>,
    pub users: Vec<User>,
    pub messages: Vec<Message>,
    pub voice: Vec<(UserId, VoiceState)>,
    pub streams: Vec<StreamInfo>,
}

pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(feature = "gui")]
pub fn color32(c: u32) -> egui::Color32 {
    egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A palette of friendly avatar colours.
pub const AVATAR_COLORS: [u32; 8] = [
    0x5b6ee1, 0x3ba55d, 0xfaa61a, 0xed4245, 0xeb459e, 0x1abc9c, 0x9b59b6, 0x747f8d,
];

pub type RoleId = u32;
/// Explicit capabilities. The owner/admin legacy roles retain full access.
pub mod permission {
    pub const VIEW: u64 = 1;
    pub const SEND: u64 = 1 << 1;
    pub const ATTACH: u64 = 1 << 2;
    pub const REACT: u64 = 1 << 3;
    pub const CONNECT: u64 = 1 << 4;
    pub const SPEAK: u64 = 1 << 5;
    pub const STREAM: u64 = 1 << 6;
    pub const MANAGE_MESSAGES: u64 = 1 << 7;
    pub const MANAGE_CHANNELS: u64 = 1 << 8;
    pub const MANAGE_ROLES: u64 = 1 << 9;
    pub const MODERATE: u64 = 1 << 10;
    pub const INVITE: u64 = 1 << 11;
    pub const MANAGE_GUILD: u64 = 1 << 12;
    pub const ALL: u64 = (1 << 13) - 1;
    pub const MEMBER: u64 = VIEW | SEND | ATTACH | REACT | CONNECT | SPEAK | STREAM;
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CommunityRole {
    pub id: RoleId,
    pub guild: GuildId,
    pub name: String,
    pub color: u32,
    pub position: i32,
    pub permissions: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RoleAssignment {
    pub guild: GuildId,
    pub user: UserId,
    pub roles: Vec<RoleId>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionScope {
    Channel(ChannelId),
    Category(CategoryId),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionTarget {
    Everyone,
    Role(RoleId),
    Member(UserId),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionOverride {
    pub scope: PermissionScope,
    pub target: PermissionTarget,
    pub allow: u64,
    pub deny: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProfileExtra {
    pub user: UserId,
    pub guild: Option<GuildId>,
    pub bio: String,
    pub nickname: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CommunityVisual {
    pub guild: GuildId,
    pub icon: Vec<u8>,
    pub banner: Vec<u8>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConversationVisual {
    pub channel: ChannelId,
    pub icon: Vec<u8>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelStyle {
    #[default]
    Standard,
    Forum,
    Announcement,
    Stage,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChannelPolicy {
    pub channel: ChannelId,
    pub slowmode_secs: u32,
    pub style: ChannelStyle,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub channel: ChannelId,
    pub parent: ChannelId,
    pub message: Option<MessageId>,
    pub creator: UserId,
    pub archived: bool,
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemberTimeout {
    pub guild: GuildId,
    pub user: UserId,
    pub until: i64,
    pub reason: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: u64,
    pub guild: GuildId,
    pub actor: UserId,
    pub target: Option<UserId>,
    pub action: String,
    pub reason: String,
    pub time: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InviteInfo {
    pub guild: GuildId,
    pub creator: UserId,
    pub code: String,
    pub expires: i64,
    pub remaining: u16,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BanInfo {
    pub guild: GuildId,
    pub user: UserId,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UnreadSummary {
    pub channel: ChannelId,
    pub count: u32,
    pub mentions: u32,
    pub first: Option<MessageId>,
    pub latest: Option<MessageId>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CommunitySnapshot {
    pub roles: Vec<CommunityRole>,
    pub assignments: Vec<RoleAssignment>,
    pub overrides: Vec<PermissionOverride>,
    pub profile_extras: Vec<ProfileExtra>,
    pub visuals: Vec<CommunityVisual>,
    pub conversation_visuals: Vec<ConversationVisual>,
    pub policies: Vec<ChannelPolicy>,
    pub threads: Vec<ThreadInfo>,
    pub timeouts: Vec<MemberTimeout>,
    pub unread: Vec<UnreadSummary>,
    pub polls: Vec<Poll>,
    pub emojis: Vec<CustomEmoji>,
    pub events: Vec<ScheduledEvent>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PollVote {
    pub user: UserId,
    pub choices: Vec<u8>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Poll {
    pub message: MessageId,
    pub channel: ChannelId,
    pub creator: UserId,
    pub question: String,
    pub options: Vec<String>,
    pub votes: Vec<PollVote>,
    pub multiple: bool,
    pub closed: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CustomEmoji {
    pub id: u64,
    pub guild: GuildId,
    pub name: String,
    pub image: Vec<u8>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScheduledEvent {
    pub id: u64,
    pub guild: GuildId,
    pub creator: UserId,
    pub name: String,
    pub description: String,
    pub starts_at: i64,
    pub ends_at: Option<i64>,
    pub channel: Option<ChannelId>,
    pub interested: Vec<UserId>,
    pub cancelled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CommunityCommand {
    UpsertRole {
        role: CommunityRole,
    },
    DeleteRole {
        guild: GuildId,
        role: RoleId,
    },
    AssignRoles {
        guild: GuildId,
        user: UserId,
        roles: Vec<RoleId>,
    },
    SetOverride {
        value: PermissionOverride,
    },
    RemoveOverride {
        scope: PermissionScope,
        target: PermissionTarget,
    },
    UpdateProfileExtra {
        guild: Option<GuildId>,
        bio: String,
        nickname: String,
    },
    UpdateVisual {
        guild: GuildId,
        icon: Vec<u8>,
        banner: Vec<u8>,
    },
    UpdateConversationIcon {
        channel: ChannelId,
        icon: Vec<u8>,
    },
    SetChannelPolicy {
        policy: ChannelPolicy,
    },
    Timeout {
        guild: GuildId,
        user: UserId,
        until: i64,
        reason: String,
    },
    Unban {
        guild: GuildId,
        user: UserId,
    },
    Moderation {
        guild: GuildId,
    },
    Invites {
        guild: GuildId,
    },
    CreateThread {
        parent: ChannelId,
        message: Option<MessageId>,
        name: String,
    },
    SetThreadArchived {
        channel: ChannelId,
        archived: bool,
    },
    SetThreadTags {
        channel: ChannelId,
        tags: Vec<String>,
    },
    MessageContext {
        channel: ChannelId,
        message: MessageId,
        limit: u16,
    },
    Unread,
    MarkUnread {
        channel: ChannelId,
        message: MessageId,
    },
    Forward {
        operation: String,
        message: MessageId,
        target: ChannelId,
        note: String,
    },
    HistoryAfter {
        channel: ChannelId,
        after: MessageId,
        limit: u16,
    },
    CreatePoll {
        operation: String,
        channel: ChannelId,
        question: String,
        options: Vec<String>,
        multiple: bool,
    },
    VotePoll {
        message: MessageId,
        choices: Vec<u8>,
    },
    ClosePoll {
        message: MessageId,
    },
    UpsertEmoji {
        guild: GuildId,
        id: Option<u64>,
        name: String,
        image: Vec<u8>,
    },
    DeleteEmoji {
        guild: GuildId,
        id: u64,
    },
    UpsertEvent {
        event: ScheduledEvent,
    },
    DeleteEvent {
        guild: GuildId,
        id: u64,
    },
    RespondEvent {
        event: u64,
        interested: bool,
    },
    StageRequest {
        channel: ChannelId,
        requested: bool,
    },
    StageSpeaker {
        channel: ChannelId,
        user: UserId,
        allowed: bool,
    },
    StageStatus {
        channel: ChannelId,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CommunityEvent {
    PollUpdated(Poll),
    AfterHistory {
        channel: ChannelId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        has_after: bool,
    },
    MarkedUnread {
        channel: ChannelId,
        message: MessageId,
    },
    Moderation {
        guild: GuildId,
        bans: Vec<BanInfo>,
        timeouts: Vec<MemberTimeout>,
        audit: Vec<AuditEntry>,
    },
    Invites {
        guild: GuildId,
        invites: Vec<InviteInfo>,
    },
    Context {
        channel: ChannelId,
        message: MessageId,
        messages: Vec<Message>,
        details: Vec<MessageDetails>,
        has_before: bool,
        has_after: bool,
    },
    Unread(Vec<UnreadSummary>),
    ThreadCreated {
        parent: ChannelId,
        channel: ChannelId,
    },
    StageState {
        channel: ChannelId,
        speakers: Vec<UserId>,
        requests: Vec<UserId>,
    },
}

#[cfg(test)]
mod reply_tests {
    use super::*;

    #[test]
    fn reply_preview_carries_author_and_excerpt_or_marks_removed() {
        let m = Message {
            id: 9,
            channel: 1,
            author: 3,
            author_name: "Ana".into(),
            author_color: 0,
            time: 0,
            text: "x".repeat(300),
        };
        let p = ReplyPreview::from_original(Some(&m));
        assert_eq!(
            (p.author, p.author_name.as_str(), p.deleted),
            (3, "Ana", false)
        );
        assert_eq!(p.text.chars().count(), REPLY_EXCERPT_CHARS);
        assert!(ReplyPreview::from_original(None).deleted);
    }
}
