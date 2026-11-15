//! Sample content shown when the app runs for a screenshot check, so the
//! picture shows the real layout without needing a network.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::app::LanvoxApp;
use crate::model::*;
use crate::state::ServerView;

pub fn fill(app: &mut LanvoxApp) {
    let now = now_secs();
    let users = vec![
        User {
            id: 1,
            name: app.settings.name.clone(),
            color: app.settings.color,
            online: true,
        },
        User {
            id: 2,
            name: "Maya".into(),
            color: 0x3ba55d,
            online: true,
        },
        User {
            id: 3,
            name: "Tomás".into(),
            color: 0xfaa61a,
            online: true,
        },
        User {
            id: 4,
            name: "Priya".into(),
            color: 0xeb459e,
            online: true,
        },
        User {
            id: 5,
            name: "Jonah".into(),
            color: 0x1abc9c,
            online: false,
        },
        User {
            id: 6,
            name: "Elena".into(),
            color: 0x9b59b6,
            online: false,
        },
    ];
    let channels = vec![
        Channel {
            id: 1,
            guild: 1,
            name: "geral".into(),
            kind: ChannelKind::Text,
            topic: "Encontro dos amigos".into(),
        },
        Channel {
            id: 2,
            guild: 1,
            name: "clipes-e-links".into(),
            kind: ChannelKind::Text,
            topic: String::new(),
        },
        Channel {
            id: 3,
            guild: 1,
            name: "Sala dos amigos".into(),
            kind: ChannelKind::Voice,
            topic: String::new(),
        },
        Channel {
            id: 4,
            guild: 1,
            name: "Jogos".into(),
            kind: ChannelKind::Voice,
            topic: String::new(),
        },
    ];
    let lines: [(UserId, i64, &str); 8] = [
        (2, 3600, "Bom dia! Quem anima jogar hoje à noite?"),
        (3, 3500, "Eu topo. Por volta das oito?"),
        (
            3,
            3480,
            "Vou compartilhar a tela para a gente combinar a partida.",
        ),
        (4, 2400, "Combinado! Já vou separar um lanche."),
        (
            1,
            1800,
            "Este é nosso espaço privado. O convite está com a Maya.",
        ),
        (2, 900, "Separei uma captura para mostrar o mapa."),
        (2, 880, "Depois podemos conferir os detalhes juntos."),
        (
            3,
            120,
            "Entrando na sala dos amigos. Cheguem quando puderem!",
        ),
    ];
    let name_of = |id: UserId| {
        users
            .iter()
            .find(|u| u.id == id)
            .map(|u| (u.name.clone(), u.color))
            .unwrap()
    };
    let messages: Vec<Message> = lines
        .iter()
        .enumerate()
        .map(|(i, (author, ago, text))| {
            let (author_name, author_color) = name_of(*author);
            Message {
                id: i as u64 + 1,
                channel: 1,
                author: *author,
                author_name,
                author_color,
                time: now - ago,
                text: (*text).into(),
            }
        })
        .collect();
    let mut voice = HashMap::new();
    voice.insert(
        1,
        VoiceState {
            channel: Some(3),
            ..Default::default()
        },
    );
    voice.insert(
        2,
        VoiceState {
            channel: Some(3),
            muted: true,
            ..Default::default()
        },
    );
    voice.insert(
        3,
        VoiceState {
            channel: Some(3),
            streaming: true,
            ..Default::default()
        },
    );
    voice.insert(4, VoiceState::default());
    let mut msg_map = HashMap::new();
    msg_map.insert(1, messages);
    let mut unread = HashMap::new();
    unread.insert(2, 3);
    let mut view = ServerView {
        me: 1,
        name: "Turma do fim de semana".into(),
        guilds: vec![
            Guild {
                id: 1,
                name: "Turma do fim de semana".into(),
            },
            Guild {
                id: 2,
                name: "Grupo de estudos".into(),
            },
        ],
        friends: vec![
            FriendEntry {
                user: 2,
                status: FriendStatus::Friend,
            },
            FriendEntry {
                user: 3,
                status: FriendStatus::Friend,
            },
            FriendEntry {
                user: 4,
                status: FriendStatus::Incoming,
            },
        ],
        calls: HashMap::new(),
        call_channels: Vec::new(),
        channels,
        users: users
            .into_iter()
            .map(|u| (u.id, u))
            .collect::<BTreeMap<_, _>>(),
        messages: msg_map,
        voice,
        stage_states: HashMap::new(),
        streams: vec![StreamInfo {
            owner: 3,
            kind: StreamKind::Screen,
            label: "Screen 1".into(),
            width: 1920,
            height: 1080,
            fps: 30,
        }],
        speaking: HashSet::from([3]),
        typing: HashMap::new(),
        unread,
        ping_ms: None,
        viewers: HashMap::new(),
        workspace: WorkspaceSnapshot::default(),
        history_more: HashMap::new(),
        history_loading: HashSet::new(),
    };
    view.channels.push(Channel {
        id: 5,
        guild: 2,
        name: "estudos".into(),
        kind: ChannelKind::Text,
        topic: String::new(),
    });
    enhance(&mut view, now);
    app.view = Some(view);
    if let Some(image) = image::RgbaImage::from_raw(640, 360, sample_desktop(640, 360)) {
        let mut out = std::io::Cursor::new(Vec::new());
        if image.write_to(&mut out, image::ImageFormat::Png).is_ok() {
            let bytes = out.into_inner();
            let info = AttachmentInfo {
                id: 1,
                channel: 1,
                uploader: 2,
                name: "mapa-da-partida.png".into(),
                media_type: "image/png".into(),
                size: bytes.len() as u64,
            };
            if let Some(view) = &mut app.view {
                view.workspace.details.push(MessageDetails {
                    message: 6,
                    attachments: vec![info.clone()],
                    ..Default::default()
                });
            }
            app.workspace_ui.downloads.insert(1, (info, bytes));
        }
    }
    app.selected = Some(1);
    app.guild = Some(1);

    // `--demo-view call` shows the call with a sample stream instead of the chat.
    let args: Vec<String> = std::env::args().collect();
    let target = args
        .windows(2)
        .find(|w| w[0] == "--screenshot-view")
        .map(|w| w[1].as_str())
        .unwrap_or("");
    let call = target == "call"
        || args
            .windows(2)
            .any(|w| w[0] == "--demo-view" && w[1] == "call");
    let friends = target == "friends"
        || args
            .windows(2)
            .any(|w| w[0] == "--demo-view" && w[1] == "friends");
    if friends {
        app.home = true;
        app.selected = None;
    }
    if target == "dm" {
        app.home = true;
        app.selected = Some(6);
    }
    match target {
        "home" => {
            app.view = None;
            app.selected = None;
        }
        "settings" => app.modal = crate::app::Modal::Settings(crate::app::SettingsTab::Account),
        "appearance" => {
            app.modal = crate::app::Modal::Settings(crate::app::SettingsTab::Appearance)
        }
        "search" => {
            app.workspace_ui.search_open = true;
            app.workspace_ui.search_request.query = "partida".into();
            app.workspace_ui.search_query = "partida".into();
            app.workspace_ui.search_results = app
                .view
                .as_ref()
                .map(|v| {
                    v.messages
                        .values()
                        .flatten()
                        .filter(|m| m.text.contains("partida"))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
        }
        "inbox" => app.workspace_ui.inbox_open = true,
        "roles" | "events" | "expressions" => {
            app.workspace_ui.managing = true;
            app.workspace_ui.extras.admin_guild = Some(1);
            app.workspace_ui.extras.admin_tab = match target {
                "roles" => 2,
                "events" => 8,
                _ => 7,
            };
        }
        "forum" => app.selected = Some(20),
        "update" => crate::updater::show_demo(),
        "poll" => {
            if let Some(view) = &mut app.view {
                let message = Message {
                    id: 200,
                    channel: 1,
                    author: 1,
                    author_name: view.user_name(1),
                    author_color: view.user_color(1),
                    time: now,
                    text: "Qual jogo vamos jogar no sábado?".into(),
                };
                view.insert_message(message);
                view.workspace.community.polls.push(Poll {
                    message: 200,
                    channel: 1,
                    creator: 1,
                    question: "Qual jogo vamos jogar no sábado?".into(),
                    options: vec![
                        "Minecraft".into(),
                        "Rocket League".into(),
                        "Stardew Valley".into(),
                    ],
                    votes: vec![
                        PollVote {
                            user: 2,
                            choices: vec![0],
                        },
                        PollVote {
                            user: 3,
                            choices: vec![2],
                        },
                    ],
                    multiple: false,
                    closed: false,
                });
            }
        }
        "voice-settings" => app.modal = crate::app::Modal::Settings(crate::app::SettingsTab::Voice),
        "video-settings" => app.modal = crate::app::Modal::Settings(crate::app::SettingsTab::Video),
        "stream-settings" => {
            app.modal = crate::app::Modal::Settings(crate::app::SettingsTab::Streaming)
        }
        "manage" => app.workspace_ui.managing = true,
        "profile" => app.open_profile(1),
        "group" => {
            app.workspace_ui.group_open = true;
            app.workspace_ui.group_name = "Partida de sábado".into();
            app.workspace_ui.group_members.insert(2);
        }
        "share" => {
            app.modal = crate::app::Modal::SharePicker;
            app.picker.sources.push(crate::media::screen::ShareSource {
                id: crate::media::screen::SourceId::Monitor(1),
                name: "Tela principal".into(),
                app: String::new(),
                process_id: None,
                width: 1920,
                height: 1080,
                thumb: Some((320, 180, sample_desktop(320, 180))),
            });
            app.picker.selected = Some(0);
            let sources = std::mem::take(&mut app.picker.sources);
            app.picker.loading = Some(std::sync::Arc::new(parking_lot::Mutex::new(Some(sources))));
        }
        _ => {}
    }
    if call {
        app.selected = Some(3);
        let key = (3, StreamKind::Screen);
        let (w, h) = (1280u32, 720u32);
        app.pictures.put(key, w, h, sample_desktop(w, h), 0);
        app.watching.insert(key);
        app.focused = Some(key);
    }
}

/// Representative data for screenshot QA. No real host, file or capture device is used.
fn enhance(view: &mut ServerView, now: i64) {
    view.workspace.host_owner = Some(1);
    for user in view.users.values() {
        view.workspace.memberships.push(Membership {
            guild: 1,
            user: user.id,
            role: if user.id == 1 {
                MemberRole::Owner
            } else {
                MemberRole::Member
            },
        });
        view.workspace.profiles.push(Profile {
            user: user.id,
            status: match user.id {
                1 => "Organizando a partida",
                2 => "Disponível depois das oito",
                3 => "Compartilhando o mapa",
                _ => "",
            }
            .into(),
            presence: if user.id == 4 {
                Presence::Away
            } else {
                Presence::Online
            },
            avatar: Vec::new(),
        });
    }
    // Two categories that mix text and voice channels.
    view.workspace.categories = vec![
        Category {
            id: 1,
            guild: 1,
            name: "Encontro".into(),
            position: 0,
        },
        Category {
            id: 2,
            guild: 1,
            name: "Conversar e jogar".into(),
            position: 1,
        },
    ];
    view.workspace.channel_settings = [(1, 1, 0), (3, 1, 1), (2, 2, 0), (4, 2, 1)]
        .into_iter()
        .map(|(channel, category, position)| ChannelSettings {
            channel,
            position,
            category_id: Some(category),
            ..Default::default()
        })
        .collect();
    for (id, name, members, direct) in [
        (6, "Maya", vec![1, 2], true),
        (7, "Partida de sábado", vec![1, 2, 3, 4], false),
    ] {
        view.channels.push(Channel {
            id,
            guild: 0,
            name: name.into(),
            kind: ChannelKind::Text,
            topic: "Conversa privada persistente".into(),
        });
        view.workspace.conversations.push(Conversation {
            channel: id,
            name: name.into(),
            owner: 1,
            members,
            direct,
        });
    }
    view.messages.insert(
        6,
        vec![
            Message {
                id: 100,
                channel: 6,
                author: 2,
                author_name: "Maya".into(),
                author_color: 0x3ba55d,
                time: now - 300,
                text: "Te mandei o convite do grupo. Vamos jogar às oito?".into(),
            },
            Message {
                id: 101,
                channel: 6,
                author: 1,
                author_name: view.user_name(1),
                author_color: view.user_color(1),
                time: now - 200,
                text: "Combinado! Vou chamar o restante da turma.".into(),
            },
        ],
    );
    view.workspace.details = vec![
        MessageDetails {
            message: 1,
            pinned: true,
            reactions: vec![Reaction {
                emoji: "👍".into(),
                users: vec![1, 3, 4],
            }],
            ..Default::default()
        },
        MessageDetails {
            message: 2,
            reply_to: Some(1),
            ..Default::default()
        },
        MessageDetails {
            message: 5,
            mentions: vec![2],
            edited: Some(now - 60),
            ..Default::default()
        },
        MessageDetails {
            message: 101,
            reply_to: Some(100),
            ..Default::default()
        },
    ];
    view.history_more.insert(1, true);
    view.workspace.community.roles.push(CommunityRole {
        id: 1,
        guild: 1,
        name: "Moderadores".into(),
        color: 0x3ba55d,
        position: 10,
        permissions: permission::MEMBER | permission::MANAGE_MESSAGES | permission::MODERATE,
    });
    view.workspace.community.assignments.push(RoleAssignment {
        guild: 1,
        user: 2,
        roles: vec![1],
    });
    view.workspace.community.profile_extras.push(ProfileExtra {
        user: 1,
        guild: None,
        bio: "Jogos, projetos e boas conversas com os amigos.".into(),
        nickname: String::new(),
    });
    view.workspace.community.unread.push(UnreadSummary {
        channel: 2,
        count: 3,
        mentions: 1,
        first: None,
        latest: None,
    });
    view.workspace.community.events.push(ScheduledEvent {
        id: 1,
        guild: 1,
        creator: 1,
        name: "Partida de sábado".into(),
        description: "Traga seu jogo favorito. Vamos decidir juntos.".into(),
        starts_at: now + 86400,
        ends_at: Some(now + 93600),
        channel: Some(3),
        interested: vec![1, 2, 3],
        cancelled: false,
    });
    view.channels.push(Channel {
        id: 20,
        guild: 1,
        name: "ideias-para-jogar".into(),
        kind: ChannelKind::Text,
        topic: "Escolha um assunto e entre na conversa".into(),
    });
    view.workspace.community.policies.push(ChannelPolicy {
        channel: 20,
        style: ChannelStyle::Forum,
        slowmode_secs: 0,
    });
    view.channels.push(Channel {
        id: 21,
        guild: 1,
        name: "Qual mapa explorar primeiro?".into(),
        kind: ChannelKind::Text,
        topic: String::new(),
    });
    view.workspace.community.threads.push(ThreadInfo {
        channel: 21,
        parent: 20,
        message: None,
        creator: 2,
        archived: false,
        tags: vec!["sábado".into(), "aventura".into()],
    });
}

/// A simple made-up desktop picture: a gradient with a few window shapes.
fn sample_desktop(w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let windows = [
        (80, 70, 620, 420, [0x2b, 0x2d, 0x31]),
        (560, 200, 640, 440, [0xf2, 0xf3, 0xf5]),
    ];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let t = y as f32 / h as f32;
            let s = x as f32 / w as f32;
            let mut c = [
                (40.0 + 60.0 * s) as u8,
                (60.0 + 50.0 * t) as u8,
                (120.0 + 90.0 * (1.0 - t)) as u8,
            ];
            for (wx, wy, ww, wh, col) in windows {
                if x >= wx && x < wx + ww && y >= wy && y < wy + wh {
                    c = col;
                    if y < wy + 28 {
                        c = [0x20, 0x22, 0x25];
                    } else if (y - wy) % 24 < 10
                        && x > wx + 24
                        && x < wx + ww - 24 - ((y * 7) % 140)
                    {
                        c = if col[0] > 128 {
                            [0x50, 0x55, 0x60]
                        } else {
                            [0xc8, 0xca, 0xd0]
                        };
                    }
                }
            }
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    px
}
