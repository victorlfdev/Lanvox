use super::*;

#[test]
fn polls_are_durable_idempotent_and_enforce_membership_votes_and_closing() {
    let mut st = fixture();
    let mut store = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
    store.import(&st.saved(false), &[]).unwrap();
    st.store = Some(Arc::new(Mutex::new(store)));
    st.workspace_command(
        1,
        WorkspaceCommand::OpenConversation {
            name: String::new(),
            members: vec![2],
            direct: true,
        },
    )
    .unwrap();
    let channel = st.workspace.conversations[0].channel;
    let create = CommunityCommand::CreatePoll {
        operation: "poll-stable".into(),
        channel,
        question: "What should we play?".into(),
        options: vec!["Chess".into(), "Cards".into()],
        multiple: false,
    };
    community(&mut st, 1, create.clone()).unwrap();
    community(&mut st, 1, create).unwrap();
    assert_eq!(st.workspace.community.polls.len(), 1);
    let message = st.workspace.community.polls[0].message;
    assert_eq!(st.page(channel, None, 100).unwrap().len(), 1);
    assert!(community(
        &mut st,
        3,
        CommunityCommand::VotePoll {
            message,
            choices: vec![0]
        }
    )
    .is_err());
    assert!(st.workspace_snapshot(3).community.polls.is_empty());
    assert!(community(
        &mut st,
        2,
        CommunityCommand::VotePoll {
            message,
            choices: vec![0, 1]
        }
    )
    .is_err());
    assert!(community(
        &mut st,
        2,
        CommunityCommand::VotePoll {
            message,
            choices: vec![2]
        }
    )
    .is_err());
    community(
        &mut st,
        2,
        CommunityCommand::VotePoll {
            message,
            choices: vec![0],
        },
    )
    .unwrap();
    community(
        &mut st,
        2,
        CommunityCommand::VotePoll {
            message,
            choices: vec![1],
        },
    )
    .unwrap();
    assert_eq!(st.workspace.community.polls[0].votes.len(), 1);
    assert_eq!(st.workspace.community.polls[0].votes[0].choices, vec![1]);
    assert!(community(&mut st, 2, CommunityCommand::ClosePoll { message }).is_err());
    community(&mut st, 1, CommunityCommand::ClosePoll { message }).unwrap();
    assert!(community(
        &mut st,
        2,
        CommunityCommand::VotePoll {
            message,
            choices: vec![0]
        }
    )
    .is_err());
    let persisted = st
        .store
        .as_ref()
        .unwrap()
        .lock()
        .load_state::<Saved>()
        .unwrap()
        .unwrap();
    assert!(persisted.workspace.community.polls[0].closed);
    assert_eq!(persisted.workspace.community.polls[0].votes[0].user, 2);
    st.workspace_command(1, WorkspaceCommand::Delete { message })
        .unwrap();
    assert!(st.workspace.community.polls.is_empty());
}

#[test]
fn multiple_choice_polls_validate_options_and_allow_withdrawing_votes() {
    let mut st = fixture();
    assert!(community(
        &mut st,
        2,
        CommunityCommand::CreatePoll {
            operation: "duplicate".into(),
            channel: 1,
            question: "Question".into(),
            options: vec!["Same".into(), "same".into()],
            multiple: true
        }
    )
    .is_err());
    community(
        &mut st,
        2,
        CommunityCommand::CreatePoll {
            operation: "multi".into(),
            channel: 1,
            question: "Choose".into(),
            options: vec!["One".into(), "Two".into(), "Three".into()],
            multiple: true,
        },
    )
    .unwrap();
    let message = st.workspace.community.polls[0].message;
    community(
        &mut st,
        3,
        CommunityCommand::VotePoll {
            message,
            choices: vec![2, 0, 2],
        },
    )
    .unwrap();
    assert_eq!(st.workspace.community.polls[0].votes[0].choices, vec![0, 2]);
    community(
        &mut st,
        3,
        CommunityCommand::VotePoll {
            message,
            choices: Vec::new(),
        },
    )
    .unwrap();
    assert!(st.workspace.community.polls[0].votes.is_empty());
}
