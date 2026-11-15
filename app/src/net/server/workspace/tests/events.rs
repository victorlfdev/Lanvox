use super::*;

#[test]
fn scheduled_events_preserve_creator_and_rsvp_and_respect_private_channel_access() {
    let mut st = fixture();
    let now = now_secs();
    let event = ScheduledEvent {
        guild: 1,
        name: "Game night".into(),
        description: "Bring friends".into(),
        starts_at: now + 3600,
        ends_at: Some(now + 7200),
        channel: Some(3),
        ..Default::default()
    };
    assert!(community(
        &mut st,
        2,
        CommunityCommand::UpsertEvent {
            event: event.clone()
        }
    )
    .is_err());
    community(&mut st, 1, CommunityCommand::UpsertEvent { event }).unwrap();
    let id = st.workspace.community.events[0].id;
    community(
        &mut st,
        2,
        CommunityCommand::RespondEvent {
            event: id,
            interested: true,
        },
    )
    .unwrap();
    let mut edited = st.workspace.community.events[0].clone();
    edited.creator = 3;
    edited.interested = vec![3];
    edited.name = "Renamed".into();
    community(&mut st, 1, CommunityCommand::UpsertEvent { event: edited }).unwrap();
    assert_eq!(st.workspace.community.events[0].creator, 1);
    assert_eq!(st.workspace.community.events[0].interested, vec![2]);
    community(
        &mut st,
        1,
        CommunityCommand::SetOverride {
            value: PermissionOverride {
                scope: PermissionScope::Channel(3),
                target: PermissionTarget::Member(3),
                allow: 0,
                deny: permission::VIEW,
            },
        },
    )
    .unwrap();
    assert!(st.workspace_snapshot(3).community.events.is_empty());
    assert!(community(
        &mut st,
        3,
        CommunityCommand::RespondEvent {
            event: id,
            interested: true
        }
    )
    .is_err());
    community(
        &mut st,
        2,
        CommunityCommand::RespondEvent {
            event: id,
            interested: false,
        },
    )
    .unwrap();
    assert!(st.workspace.community.events[0].interested.is_empty());
    assert!(community(&mut st, 2, CommunityCommand::DeleteEvent { guild: 1, id }).is_err());
    community(&mut st, 1, CommunityCommand::DeleteEvent { guild: 1, id }).unwrap();
    assert!(st.workspace.community.events.is_empty());
}
