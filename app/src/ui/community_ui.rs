//! Community settings use the same capability-checked commands as the host.

use super::{components, theme};

use crate::{app::LanvoxApp, model::*};

use egui::{RichText, Ui};

mod access_panels;
mod event_panel;
mod forum_panel;
mod policy_panels;
mod visual_panels;
pub use access_panels::*;
pub use event_panel::*;
pub use forum_panel::*;
pub use policy_panels::*;
pub use visual_panels::*;

const CAPABILITIES: &[(u64, &str)] = &[
    (permission::VIEW, "Ver canal"),
    (permission::SEND, "Enviar mensagens"),
    (permission::ATTACH, "Anexar arquivos"),
    (permission::REACT, "Adicionar reações"),
    (permission::CONNECT, "Conectar à voz"),
    (permission::SPEAK, "Falar"),
    (permission::STREAM, "Transmitir"),
    (permission::MANAGE_MESSAGES, "Gerenciar mensagens"),
    (permission::MANAGE_CHANNELS, "Gerenciar canais"),
    (permission::MANAGE_ROLES, "Gerenciar cargos"),
    (permission::MODERATE, "Moderar membros"),
    (permission::INVITE, "Criar convites"),
    (permission::MANAGE_GUILD, "Gerenciar comunidade"),
];

fn command(app: &mut LanvoxApp, command: CommunityCommand) {
    app.workspace(WorkspaceCommand::Community(command));
}
