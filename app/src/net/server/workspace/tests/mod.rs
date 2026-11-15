use super::*;

mod access;
mod category_tests;
mod emojis;
mod events;
mod identity;
mod messages;
mod polls;
mod uploads;

pub(super) fn fixture() -> State {
    let mut st = super::super::tests::fixture();
    for id in 1..=3 {
        st.users.insert(
            id,
            User {
                id,
                name: format!("Pessoa {id}"),
                color: 0,
                online: true,
            },
        );
        st.public_keys.insert(id, [id as u8; 32]);
    }
    st.next_user = 4;
    st.initialize_memberships();
    st.set_relation(1, 2, Some(FriendStatus::Friend));
    st
}

pub(super) fn pipe() -> (crate::net::outbox::Outbox, std::net::TcpStream) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let (server, _) = listener.accept().unwrap();
    (crate::net::outbox::Outbox::start(server), peer)
}

pub(super) fn community(
    st: &mut State,
    actor: UserId,
    command: CommunityCommand,
) -> std::io::Result<()> {
    st.workspace_command(actor, WorkspaceCommand::Community(command))
}
