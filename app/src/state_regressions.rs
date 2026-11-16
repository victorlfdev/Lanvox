//! Regression coverage for contiguous history windows.
use super::*;

fn message(id: u64) -> Message {
    Message {
        id,
        channel: 7,
        author: 2,
        author_name: "Test".into(),
        author_color: 0,
        time: 0,
        text: format!("message {id}"),
    }
}

fn view() -> ServerView {
    ServerView::from_snapshot(
        1,
        ServerSnapshot {
            name: "test".into(),
            guilds: vec![],
            friends: vec![],
            calls: vec![],
            channels: vec![Channel {
                id: 7,
                guild: 0,
                name: "test".into(),
                kind: ChannelKind::Text,
                topic: String::new(),
            }],
            users: vec![],
            messages: vec![],
            voice: vec![],
            streams: vec![],
        },
    )
}

#[test]
fn older_pages_record_the_evicted_tail_for_forward_pagination() {
    let mut v = view();
    v.messages.insert(7, (101..=700).map(message).collect());
    v.apply_workspace(&WorkspaceEvent::History {
        channel: 7,
        messages: (1..=100).map(message).collect(),
        details: vec![],
        has_more: false,
    });
    assert_eq!(v.messages[&7].last().unwrap().id, 600);
    assert!(!v.history_more[&7]);
    assert!(v.history_after.contains(&7));
    assert!(!v.messages[&7].iter().any(|m| m.id == 700));
}

#[test]
fn live_messages_do_not_create_holes_in_a_historical_window() {
    let mut v = view();
    v.messages.insert(7, (1..=600).map(message).collect());
    v.history_after.insert(7);
    v.apply_workspace(&WorkspaceEvent::MessageChanged {
        message: message(1000),
        details: MessageDetails {
            message: 1000,
            ..Default::default()
        },
    });
    assert_eq!(v.messages[&7].len(), 600);
    assert_eq!(v.messages[&7][599].id, 600);
    assert!(!v.messages[&7].iter().any(|m| m.id == 1000));
}

#[test]
fn authoritative_deletion_summary_clears_the_unread_badge() {
    let mut v = view();
    v.apply(ServerMsg::Message(message(1)), None);
    v.apply_workspace(&WorkspaceEvent::MessageDeleted {
        channel: 7,
        message: 1,
    });
    assert!(v.messages[&7].is_empty());
    v.apply_workspace(&WorkspaceEvent::Community(CommunityEvent::Unread(vec![
        UnreadSummary {
            channel: 7,
            count: 0,
            mentions: 0,
            first: None,
            latest: None,
        },
    ])));
    assert!(!v.unread.contains_key(&7));
}
