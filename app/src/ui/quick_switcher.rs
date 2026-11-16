//! Ctrl+K quick switcher: incremental search over channels, conversations and
//! communities, with a recents list and keyboard navigation. Also remembers the
//! last channel visited in each community.

use egui::{vec2, Align2, Color32, Key, Modifiers, RichText};

use super::theme;
use crate::app::LanvoxApp;
use crate::model::{ChannelId, ChannelKind, GuildId};

const MAX_RECENTS: usize = 12;
const MAX_RESULTS: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// A channel, or a DM/group conversation (guild 0).
    Channel(ChannelId),
    Guild(GuildId),
}

impl Dest {
    pub fn key(self) -> String {
        match self {
            Dest::Channel(id) => format!("c:{id}"),
            Dest::Guild(id) => format!("g:{id}"),
        }
    }
}

/// Declaration order is the tie-break order between kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Text,
    Voice,
    Conversation,
    Group,
    Guild,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub dest: Dest,
    pub kind: ItemKind,
    pub guild: GuildId,
    pub label: String,
    pub detail: String,
}

#[derive(Default)]
pub struct QuickSwitcher {
    pub open: bool,
    query: String,
    selected: usize,
    /// Navigation seen on the previous frame, to record recents.
    seen: Option<ChannelId>,
    seen_guild: Option<GuildId>,
    id: Option<egui::Id>,
}

impl QuickSwitcher {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.query.clear();
        self.selected = 0;
    }
}

// ---- pure logic -----------------------------------------------------------

fn fold(text: &str) -> String {
    text.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// Score of one folded token against one folded text; None when it does not match.
fn token_score(text: &str, token: &str) -> Option<u32> {
    if text == token {
        return Some(100);
    }
    if text.starts_with(token) {
        return Some(80);
    }
    if let Some(pos) = text.find(token) {
        let boundary = text[..pos]
            .chars()
            .last()
            .is_some_and(|c| !c.is_alphanumeric());
        return Some(if boundary { 60 } else { 40 });
    }
    // Subsequence ("gdv" matches "geral-dev").
    if token.chars().count() >= 2 {
        let mut chars = text.chars();
        if token.chars().all(|t| chars.any(|c| c == t)) {
            return Some(15);
        }
    }
    None
}

/// Indexes into `items`, best first. `recents` lists `Dest::key` values, newest first.
/// An empty query shows the recents (or everything when there are none).
pub fn rank(items: &[Item], query: &str, recents: &[String]) -> Vec<usize> {
    let recent_rank = |item: &Item| recents.iter().position(|k| *k == item.dest.key());
    let query = fold(query);
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.is_empty() {
        let mut list: Vec<usize> = (0..items.len())
            .filter(|i| recent_rank(&items[*i]).is_some())
            .collect();
        if list.is_empty() {
            list = (0..items.len()).collect();
            list.sort_by_key(|i| (items[*i].kind as u8, fold(&items[*i].label)));
        } else {
            list.sort_by_key(|i| recent_rank(&items[*i]));
        }
        list.truncate(MAX_RESULTS);
        return list;
    }
    let mut scored: Vec<(usize, u32)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let label = fold(&item.label);
        let detail = fold(&item.detail);
        let mut total = 0;
        let mut all = true;
        for token in &tokens {
            let best = token_score(&label, token)
                .map(|s| s * 2)
                .max(token_score(&detail, token));
            match best {
                Some(s) => total += s,
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            scored.push((index, total));
        }
    }
    scored.sort_by(|(a, sa), (b, sb)| {
        sb.cmp(sa)
            .then_with(|| {
                let ra = recent_rank(&items[*a]).unwrap_or(usize::MAX);
                let rb = recent_rank(&items[*b]).unwrap_or(usize::MAX);
                ra.cmp(&rb)
            })
            .then_with(|| (items[*a].kind as u8).cmp(&(items[*b].kind as u8)))
            .then_with(|| fold(&items[*a].label).cmp(&fold(&items[*b].label)))
    });
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(i, _)| i)
        .collect()
}

/// Pushes `key` to the front of `list`, removing duplicates and capping its length.
pub fn push_recent(list: &mut Vec<String>, key: String) {
    list.retain(|k| *k != key);
    list.insert(0, key);
    list.truncate(MAX_RECENTS);
}

/// The remembered channel when it still exists in the community, otherwise the first text channel.
pub fn resolve_last(
    stored: Option<ChannelId>,
    channels: &[(ChannelId, ChannelKind)],
) -> Option<ChannelId> {
    stored
        .filter(|id| channels.iter().any(|(c, _)| c == id))
        .or_else(|| {
            channels
                .iter()
                .find(|(_, k)| *k == ChannelKind::Text)
                .map(|(c, _)| *c)
        })
}

// ---- app glue -------------------------------------------------------------

pub fn host_scope(app: &LanvoxApp) -> String {
    let host = app
        .reconnect_address
        .as_deref()
        .unwrap_or(&app.settings.last_address);
    format!("{}|{host}", app.local_identity)
}

/// Channel to open when clicking a community: the last one visited there.
pub fn channel_for_guild(app: &LanvoxApp, guild: GuildId) -> Option<ChannelId> {
    let view = app.view.as_ref()?;
    let channels: Vec<_> = view.guild_channels(guild).map(|c| (c.id, c.kind)).collect();
    let stored = app
        .settings
        .last_channels
        .get(&format!("{}|{guild}", host_scope(app)))
        .copied();
    resolve_last(stored, &channels)
}

fn build_items(app: &LanvoxApp) -> Vec<Item> {
    let Some(view) = &app.view else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for guild in &view.guilds {
        items.push(Item {
            dest: Dest::Guild(guild.id),
            kind: ItemKind::Guild,
            guild: guild.id,
            label: guild.name.clone(),
            detail: "Comunidade".into(),
        });
        for channel in view.guild_channels(guild.id) {
            items.push(Item {
                dest: Dest::Channel(channel.id),
                kind: if channel.kind == ChannelKind::Voice {
                    ItemKind::Voice
                } else {
                    ItemKind::Text
                },
                guild: guild.id,
                label: channel.name.clone(),
                detail: guild.name.clone(),
            });
        }
    }
    for conversation in &view.workspace.conversations {
        items.push(Item {
            dest: Dest::Channel(conversation.channel),
            kind: if conversation.direct {
                ItemKind::Conversation
            } else {
                ItemKind::Group
            },
            guild: 0,
            label: conversation.name.clone(),
            detail: if conversation.direct {
                "Mensagem direta".into()
            } else {
                "Grupo".into()
            },
        });
    }
    items
}

fn open_item(app: &mut LanvoxApp, item: &Item) {
    app.scroll_to_bottom = 3;
    match item.dest {
        Dest::Guild(guild) => {
            app.home = false;
            app.guild = Some(guild);
            if let Some(channel) = channel_for_guild(app, guild) {
                app.select_channel(channel);
            } else {
                app.show_friends();
                app.home = false;
            }
            let scope = host_scope(app);
            push_recent(
                app.settings.recents.entry(scope).or_default(),
                item.dest.key(),
            );
            app.save_preferences();
        }
        Dest::Channel(id) if item.guild == 0 => {
            app.home = true;
            app.select_channel(id);
        }
        Dest::Channel(id) => {
            app.home = false;
            app.guild = Some(item.guild);
            app.select_channel(id);
        }
    }
}

/// Records navigation (recents and last channel per community). Runs every frame.
fn track(app: &mut LanvoxApp) {
    let selected = app.selected;
    let guild = if app.home { None } else { app.guild };
    if app.quick.seen == selected && app.quick.seen_guild == guild {
        return;
    }
    app.quick.seen = selected;
    app.quick.seen_guild = guild;
    let (Some(id), Some(view)) = (selected, &app.view) else {
        return;
    };
    let Some(channel) = view.channel(id) else {
        return;
    };
    let channel_guild = channel.guild;
    let scope = host_scope(app);
    push_recent(
        app.settings.recents.entry(scope.clone()).or_default(),
        Dest::Channel(id).key(),
    );
    if channel_guild != 0 {
        app.settings
            .last_channels
            .insert(format!("{scope}|{channel_guild}"), id);
    }
    app.save_preferences();
}

pub fn show(app: &mut LanvoxApp, ctx: &egui::Context) {
    track(app);
    if !matches!(app.modal, crate::app::Modal::None)
        || app.workspace_ui.profile.is_some()
        || app.workspace_ui.group_open
        || app.workspace_ui.managing
        || app.workspace_ui.editing.is_some()
        || app.workspace_ui.deleting.is_some()
    {
        app.quick.open = false;
        return;
    }
    if !app.quick.open {
        return;
    }
    if !app.connected() {
        app.quick.open = false;
        return;
    }
    let items = build_items(app);
    let recents = app
        .settings
        .recents
        .get(&host_scope(app))
        .cloned()
        .unwrap_or_default();
    let (down, up, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if esc {
        app.quick.open = false;
        return;
    }
    let mut results = rank(&items, &app.quick.query, &recents);
    let count = results.len();
    if count > 0 {
        if down {
            app.quick.selected = (app.quick.selected + 1) % count;
        }
        if up {
            app.quick.selected = (app.quick.selected + count - 1) % count;
        }
    }
    app.quick.selected = app.quick.selected.min(count.saturating_sub(1));
    let mut chosen = if enter {
        results.get(app.quick.selected).copied()
    } else {
        None
    };
    let mut query = std::mem::take(&mut app.quick.query);
    let before = query.clone();
    let mut selected = app.quick.selected;
    let mut clicked_row = None;
    let id = *app
        .quick
        .id
        .get_or_insert_with(|| egui::Id::unique("quick-switcher"));
    let area = egui::Area::new(id)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 90.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::color(theme::PANEL))
                .stroke(egui::Stroke::new(1.0, theme::color(theme::RAIL)))
                .corner_radius(8)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(480.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("Ir para canal, conversa ou comunidade...")
                            .desired_width(f32::INFINITY),
                    );
                    edit.request_focus();
                    if query != before {
                        // Re-rank for the new text so this frame already shows it.
                        results = rank(&items, &query, &recents);
                        selected = 0;
                    }
                    ui.add_space(6.0);
                    if query.trim().is_empty() && !recents.is_empty() {
                        ui.label(RichText::new("Recentes").small().color(theme::TEXT_MUTED));
                    }
                    if results.is_empty() {
                        ui.label(RichText::new("Nada encontrado").color(theme::TEXT_MUTED));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(340.0)
                        .show(ui, |ui| {
                            for (row, index) in results.iter().enumerate() {
                                let item = &items[*index];
                                let (rect, resp) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 30.0),
                                    egui::Sense::click(),
                                );
                                if row == selected {
                                    ui.painter().rect_filled(
                                        rect,
                                        4.0,
                                        theme::color(theme::SELECTED),
                                    );
                                    ui.scroll_to_rect(rect, None);
                                } else if resp.hovered() {
                                    ui.painter()
                                        .rect_filled(rect, 4.0, theme::color(theme::HOVER));
                                }
                                let prefix = match item.kind {
                                    ItemKind::Text => "# ",
                                    ItemKind::Voice => "\u{1F50A} ",
                                    ItemKind::Conversation | ItemKind::Group => "@ ",
                                    ItemKind::Guild => "",
                                };
                                ui.painter().text(
                                    rect.left_center() + vec2(8.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    format!("{prefix}{}", item.label),
                                    egui::FontId::proportional(14.5),
                                    theme::TEXT_BRIGHT,
                                );
                                ui.painter().text(
                                    rect.right_center() - vec2(8.0, 0.0),
                                    Align2::RIGHT_CENTER,
                                    &item.detail,
                                    egui::FontId::proportional(12.0),
                                    Color32::from_gray(150),
                                );
                                if resp.clicked() {
                                    clicked_row = Some(*index);
                                }
                            }
                        });
                });
        });
    app.quick.query = query;
    app.quick.selected = selected;
    if chosen.is_none() {
        chosen = clicked_row;
    }
    let outside = ctx.input(|i| {
        i.pointer.primary_clicked()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !area.response.rect.contains(p))
    });
    if let Some(index) = chosen {
        let item = items[index].clone();
        app.quick.open = false;
        open_item(app, &item);
    } else if outside {
        app.quick.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_navigation_clears_a_reply_from_the_previous_conversation() {
        let (mut app, _) = crate::app::ui_regressions::fixture();
        app.workspace_ui.reply_to = Some(1);
        app.workspace_ui.pinned_only = true;
        open_item(&mut app, &item(2, ItemKind::Text, "links", "community"));
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.workspace_ui.reply_to, None);
        assert!(!app.workspace_ui.pinned_only);
    }

    fn item(id: u32, kind: ItemKind, label: &str, detail: &str) -> Item {
        let dest = if kind == ItemKind::Guild {
            Dest::Guild(id)
        } else {
            Dest::Channel(id)
        };
        Item {
            dest,
            kind,
            guild: 1,
            label: label.into(),
            detail: detail.into(),
        }
    }

    fn sample() -> Vec<Item> {
        vec![
            item(1, ItemKind::Guild, "Amigos", "Comunidade"),
            item(2, ItemKind::Text, "geral", "Amigos"),
            item(3, ItemKind::Text, "geral-dev", "Trabalho"),
            item(4, ItemKind::Conversation, "João", "Mensagem direta"),
            item(5, ItemKind::Voice, "Sala de voz", "Amigos"),
        ]
    }

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let items = sample();
        assert_eq!(rank(&items, "geral", &[]), vec![1, 2]);
        assert_eq!(rank(&items, "dev", &[])[0], 2);
    }

    #[test]
    fn accents_and_case_are_ignored() {
        let items = sample();
        assert_eq!(rank(&items, "JOAO", &[]), vec![3]);
        assert_eq!(rank(&items, "joão", &[]), vec![3]);
    }

    #[test]
    fn all_tokens_must_match_label_or_detail() {
        let items = sample();
        assert_eq!(rank(&items, "geral trabalho", &[]), vec![2]);
        assert!(rank(&items, "geral zzz", &[]).is_empty());
    }

    #[test]
    fn subsequence_matches() {
        assert_eq!(rank(&sample(), "gdv", &[]), vec![2]);
    }

    #[test]
    fn empty_query_shows_recents_in_order() {
        let items = sample();
        let recents = vec!["c:3".to_string(), "g:1".to_string()];
        assert_eq!(rank(&items, "", &recents), vec![2, 0]);
    }

    #[test]
    fn empty_query_without_recents_lists_everything() {
        let items = sample();
        let r = rank(&items, "", &[]);
        assert_eq!(r.len(), items.len());
        assert_eq!(items[r[0]].kind, ItemKind::Text);
    }

    #[test]
    fn recents_break_score_ties() {
        let tie = vec![
            item(10, ItemKind::Text, "obras", "x"),
            item(11, ItemKind::Text, "obras", "x"),
        ];
        assert_eq!(rank(&tie, "obras", &["c:11".to_string()]), vec![1, 0]);
    }

    #[test]
    fn recents_are_deduplicated_and_capped() {
        let mut list = Vec::new();
        for i in 0..20 {
            push_recent(&mut list, format!("c:{i}"));
        }
        push_recent(&mut list, "c:15".into());
        assert_eq!(list.len(), MAX_RECENTS);
        assert_eq!(list[0], "c:15");
        assert_eq!(list.iter().filter(|k| *k == "c:15").count(), 1);
    }

    #[test]
    fn resolve_last_prefers_stored_then_first_text() {
        let channels = [
            (10, ChannelKind::Voice),
            (11, ChannelKind::Text),
            (12, ChannelKind::Text),
        ];
        assert_eq!(resolve_last(Some(12), &channels), Some(12));
        assert_eq!(resolve_last(Some(99), &channels), Some(11));
        assert_eq!(resolve_last(None, &channels), Some(11));
        assert_eq!(resolve_last(None, &[]), None);
    }
}
