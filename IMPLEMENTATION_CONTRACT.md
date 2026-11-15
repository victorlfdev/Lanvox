# Contrato compartilhado de implementação

O backend mantém os structs existentes. A extensão usa `ClientMsg::Workspace(WorkspaceCommand)` e `ServerMsg::Workspace(WorkspaceEvent)`; os enums e structs completos estão em `app/src/model.rs`.

- `WorkspaceCommand::Send { operation: String, channel, text, reply_to: Option<MessageId>, attachments: Vec<u64> }`: operação estável por envio. Reenvio do mesmo operation devolve o mesmo ACK sem duplicar. ACK somente após persistência.
- `History { channel, before: Option<MessageId>, limit: u16 }`, limitado a 100. Evento `History { channel, messages, details, has_more }`, mensagens em ordem crescente.
- `OpenConversation { name, members, direct }`: cria ou reutiliza DM. Conversas são canais textuais `guild=0`; eventos ChannelCreated/Welcome incluem somente canais autorizados. `WorkspaceSnapshot.conversations` fornece membros e nomes. `UpdateConversation` e `LeaveConversation` administram grupos.
- `Refresh`: envia `WorkspaceEvent::State(WorkspaceSnapshot)` e o servidor envia este estado após Welcome.
- `State` inclui conversations, memberships, channel_settings, profiles, details, read e host_owner.
- `MessageChanged { message, details }`, `MessageDeleted {channel,message}`; detalhes por ID contêm reply_to, edited, attachments, reactions, mentions, pinned.
- `Edit`, `Delete`, `React`, `Pin`, `Read`, `Search`: ver variantes exatas. `SearchResults {query,messages,details}`.
- `Upload {operation,channel,name,media_type,bytes}` até 8 MiB; `Uploaded {operation,info}`; incluir info.id no Send. `Download {attachment}` retorna `Attachment {info,bytes}` após autorização.
- `UpdateProfile {status,presence,avatar}`: Presence Online/Away/DoNotDisturb/Invisible; avatar limitado e validado.
- `UpdateChannel {channel,name,topic,category,position,allowed}`; allowed vazio = membros da comunidade. `DeleteChannel`, `UpdateGuild`, `DeleteGuild`.
- `CreateInvite {guild,max_uses,expires_hours}` retorna `Invite {guild,code,expires}`; `RedeemInvite {code}`, `RevokeInvite {code}`.
- `SetRole {guild,user,role}` (MemberRole Owner/Admin/Member), `RemoveMember {guild,user,ban}`, `LeaveGuild`, `Backup`.
- Falha por `WorkspaceEvent::Error {operation: Option<String>,reason}`; avisos por `Notice(String)`.

Os arquivos model.rs/protocol.rs/server.rs pertencem ao agente backend; frontend não deve editá-los. Mudanças adicionais de transporte devem ser coordenadas.

Atualizações integradas:

- `WorkspaceCommand::Pinned { channel }` consulta até 100 mensagens fixadas no armazenamento, com autorização; retorna `WorkspaceEvent::Pinned { channel, messages, details }`.
- Leitura confirma com `WorkspaceEvent::Read { channel, message }`, sem retransmitir avatares e detalhes do workspace.
- `WorkspaceCommand::RotateIdentity { old_public_key, new_public_key, nonce, old_signature, new_signature }` exige duas assinaturas Ed25519 do transcript público `identity_rotation_transcript(user, old, new, nonce)`. As chaves e nonce são `[u8;32]`; assinaturas são `Vec<u8>` de 64 bytes. Confirmação `WorkspaceEvent::IdentityRotated { public_key, nonce }` somente após commit durável. O último receipt por usuário permite repetir o mesmo comando sem outra rotação.
- O cliente precisa salvar a chave nova pendente de forma protegida antes de solicitar rotação, manter a anterior até confirmação e recuperar a conexão usando a pendente se a resposta se perder. As identidades locais são específicas por host.
- `WorkspaceCommand::SetHostAccess { user, allowed }` é exclusivo do proprietário do host. Desabilitar revoga a chave persistida e fecha a sessão. `WorkspaceSnapshot.disabled_users` é informado apenas ao proprietário. `ServerOptions.revoked_keys` tem prioridade sobre owner/allowlist/token/identidade conhecida e não pode ser revertido pela UI.
- Protocolo e descoberta são v3 (`LANVOX3`), conexão de controle Noise cifrada e mídia em datagramas AEAD. `MediaRouter` encaminha mídia sem adquirir o mutex de persistência e mantém orçamento por usuário entre atualizações de autorização.
