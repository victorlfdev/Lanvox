//! Authorized conversations, community administration and durable messages.
use super::uploads::PendingUpload;
use super::*;
use crate::categories;

mod access;
mod admin_commands;
mod attachment_commands;
mod attachments;
mod conversation_commands;
mod emoji_commands;
mod event_commands;
mod host_commands;
mod membership_commands;
mod message_commands;
mod messages;
mod poll_commands;
mod snapshot;
mod structure_commands;
mod thread_commands;

const MAX_COMMUNITIES: usize = 128;
const MAX_CHANNELS: usize = 2048;
const MAX_MEMBERS: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Invite {
    guild: GuildId,
    creator: UserId,
    code: String,
    expires: i64,
    remaining: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RotationReceipt {
    user: UserId,
    old: [u8; 32],
    new: [u8; 32],
    nonce: [u8; 32],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct WorkspaceData {
    pub memberships: Vec<Membership>,
    pub conversations: Vec<Conversation>,
    pub channel_settings: Vec<ChannelSettings>,
    pub profiles: Vec<Profile>,
    pub details: Vec<MessageDetails>,
    pub read: Vec<(UserId, ChannelId, MessageId)>,
    pub host_owner: Option<UserId>,
    invites: Vec<Invite>,
    bans: Vec<(GuildId, UserId)>,
    next_attachment: u64,
    #[serde(default)]
    upload_operations: Vec<(UserId, String, u64)>,
    #[serde(default)]
    pub(super) initialized: bool,
    #[serde(default)]
    revoked_keys: Vec<[u8; 32]>,
    #[serde(default)]
    rotations: Vec<RotationReceipt>,
    /// Channel categories (schema v3). Order inside a community: `position`.
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    next_category: CategoryId,
    /// Legacy text categories were already converted into `categories`.
    #[serde(default)]
    pub categories_ready: bool,
    #[serde(default)]
    pub community: CommunitySnapshot,
    #[serde(default)]
    audit: Vec<AuditEntry>,
    #[serde(default)]
    last_sends: Vec<(UserId, ChannelId, i64)>,
    #[serde(default)]
    next_role: RoleId,
    #[serde(default)]
    next_emoji: u64,
    #[serde(default)]
    next_event: u64,
}

pub(super) fn random_token() -> String {
    let bytes = crate::auth::challenge().unwrap_or_else(|_| rand::random());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl State {
    pub(super) fn handle_workspace(&mut self, user: UserId, command: WorkspaceCommand) {
        let operation = match &command {
            WorkspaceCommand::Send { operation, .. }
            | WorkspaceCommand::Upload { operation, .. }
            | WorkspaceCommand::UploadBegin { operation, .. }
            | WorkspaceCommand::UploadBeginResumable { operation, .. }
            | WorkspaceCommand::UploadChunk { operation, .. }
            | WorkspaceCommand::UploadFinish { operation } => Some(operation.clone()),
            WorkspaceCommand::Community(
                CommunityCommand::CreatePoll { operation, .. }
                | CommunityCommand::Forward { operation, .. },
            ) => Some(operation.clone()),
            _ => None,
        };
        let download = match &command {
            WorkspaceCommand::Download { attachment }
            | WorkspaceCommand::DownloadChunk { attachment, .. } => Some(*attachment),
            _ => None,
        };
        if let Err(e) = self.workspace_command(user, command) {
            let event = if let Some(attachment) = download {
                WorkspaceEvent::DownloadFailed {
                    attachment,
                    reason: e.to_string(),
                }
            } else {
                WorkspaceEvent::Error {
                    operation,
                    reason: e.to_string(),
                }
            };
            self.send_to(user, &ServerMsg::Workspace(event));
        }
    }
    fn workspace_command(
        &mut self,
        user: UserId,
        command: WorkspaceCommand,
    ) -> std::io::Result<()> {
        use WorkspaceCommand as C;
        match command {
            C::Query { id, command } => {
                require(self.query.is_none(), "Consulta inválida")?;
                self.query = Some((user, id));
                self.handle_workspace(user, command.into_command());
                self.query = None;
                Ok(())
            }
            C::Community(command) => self.community_command(user, command),
            command @ (C::RotateIdentity { .. }
            | C::SetHostAccess { .. }
            | C::Backup
            | C::UpdateProfile { .. }) => self.host_commands(user, command),
            command @ (C::Pinned { .. }
            | C::Refresh
            | C::Send { .. }
            | C::History { .. }
            | C::Edit { .. }
            | C::Delete { .. }
            | C::React { .. }
            | C::Pin { .. }
            | C::Read { .. }
            | C::Search { .. }
            | C::SearchAdvanced(..)
            | C::Inbox { .. }) => self.message_commands(user, command),
            command @ (C::OpenConversation { .. }
            | C::UpdateConversation { .. }
            | C::LeaveConversation { .. }) => self.conversation_commands(user, command),
            command @ (C::Upload { .. }
            | C::Download { .. }
            | C::DownloadChunk { .. }
            | C::UploadBegin { .. }
            | C::UploadBeginResumable { .. }
            | C::UploadChunk { .. }
            | C::UploadFinish { .. }
            | C::UploadAbort { .. }) => self.attachment_commands(user, command),
            command @ (C::UpdateChannel { .. }
            | C::CreateCategory { .. }
            | C::RenameCategory { .. }
            | C::DeleteCategory { .. }
            | C::ReorderCategories { .. }
            | C::MoveChannel { .. }
            | C::CreateChannelIn { .. }
            | C::DeleteChannel { .. }
            | C::UpdateGuild { .. }
            | C::DeleteGuild { .. }) => self.structure_commands(user, command),
            command @ (C::CreateInvite { .. }
            | C::RedeemInvite { .. }
            | C::RevokeInvite { .. }
            | C::SetRole { .. }
            | C::RemoveMember { .. }
            | C::LeaveGuild { .. }) => self.membership_commands(user, command),
        }
    }
    fn community_command(
        &mut self,
        user: UserId,
        command: CommunityCommand,
    ) -> std::io::Result<()> {
        use CommunityCommand as C;
        match command {
            command @ (C::UpsertRole { .. }
            | C::DeleteRole { .. }
            | C::AssignRoles { .. }
            | C::SetOverride { .. }
            | C::RemoveOverride { .. }
            | C::UpdateProfileExtra { .. }
            | C::UpdateVisual { .. }
            | C::UpdateConversationIcon { .. }
            | C::SetChannelPolicy { .. }
            | C::Timeout { .. }
            | C::Unban { .. }
            | C::Moderation { .. }
            | C::Invites { .. }
            | C::StageRequest { .. }
            | C::StageSpeaker { .. }
            | C::StageStatus { .. }) => self.admin_commands(user, command),
            command @ (C::CreateThread { .. }
            | C::SetThreadArchived { .. }
            | C::SetThreadTags { .. }
            | C::MessageContext { .. }
            | C::Unread
            | C::MarkUnread { .. }
            | C::HistoryAfter { .. }
            | C::Forward { .. }) => self.thread_commands(user, command),
            command @ (C::CreatePoll { .. } | C::VotePoll { .. } | C::ClosePoll { .. }) => {
                self.poll_commands(user, command)
            }
            command @ (C::UpsertEmoji { .. } | C::DeleteEmoji { .. }) => {
                self.emoji_commands(user, command)
            }
            command @ (C::UpsertEvent { .. } | C::DeleteEvent { .. } | C::RespondEvent { .. }) => {
                self.event_commands(user, command)
            }
        }
    }
}
impl WorkspaceData {
    /// Converts leftover free-text categories (legacy JSON import, fresh host) once.
    pub(super) fn adopt_legacy_categories(&mut self, channels: &[Channel]) {
        if self.categories_ready {
            return;
        }
        categories::adopt_text_categories(
            channels,
            &mut self.channel_settings,
            &mut self.categories,
            &mut self.next_category,
            true,
        );
        self.categories_ready = true;
    }
}
fn clean(text: &str, max: usize) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(max)
        .collect()
}
fn require(condition: bool, reason: &str) -> std::io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            reason,
        ))
    }
}

fn infos_allowed(state: &State, user: UserId, channel: ChannelId, attachments: bool) -> bool {
    !attachments || state.has_permission(user, channel, permission::ATTACH)
}

fn validate_visual(bytes: &[u8], max_bytes: usize, max_side: u32) -> std::io::Result<()> {
    require(
        bytes.len() <= max_bytes,
        "Imagem excede o limite de tamanho",
    )?;
    if bytes.is_empty() {
        return Ok(());
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(std::io::Error::other)?;
    let (w, h) = reader.into_dimensions().map_err(std::io::Error::other)?;
    require(
        w > 0 && h > 0 && w <= max_side && h <= max_side,
        "Dimensões de imagem inválidas",
    )
}

#[cfg(test)]
mod tests;
