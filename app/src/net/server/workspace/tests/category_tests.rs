use super::*;

pub(super) fn layout_of(st: &State) -> Vec<(Option<String>, Vec<ChannelId>)> {
    categories::layout(
        1,
        &st.channels,
        &st.workspace.channel_settings,
        &st.workspace.categories,
    )
    .into_iter()
    .map(|s| (s.category.map(|c| c.name), s.channels))
    .collect()
}

#[test]
fn category_commands_need_the_channel_management_permission() {
    let mut st = fixture();
    let channel = st.channels.iter().find(|c| c.guild == 1).unwrap().id;
    st.workspace_command(
        1,
        WorkspaceCommand::CreateCategory {
            guild: 1,
            name: "Base".into(),
        },
    )
    .unwrap();
    let id = st.workspace.categories[0].id;
    for command in [
        WorkspaceCommand::CreateCategory {
            guild: 1,
            name: "x".into(),
        },
        WorkspaceCommand::RenameCategory {
            category: id,
            name: "x".into(),
        },
        WorkspaceCommand::DeleteCategory { category: id },
        WorkspaceCommand::ReorderCategories {
            guild: 1,
            order: vec![id],
        },
        WorkspaceCommand::MoveChannel {
            channel,
            category: Some(id),
            index: 0,
        },
        WorkspaceCommand::CreateChannelIn {
            guild: 1,
            name: "x".into(),
            kind: ChannelKind::Text,
            category: Some(id),
        },
    ] {
        assert!(
            st.workspace_command(2, command).is_err(),
            "a plain member must be refused"
        );
    }
    assert_eq!(st.workspace.categories.len(), 1);
    assert_eq!(st.workspace.categories[0].name, "Base");
    // Bad input from an administrator is refused too.
    for command in [
        WorkspaceCommand::CreateCategory {
            guild: 1,
            name: "  ".into(),
        },
        WorkspaceCommand::CreateCategory {
            guild: 99,
            name: "x".into(),
        },
        WorkspaceCommand::RenameCategory {
            category: 99,
            name: "x".into(),
        },
        WorkspaceCommand::ReorderCategories {
            guild: 1,
            order: vec![99],
        },
        WorkspaceCommand::MoveChannel {
            channel,
            category: Some(99),
            index: 0,
        },
        WorkspaceCommand::MoveChannel {
            channel: 9999,
            category: None,
            index: 0,
        },
    ] {
        assert!(st.workspace_command(1, command).is_err());
    }
}

#[test]
fn categories_mix_kinds_reorder_move_and_delete_without_losing_channels() {
    let mut st = fixture();
    let all: Vec<ChannelId> = st
        .channels
        .iter()
        .filter(|c| c.guild == 1)
        .map(|c| c.id)
        .collect();
    for name in ["A", "B"] {
        st.workspace_command(
            1,
            WorkspaceCommand::CreateCategory {
                guild: 1,
                name: name.into(),
            },
        )
        .unwrap();
    }
    let (a, b) = (st.workspace.categories[0].id, st.workspace.categories[1].id);
    st.workspace_command(
        1,
        WorkspaceCommand::CreateChannelIn {
            guild: 1,
            name: "voz".into(),
            kind: ChannelKind::Voice,
            category: Some(a),
        },
    )
    .unwrap();
    let voice = st.channels.last().unwrap().id;
    st.workspace_command(
        1,
        WorkspaceCommand::CreateChannelIn {
            guild: 1,
            name: "texto".into(),
            kind: ChannelKind::Text,
            category: Some(a),
        },
    )
    .unwrap();
    let text = st.channels.last().unwrap().id;
    // text and voice share one category, in creation order
    assert_eq!(layout_of(&st)[1], (Some("A".into()), vec![voice, text]));
    // move a channel in front, across categories
    st.workspace_command(
        1,
        WorkspaceCommand::MoveChannel {
            channel: text,
            category: Some(a),
            index: 0,
        },
    )
    .unwrap();
    assert_eq!(layout_of(&st)[1].1, vec![text, voice]);
    st.workspace_command(
        1,
        WorkspaceCommand::MoveChannel {
            channel: voice,
            category: Some(b),
            index: 5,
        },
    )
    .unwrap();
    assert_eq!(layout_of(&st)[1].1, vec![text]);
    assert_eq!(layout_of(&st)[2], (Some("B".into()), vec![voice]));
    // categories reorder and rename
    st.workspace_command(
        1,
        WorkspaceCommand::ReorderCategories {
            guild: 1,
            order: vec![b, a],
        },
    )
    .unwrap();
    st.workspace_command(
        1,
        WorkspaceCommand::RenameCategory {
            category: a,
            name: "  Renomeada ".into(),
        },
    )
    .unwrap();
    let names: Vec<_> = layout_of(&st).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        vec![None, Some("B".into()), Some("Renomeada".into())]
    );
    // deleting keeps every channel, now uncategorised at the top
    st.workspace_command(1, WorkspaceCommand::DeleteCategory { category: b })
        .unwrap();
    let layout = layout_of(&st);
    assert_eq!(layout.len(), 2);
    assert!(layout[0].1.contains(&voice));
    let mut placed: Vec<_> = layout.iter().flat_map(|(_, c)| c.clone()).collect();
    placed.sort();
    let mut expected = all;
    expected.extend([voice, text]);
    expected.sort();
    assert_eq!(placed, expected);
    // ids are not reused
    st.workspace_command(
        1,
        WorkspaceCommand::CreateCategory {
            guild: 1,
            name: "C".into(),
        },
    )
    .unwrap();
    assert!(st.workspace.categories.iter().all(|c| c.id != b));
    // the category state survives a save and load of the host blob
    let saved: Saved =
        serde_json::from_slice(&serde_json::to_vec(&st.saved(false)).unwrap()).unwrap();
    assert_eq!(saved.workspace.categories, st.workspace.categories);
    assert!(saved.workspace.channel_settings == st.workspace.channel_settings);
}

#[test]
fn category_changes_are_broadcast_to_members_and_in_the_snapshot() {
    let mut st = fixture();
    let (one, mut peer_one) = pipe();
    let (two, mut peer_two) = pipe();
    st.clients.insert(1, one);
    st.clients.insert(2, two);
    st.workspace_command(
        1,
        WorkspaceCommand::CreateCategory {
            guild: 1,
            name: "Jogos".into(),
        },
    )
    .unwrap();
    for peer in [&mut peer_one, &mut peer_two] {
        let (guild, categories) = loop {
            if let ServerMsg::Workspace(WorkspaceEvent::CategoriesChanged {
                guild,
                categories,
                ..
            }) = read_packet::<_, ServerMsg>(peer).unwrap()
            {
                break (guild, categories);
            }
        };
        assert_eq!(
            (guild, categories.len(), categories[0].name.as_str()),
            (1, 1, "Jogos")
        );
    }
    let snapshot = st.workspace_snapshot(2);
    assert_eq!(snapshot.categories.len(), 1);
    // The wire format (bincode, positional) round-trips the new shapes.
    let bytes = bincode::serialize(&snapshot).unwrap();
    let back: WorkspaceSnapshot = bincode::deserialize(&bytes).unwrap();
    assert_eq!(back.categories, snapshot.categories);
    let command = WorkspaceCommand::MoveChannel {
        channel: 1,
        category: Some(2),
        index: 3,
    };
    let bytes = bincode::serialize(&command).unwrap();
    assert!(matches!(
        bincode::deserialize::<WorkspaceCommand>(&bytes).unwrap(),
        WorkspaceCommand::MoveChannel {
            channel: 1,
            category: Some(2),
            index: 3
        }
    ));
}
