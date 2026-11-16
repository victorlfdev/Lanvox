//! Client interaction regressions, with demo data and no network/capture devices.
use super::*;

pub(crate) fn fixture() -> (LanvoxApp, egui::Context) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = LanvoxApp::new(&cc, Some(PathBuf::from("unused-test.png")));
    app.screenshot = None;
    app.device_cache = Some((Vec::new(), Vec::new(), Vec::new()));
    (app, ctx)
}

fn frame(app: &mut LanvoxApp, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    frame_at_size(app, ctx, events, egui::vec2(1280.0, 720.0))
}

fn frame_at_size(
    app: &mut LanvoxApp,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    size: egui::Vec2,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ui| crate::ui::root(app, ui),
    );
    output.textures_delta.clear();
    output
}

#[test]
fn forum_creation_remains_reachable_by_scrolling_at_the_minimum_scaled_size() {
    let (mut app, ctx) = fixture();
    app.select_channel(20);
    let size = egui::vec2(940.0 / 1.4, 500.0 / 1.4);
    for _ in 0..5 {
        let _ = frame_at_size(&mut app, &ctx, vec![], size);
    }
    let output = frame_at_size(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(egui::pos2(500.0, 200.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -160.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        size,
    );
    drop(output);
    for _ in 0..8 {
        let _ = frame_at_size(&mut app, &ctx, vec![], size);
    }
    let output = frame_at_size(&mut app, &ctx, vec![], size);
    let create = text_center(&output.shapes, "Criar discussão")
        .expect("Creation action must remain rendered");
    assert!(
        ctx.content_rect().contains(create),
        "Creation button is outside the viewport: {create:?}"
    );
}

fn text_center(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<egui::Pos2> {
    fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
            _ => None,
        }
    }
    shapes.iter().rev().find_map(|s| find(&s.shape, label))
}

fn click_text(app: &mut LanvoxApp, ctx: &egui::Context, label: &str) {
    for _ in 0..3 {
        let _ = frame(app, ctx, vec![]);
    }
    let output = frame(app, ctx, vec![]);
    let pos =
        text_center(&output.shapes, label).unwrap_or_else(|| panic!("Missing UI control: {label}"));
    for pressed in [true, false] {
        let _ = frame(
            app,
            ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn navigation_clears_response_and_restores_destination_read_position() {
    let (mut app, _) = fixture();
    app.workspace_ui.reply_to = Some(1);
    app.workspace_ui.pinned_only = true;
    app.workspace_ui.editing = Some((1, "pending edit".into()));
    app.workspace_ui.reading_positions.insert(2, 42);
    app.select_channel(2);
    assert_eq!(app.selected, Some(2));
    assert_eq!(app.workspace_ui.reply_to, None);
    assert_eq!(app.workspace_ui.editing, None);
    assert!(!app.workspace_ui.pinned_only);
    assert_eq!(app.workspace_ui.history_anchor, Some(42));
    assert!(!app.chat_at_bottom);
}

#[test]
fn voice_preview_survives_navigation_and_is_scoped_to_identity_host_and_channel() {
    let (mut app, _) = fixture();
    app.voice_note = Some(VoiceNote {
        identity: app.local_identity.clone(),
        host: app.current_host(),
        channel: 1,
        recorder: None,
        bytes: Some(b"RIFF private preview".to_vec()),
        preview: None,
    });
    app.select_channel(2);
    assert!(app.voice_note.is_none());
    app.restore_voice_note(2);
    assert!(app.voice_note.is_none());
    app.select_channel(1);
    app.restore_voice_note(1);
    assert_eq!(
        app.voice_note.as_ref().unwrap().bytes.as_deref(),
        Some(b"RIFF private preview".as_slice())
    );
    app.preserve_voice_note();
    app.local_identity = "another-identity".into();
    app.restore_voice_note(1);
    assert!(app.voice_note.is_none());
    assert_ne!(
        super::voice_notes::draft_key("a", "host:47800", 1),
        super::voice_notes::draft_key("a", "other:47800", 1)
    );
}

#[test]
fn voice_attachment_failure_does_not_consume_the_preview() {
    let (mut app, ctx) = fixture();
    let bytes = vec![1; 100];
    app.voice_note = Some(VoiceNote {
        identity: app.local_identity.clone(),
        host: app.current_host(),
        channel: 1,
        recorder: None,
        bytes: Some(bytes.clone()),
        preview: None,
    });
    app.workspace_ui.file_read_reserved = 2 * MAX_ATTACHMENT_BYTES;
    click_text(&mut app, &ctx, "Anexar gravação");
    assert_eq!(
        app.voice_note.as_ref().unwrap().bytes.as_ref(),
        Some(&bytes)
    );
    assert!(!app.workspace_ui.staged.contains_key(&1));
}

#[test]
fn opening_another_community_resets_confirmation_without_changing_the_conversation() {
    let (mut app, _) = fixture();
    app.workspace_ui.delete_guild_confirm = Some(1);
    app.open_guild_settings(Some(2), 6);
    assert_eq!(app.selected, Some(1));
    assert_eq!(app.workspace_ui.management_guild, Some(2));
    assert_eq!(app.workspace_ui.extras.admin_tab, 6);
    assert_eq!(app.workspace_ui.delete_guild_confirm, None);
}

#[test]
fn management_renders_the_requested_community_even_with_another_channel_open() {
    let (mut app, ctx) = fixture();
    let name = app
        .view
        .as_ref()
        .unwrap()
        .guilds
        .iter()
        .find(|g| g.id == 2)
        .unwrap()
        .name
        .clone();
    app.open_guild_settings(Some(2), 6);
    for _ in 0..3 {
        let _ = frame(&mut app, &ctx, vec![]);
    }
    let output = frame(&mut app, &ctx, vec![]);
    assert!(text_center(&output.shapes, &format!("Configurações · {name}")).is_some());
    assert_eq!(app.workspace_ui.extras.admin_guild, Some(2));
    assert_eq!(app.workspace_ui.extras.admin_tab, 6);
}

#[test]
fn search_load_more_appends_and_deduplicates_without_skipping_the_next_offset() {
    let (mut app, _) = fixture();
    app.workspace_ui.search_submitted.query = "game".into();
    let message = |id| Message {
        id,
        channel: 1,
        author: 2,
        author_name: "Maya".into(),
        author_color: 0,
        time: 0,
        text: "game".into(),
    };
    for (offset, ids) in [(0, vec![100, 101]), (2, vec![101, 102])] {
        app.on_workspace_event(WorkspaceEvent::SearchPage {
            query: "game".into(),
            messages: ids.into_iter().map(message).collect(),
            details: vec![],
            offset,
            has_more: true,
        });
    }
    assert_eq!(
        app.workspace_ui
            .search_results
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![100, 101, 102]
    );
    assert_eq!(app.workspace_ui.search_offset, 4);
    app.on_workspace_event(WorkspaceEvent::SearchPage {
        query: "stale query".into(),
        messages: vec![message(999)],
        details: vec![],
        offset: 0,
        has_more: false,
    });
    assert_eq!(app.workspace_ui.search_results.len(), 3);
}

#[test]
fn forum_failure_preserves_both_title_and_body() {
    let (mut app, _) = fixture();
    app.workspace_ui.forum_pending_draft = Some((
        20,
        crate::ui::workspace::ForumDraft {
            title: "Which game?".into(),
            body: "A long first message".into(),
        },
    ));
    app.on_workspace_event(WorkspaceEvent::Error {
        operation: None,
        reason: "Not allowed".into(),
    });
    let (_, recovered) = app.workspace_ui.forum_failed_draft.as_ref().unwrap();
    assert_eq!(recovered.title, "Which game?");
    assert_eq!(recovered.body, "A long first message");
    assert_eq!(
        app.workspace_ui.operation_errors.front().unwrap(),
        "Not allowed"
    );
}

#[test]
fn changing_settings_tab_stops_the_microphone_test() {
    let (mut app, ctx) = fixture();
    app.modal = Modal::Settings(SettingsTab::Voice);
    app.audio_shared.mic_test.store(true, Ordering::Relaxed);
    click_text(&mut app, &ctx, "Aparência");
    assert!(matches!(
        app.modal,
        Modal::Settings(SettingsTab::Appearance)
    ));
    assert!(!app.audio_shared.mic_test.load(Ordering::Relaxed));
}

#[test]
fn event_edit_expands_the_editor_and_open_channel_closes_management() {
    let (mut app, ctx) = fixture();
    app.open_guild_settings(Some(1), 8);
    click_text(&mut app, &ctx, "Editar");
    assert_ne!(app.workspace_ui.extras.event.id, 0);
    let output = frame(&mut app, &ctx, vec![]);
    assert!(text_center(&output.shapes, "NOME").is_some());
    let (mut app, ctx) = fixture();
    app.open_guild_settings(Some(1), 8);
    click_text(&mut app, &ctx, "Abrir canal");
    assert!(!app.workspace_ui.managing);
}

#[test]
fn errors_persist_independently_of_success_notices() {
    let (mut app, _) = fixture();
    app.notify_error("Permission denied");
    app.notify("Copied");
    app.notify("Saved");
    assert_eq!(
        app.workspace_ui.operation_errors.front().unwrap(),
        "Permission denied"
    );
    assert_eq!(app.notice.as_ref().unwrap().0, "Copied");
    assert_eq!(app.notice_queue.front().unwrap(), "Saved");
}

#[test]
fn invalid_date_filter_blocks_search_even_when_filters_are_collapsed() {
    let (mut app, ctx) = fixture();
    app.workspace_ui.search_open = true;
    let after_key = format!(
        "{}|{}|search-after",
        app.local_identity,
        crate::settings::host_address(&app.current_host())
    );
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::unique(after_key),
            (None::<i64>, "invalid date".to_owned()),
        )
    });
    click_text(&mut app, &ctx, "Buscar");
    assert!(!app.workspace_ui.search_loading);
    assert!(!app.workspace_ui.queries.contains_key("search"));
    app.reconnect_address = Some("other-host:47800".into());
    click_text(&mut app, &ctx, "Buscar");
    assert!(
        app.workspace_ui.search_loading,
        "An invalid date from another host must not block this search"
    );
}

#[test]
fn closing_community_settings_clears_the_destructive_confirmation() {
    let (mut app, ctx) = fixture();
    app.open_guild_settings(Some(1), 0);
    app.workspace_ui.delete_guild_confirm = Some(1);
    click_text(&mut app, &ctx, "Fechar");
    assert!(!app.workspace_ui.managing);
    assert_eq!(app.workspace_ui.delete_guild_confirm, None);
    app.open_guild_settings(Some(2), 0);
    assert_eq!(app.workspace_ui.delete_guild_confirm, None);
}

#[test]
fn action_colors_and_member_text_are_readable() {
    use crate::ui::theme;
    assert!(theme::contrast_ratio(egui::Color32::WHITE, theme::ACTION_FILL) >= 4.5);
    assert!(theme::contrast_ratio(theme::TEXT, theme::ACTION_FILL) >= 4.5);
    assert!(theme::contrast_ratio(egui::Color32::WHITE, theme::RED) >= 4.5);
    for color in AVATAR_COLORS {
        let color = color32(color);
        assert!(theme::contrast_ratio(theme::on_color(color), color) >= 4.5);
        for background in [
            theme::CHAT,
            theme::SIDEBAR,
            egui::Color32::from_rgb(54, 57, 63),
        ] {
            assert!(
                theme::contrast_ratio(theme::readable_text(color, background), background) >= 4.5
            );
        }
    }
}
