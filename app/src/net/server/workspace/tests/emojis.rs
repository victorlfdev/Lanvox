use super::*;

pub(super) fn emoji_png(side: u32) -> Vec<u8> {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        side,
        side,
        image::Rgba([80, 100, 255, 255]),
    ));
    let mut output = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

#[test]
fn custom_emojis_validate_membership_images_names_limits_and_reactions() {
    let mut st = fixture();
    let image = emoji_png(16);
    assert!(community(
        &mut st,
        2,
        CommunityCommand::UpsertEmoji {
            guild: 1,
            id: None,
            name: "smile".into(),
            image: image.clone()
        }
    )
    .is_err());
    assert!(community(
        &mut st,
        1,
        CommunityCommand::UpsertEmoji {
            guild: 1,
            id: None,
            name: "bad/name".into(),
            image: image.clone()
        }
    )
    .is_err());
    assert!(community(
        &mut st,
        1,
        CommunityCommand::UpsertEmoji {
            guild: 1,
            id: None,
            name: "oversize".into(),
            image: emoji_png(129)
        }
    )
    .is_err());
    assert!(community(
        &mut st,
        1,
        CommunityCommand::UpsertEmoji {
            guild: 1,
            id: None,
            name: "invalid".into(),
            image: vec![1, 2, 3]
        }
    )
    .is_err());
    for id in 0..64 {
        community(
            &mut st,
            1,
            CommunityCommand::UpsertEmoji {
                guild: 1,
                id: None,
                name: format!("emoji_{id}"),
                image: image.clone(),
            },
        )
        .unwrap();
    }
    assert!(community(
        &mut st,
        1,
        CommunityCommand::UpsertEmoji {
            guild: 1,
            id: None,
            name: "too_many".into(),
            image: image.clone()
        }
    )
    .is_err());
    let emoji = st.workspace.community.emojis[0].id;
    st.durable_send(1, None, 1, "message".into(), None, vec![])
        .unwrap();
    let message = st.messages.last().unwrap().id;
    st.workspace_command(
        2,
        WorkspaceCommand::React {
            message,
            emoji: format!("custom:{emoji}"),
        },
    )
    .unwrap();
    community(
        &mut st,
        1,
        CommunityCommand::DeleteEmoji {
            guild: 1,
            id: emoji,
        },
    )
    .unwrap();
    assert!(st
        .workspace_command(
            3,
            WorkspaceCommand::React {
                message,
                emoji: format!("custom:{emoji}")
            }
        )
        .is_err());
    st.workspace_command(
        2,
        WorkspaceCommand::React {
            message,
            emoji: format!("custom:{emoji}"),
        },
    )
    .unwrap();
    assert!(st.details_for(&[st.find_message(message).unwrap()])[0]
        .reactions
        .is_empty());
}
