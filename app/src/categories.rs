#![cfg_attr(not(feature = "gui"), allow(dead_code))]
//! Pure category and channel-placement logic, shared by the host, the client and the
//! migration, so ordering and moving behave identically everywhere.
//!
//! Model: a community has ordered `Category`s; every channel belongs to at most one
//! (`ChannelSettings::category_id`) and has a `position` inside that group. Channels
//! without a category form the uncategorised group, shown at the top. Text and voice
//! channels share the same list.

use std::collections::BTreeMap;

use crate::model::*;

/// Categories allowed per community.
pub const MAX_CATEGORIES: usize = 50;

fn is_voice(kind: ChannelKind) -> bool {
    kind == ChannelKind::Voice
}

/// The category a channel really belongs to: a missing entry, a deleted category or a
/// category of another community all mean "uncategorised".
fn effective(
    guild: GuildId,
    setting: Option<&ChannelSettings>,
    categories: &[Category],
) -> Option<CategoryId> {
    let id = setting?.category_id?;
    categories
        .iter()
        .any(|c| c.id == id && c.guild == guild)
        .then_some(id)
}

/// Channels of one group, in display order (position, then id).
pub fn group_order(
    guild: GuildId,
    channels: &[Channel],
    settings: &[ChannelSettings],
    categories: &[Category],
    group: Option<CategoryId>,
) -> Vec<ChannelId> {
    let mut list: Vec<(i32, ChannelId)> = channels
        .iter()
        .filter(|c| c.guild == guild)
        .filter_map(|c| {
            let s = settings.iter().find(|s| s.channel == c.id);
            (effective(guild, s, categories) == group)
                .then(|| (s.map(|s| s.position).unwrap_or(0), c.id))
        })
        .collect();
    list.sort();
    list.into_iter().map(|(_, id)| id).collect()
}

/// One block of the sidebar: the uncategorised channels (`category: None`, always first) or a
/// category with its channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub category: Option<Category>,
    pub channels: Vec<ChannelId>,
}

/// The sidebar layout of one community. Empty categories are kept (they are drop targets).
pub fn layout(
    guild: GuildId,
    channels: &[Channel],
    settings: &[ChannelSettings],
    categories: &[Category],
) -> Vec<Section> {
    let mut sections = vec![Section {
        category: None,
        channels: group_order(guild, channels, settings, categories, None),
    }];
    for category in sorted_categories(guild, categories) {
        let list = group_order(guild, channels, settings, categories, Some(category.id));
        sections.push(Section {
            category: Some(category),
            channels: list,
        });
    }
    sections
}

/// The community's categories in display order.
pub fn sorted_categories(guild: GuildId, categories: &[Category]) -> Vec<Category> {
    let mut list: Vec<Category> = categories
        .iter()
        .filter(|c| c.guild == guild)
        .cloned()
        .collect();
    list.sort_by_key(|c| (c.position, c.id));
    list
}

fn entry(settings: &mut Vec<ChannelSettings>, channel: ChannelId) -> &mut ChannelSettings {
    if let Some(i) = settings.iter().position(|s| s.channel == channel) {
        return &mut settings[i];
    }
    settings.push(ChannelSettings {
        channel,
        ..Default::default()
    });
    settings.last_mut().unwrap()
}

fn renumber(settings: &mut Vec<ChannelSettings>, order: &[ChannelId]) {
    for (i, id) in order.iter().enumerate() {
        entry(settings, *id).position = i as i32;
    }
}

/// Moves `channel` into `target` (None = uncategorised) at `index`, counted among the target
/// group's channels without the moved one (clamped). Both groups end up with dense positions.
/// Returns false when the channel is unknown or the category is not in the channel's community.
pub fn move_channel(
    channels: &[Channel],
    settings: &mut Vec<ChannelSettings>,
    categories: &[Category],
    channel: ChannelId,
    target: Option<CategoryId>,
    index: usize,
) -> bool {
    let Some(ch) = channels.iter().find(|c| c.id == channel) else {
        return false;
    };
    let guild = ch.guild;
    if let Some(t) = target {
        if !categories.iter().any(|c| c.id == t && c.guild == guild) {
            return false;
        }
    }
    let source = effective(
        guild,
        settings.iter().find(|s| s.channel == channel),
        categories,
    );
    let mut order = group_order(guild, channels, settings, categories, target);
    order.retain(|id| *id != channel);
    order.insert(index.min(order.len()), channel);
    {
        let e = entry(settings, channel);
        e.category_id = target;
        e.category.clear();
    }
    renumber(settings, &order);
    if source != target {
        let rest = group_order(guild, channels, settings, categories, source);
        renumber(settings, &rest);
    }
    true
}

/// Next free category id (ids are never reused while the counter is kept).
pub fn next_category_id(categories: &[Category], counter: CategoryId) -> CategoryId {
    counter.max(
        categories
            .iter()
            .map(|c| c.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    )
}

/// Appends a category at the end of its community's list and returns its id.
pub fn create_category(
    categories: &mut Vec<Category>,
    counter: &mut CategoryId,
    guild: GuildId,
    name: &str,
) -> CategoryId {
    let id = next_category_id(categories, *counter);
    *counter = id.saturating_add(1);
    let position = categories
        .iter()
        .filter(|c| c.guild == guild)
        .map(|c| c.position + 1)
        .max()
        .unwrap_or(0);
    categories.push(Category {
        id,
        guild,
        name: name.to_string(),
        position,
    });
    id
}

/// Applies a full or partial order: listed ids first (duplicates ignored), the rest after, in
/// their previous order. Err when an id is not a category of `guild`.
pub fn reorder_categories(
    categories: &mut [Category],
    guild: GuildId,
    order: &[CategoryId],
) -> Result<(), &'static str> {
    if order
        .iter()
        .any(|id| !categories.iter().any(|c| c.id == *id && c.guild == guild))
    {
        return Err("Categoria inválida");
    }
    let mut sequence: Vec<CategoryId> = Vec::new();
    for id in order {
        if !sequence.contains(id) {
            sequence.push(*id);
        }
    }
    for c in sorted_categories(guild, categories) {
        if !sequence.contains(&c.id) {
            sequence.push(c.id);
        }
    }
    for c in categories.iter_mut().filter(|c| c.guild == guild) {
        c.position = sequence.iter().position(|id| *id == c.id).unwrap_or(0) as i32;
    }
    Ok(())
}

/// Removes a category; its channels are kept and go to the end of the uncategorised group in
/// their previous order. Returns false when the category does not exist.
pub fn delete_category(
    channels: &[Channel],
    settings: &mut Vec<ChannelSettings>,
    categories: &mut Vec<Category>,
    id: CategoryId,
) -> bool {
    let Some(guild) = categories.iter().find(|c| c.id == id).map(|c| c.guild) else {
        return false;
    };
    let moved = group_order(guild, channels, settings, categories, Some(id));
    let mut top = group_order(guild, channels, settings, categories, None);
    categories.retain(|c| c.id != id);
    for s in settings.iter_mut().filter(|s| s.category_id == Some(id)) {
        s.category_id = None;
    }
    top.extend(moved);
    renumber(settings, &top);
    // Close the gap in the remaining category positions.
    let order: Vec<CategoryId> = sorted_categories(guild, categories)
        .iter()
        .map(|c| c.id)
        .collect();
    let _ = reorder_categories(categories, guild, &order);
    true
}

/// Places a freshly created channel at the end of `target` (None = uncategorised).
pub fn place_new_channel(
    channels: &[Channel],
    settings: &mut Vec<ChannelSettings>,
    categories: &[Category],
    channel: ChannelId,
    target: Option<CategoryId>,
) {
    move_channel(channels, settings, categories, channel, target, usize::MAX);
}

/// Converts schema-v2 free-text categories into entities.
///
/// * Channels of a community sharing the same (trimmed) text become one category.
/// * Categories are ordered by their text, exactly as the old sidebar sorted them.
/// * Within each group the old order is preserved: text channels before voice channels (the old
///   sidebar drew them as two sections), then by position and id. Positions become dense and
///   every channel gets an explicit entry.
/// * Channels with no text stay uncategorised, at the top.
///
/// `all_groups` normalises every community (the first run over old data); otherwise only
/// stray text categories are converted. Running it again changes nothing.
pub fn adopt_text_categories(
    channels: &[Channel],
    settings: &mut Vec<ChannelSettings>,
    categories: &mut Vec<Category>,
    counter: &mut CategoryId,
    all_groups: bool,
) -> bool {
    let before = (settings.clone(), categories.clone(), *counter);
    let mut guilds: Vec<GuildId> = channels
        .iter()
        .filter(|c| c.guild != 0)
        .map(|c| c.guild)
        .collect();
    guilds.sort_unstable();
    guilds.dedup();
    for guild in guilds {
        let in_guild: Vec<&Channel> = channels.iter().filter(|c| c.guild == guild).collect();
        let text_of = |settings: &[ChannelSettings], id: ChannelId| -> Option<String> {
            let s = settings.iter().find(|s| s.channel == id)?;
            let t = s.category.trim();
            (s.category_id.is_none() && !t.is_empty()).then(|| t.to_string())
        };
        let stray = in_guild.iter().any(|c| text_of(settings, c.id).is_some());
        if !stray && !all_groups {
            continue;
        }
        // text -> channels, sorted by text like the old sidebar.
        let mut groups: BTreeMap<String, Vec<&Channel>> = BTreeMap::new();
        for c in &in_guild {
            if let Some(text) = text_of(settings, c.id) {
                groups.entry(text).or_default().push(c);
            }
        }
        for (text, members) in groups {
            let existing = categories
                .iter()
                .find(|c| c.guild == guild && c.name == text)
                .map(|c| c.id);
            let id = existing.unwrap_or_else(|| create_category(categories, counter, guild, &text));
            for c in members {
                let e = entry(settings, c.id);
                e.category_id = Some(id);
                e.category.clear();
            }
        }
        if all_groups || stray {
            let mut groups: Vec<Option<CategoryId>> = vec![None];
            groups.extend(
                sorted_categories(guild, categories)
                    .iter()
                    .map(|c| Some(c.id)),
            );
            for group in groups {
                let mut keyed: Vec<(bool, i32, ChannelId)> = in_guild
                    .iter()
                    .filter(|c| {
                        effective(
                            guild,
                            settings.iter().find(|s| s.channel == c.id),
                            categories,
                        ) == group
                    })
                    .map(|c| {
                        let pos = settings
                            .iter()
                            .find(|s| s.channel == c.id)
                            .map(|s| s.position)
                            .unwrap_or(0);
                        (is_voice(c.kind), pos, c.id)
                    })
                    .collect();
                keyed.sort();
                let order: Vec<ChannelId> = keyed.into_iter().map(|(_, _, id)| id).collect();
                renumber(settings, &order);
            }
        }
    }
    (settings.clone(), categories.clone(), *counter) != before
}

// ------------------------------------------------------------------ drag and drop

/// What the sidebar rows are, top to bottom, with their vertical extent. Built each frame
/// from the real layout so the drag logic below stays pure and testable.
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// Header of a category, or the uncategorised drop strip (`None`).
    Header(Option<CategoryId>),
    /// A channel row inside a group.
    Channel {
        channel: ChannelId,
        category: Option<CategoryId>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub top: f32,
    pub bottom: f32,
}

/// Where a dragged channel would land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelDrop {
    pub category: Option<CategoryId>,
    /// Index among the target group's channels, not counting the dragged one.
    pub index: usize,
}

/// Target for a channel dragged to vertical position `y`: the upper half of a channel row
/// inserts before it, the lower half after it; a category header inserts at the group's start.
/// Above everything it goes to the top of the uncategorised group, below everything to the end
/// of the last group. `collapsed` groups only accept drops on their header (appended).
pub fn locate_channel_drop(
    rows: &[Row],
    dragged: ChannelId,
    collapsed: &[CategoryId],
    y: f32,
) -> Option<ChannelDrop> {
    let first = rows.first()?;
    let count = |category: Option<CategoryId>, upto: usize| {
        rows[..upto]
            .iter()
            .filter(|r| {
                matches!(&r.kind, RowKind::Channel { channel, category: c }
                    if *c == category && *channel != dragged)
            })
            .count()
    };
    if y < first.top {
        return Some(ChannelDrop {
            category: None,
            index: 0,
        });
    }
    for (i, row) in rows.iter().enumerate() {
        if y > row.bottom && i + 1 < rows.len() {
            continue;
        }
        return Some(match &row.kind {
            RowKind::Header(category) => {
                // A collapsed group hides its rows, so a drop on it appends (the host clamps).
                let index = if category.is_some_and(|c| collapsed.contains(&c)) {
                    usize::MAX
                } else {
                    0
                };
                ChannelDrop {
                    category: *category,
                    index,
                }
            }
            RowKind::Channel { channel, category } => {
                let before = count(*category, i);
                let upper = y < (row.top + row.bottom) / 2.0;
                // The dragged row itself: stay where we are.
                let index = if *channel == dragged || upper {
                    before
                } else {
                    before + 1
                };
                ChannelDrop {
                    category: *category,
                    index,
                }
            }
        });
    }
    None
}

/// Vertical position of the insertion line for `drop`: before the channel row now at that
/// index, after the group's last row, or under the group's header when it has no rows.
pub fn channel_indicator_y(rows: &[Row], dragged: ChannelId, drop: ChannelDrop) -> Option<f32> {
    let mut seen = 0usize;
    let mut last: Option<f32> = None;
    for row in rows {
        match &row.kind {
            RowKind::Channel { channel, category } if *category == drop.category => {
                if *channel == dragged {
                    continue;
                }
                if seen == drop.index {
                    return Some(row.top - 1.0);
                }
                seen += 1;
                last = Some(row.bottom + 1.0);
            }
            _ => {}
        }
    }
    last.or_else(|| {
        rows.iter()
            .find(|r| r.kind == RowKind::Header(drop.category))
            .map(|r| r.bottom + 1.0)
    })
}

/// What the person is dragging in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarDrag {
    Channel(ChannelId),
    Category(CategoryId),
}

/// Insertion index (0..=len) for a dragged category given each category block's vertical
/// extent (header through its last channel), in display order.
pub fn locate_category_drop(blocks: &[(f32, f32)], y: f32) -> usize {
    blocks
        .iter()
        .position(|(top, bottom)| y < (top + bottom) / 2.0)
        .unwrap_or(blocks.len())
}

/// New category order after dragging `from` (index in `order`) to insertion point `to`
/// (0..=len, as returned by `locate_category_drop`).
pub fn reordered(order: &[CategoryId], from: usize, to: usize) -> Vec<CategoryId> {
    let mut list = order.to_vec();
    if from >= list.len() {
        return list;
    }
    let item = list.remove(from);
    let at = if to > from { to - 1 } else { to }.min(list.len());
    list.insert(at, item);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: ChannelId, kind: ChannelKind) -> Channel {
        Channel {
            id,
            guild: 1,
            name: format!("c{id}"),
            kind,
            topic: String::new(),
        }
    }
    fn st(channel: ChannelId, category: &str, position: i32) -> ChannelSettings {
        ChannelSettings {
            channel,
            category: category.into(),
            position,
            ..Default::default()
        }
    }
    fn cat(id: CategoryId, name: &str, position: i32) -> Category {
        Category {
            id,
            guild: 1,
            name: name.into(),
            position,
        }
    }
    use ChannelKind::{Text, Voice};

    #[test]
    fn adoption_merges_same_text_orders_by_text_and_keeps_every_channel() {
        let channels = vec![
            ch(1, Text),
            ch(2, Voice),
            ch(3, Text),
            ch(4, Voice),
            ch(5, Text),
        ];
        let mut settings = vec![
            st(1, "Zeta", 0),
            st(2, "Zeta", 0),
            st(3, "Alfa", 5),
            st(4, "Alfa", 1),
            // 5 has no entry at all
        ];
        let (mut cats, mut counter) = (Vec::new(), 1);
        assert!(adopt_text_categories(
            &channels,
            &mut settings,
            &mut cats,
            &mut counter,
            true
        ));
        let l = layout(1, &channels, &settings, &cats);
        assert_eq!(l[0].category, None);
        assert_eq!(l[0].channels, vec![5]);
        let names: Vec<_> = l[1..]
            .iter()
            .map(|s| s.category.as_ref().unwrap().name.as_str())
            .collect();
        assert_eq!(names, ["Alfa", "Zeta"]);
        // text before voice inside a category, as the old two-section sidebar drew them
        assert_eq!(l[1].channels, vec![3, 4]);
        assert_eq!(l[2].channels, vec![1, 2]);
        let total: usize = l.iter().map(|s| s.channels.len()).sum();
        assert_eq!(total, channels.len());
        assert!(settings.iter().all(|s| s.category.is_empty()));
        // idempotent
        let snapshot = (settings.clone(), cats.clone(), counter);
        assert!(!adopt_text_categories(
            &channels,
            &mut settings,
            &mut cats,
            &mut counter,
            false
        ));
        assert_eq!(snapshot, (settings, cats, counter));
    }

    fn fixture() -> (Vec<Channel>, Vec<ChannelSettings>, Vec<Category>) {
        let channels = vec![ch(1, Text), ch(2, Voice), ch(3, Text), ch(4, Text)];
        let mut settings = vec![st(1, "", 0), st(2, "", 1), st(3, "", 0), st(4, "", 1)];
        settings[0].category_id = Some(10);
        settings[1].category_id = Some(10);
        settings[2].category_id = Some(11);
        settings[3].category_id = Some(11);
        (channels, settings, vec![cat(10, "A", 0), cat(11, "B", 1)])
    }
    fn ids(l: &[Section]) -> Vec<Vec<u32>> {
        l.iter().map(|s| s.channels.clone()).collect()
    }

    #[test]
    fn moving_between_categories_and_inside_keeps_positions_dense() {
        let (channels, mut settings, cats) = fixture();
        // text and voice mix; 3 goes to the middle of A
        assert!(move_channel(
            &channels,
            &mut settings,
            &cats,
            3,
            Some(10),
            1
        ));
        assert_eq!(
            ids(&layout(1, &channels, &settings, &cats)),
            vec![vec![], vec![1, 3, 2], vec![4]]
        );
        let b: Vec<_> = settings
            .iter()
            .filter(|s| s.category_id == Some(11))
            .map(|s| (s.channel, s.position))
            .collect();
        assert_eq!(b, vec![(4, 0)]);
        // reorder inside the same category
        assert!(move_channel(
            &channels,
            &mut settings,
            &cats,
            2,
            Some(10),
            0
        ));
        assert_eq!(
            ids(&layout(1, &channels, &settings, &cats))[1],
            vec![2, 1, 3]
        );
        // out of every category: top group
        assert!(move_channel(&channels, &mut settings, &cats, 4, None, 0));
        assert_eq!(ids(&layout(1, &channels, &settings, &cats))[0], vec![4]);
        // index beyond the end appends; unknown targets are refused
        assert!(move_channel(
            &channels,
            &mut settings,
            &cats,
            4,
            Some(11),
            99
        ));
        assert!(!move_channel(
            &channels,
            &mut settings,
            &cats,
            4,
            Some(99),
            0
        ));
        assert!(!move_channel(&channels, &mut settings, &cats, 77, None, 0));
    }

    #[test]
    fn deleting_a_category_keeps_its_channels_at_the_end_of_the_top_group() {
        let (channels, mut settings, mut cats) = fixture();
        settings.push(st(5, "", 0));
        let mut channels = channels;
        channels.push(ch(5, Text));
        assert!(delete_category(&channels, &mut settings, &mut cats, 10));
        let l = layout(1, &channels, &settings, &cats);
        assert_eq!(ids(&l), vec![vec![5, 1, 2], vec![3, 4]]);
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].position, 0);
        assert!(!delete_category(&channels, &mut settings, &mut cats, 10));
    }

    #[test]
    fn dangling_category_ids_fall_back_to_the_top() {
        let (channels, settings, _) = fixture();
        let l = layout(1, &channels, &settings, &[]);
        assert_eq!(ids(&l), vec![vec![1, 3, 2, 4]]);
    }

    #[test]
    fn create_and_reorder_categories() {
        let mut cats = vec![cat(1, "A", 0), cat(2, "B", 1)];
        let mut counter = 1;
        let id = create_category(&mut cats, &mut counter, 1, "C");
        assert_eq!(id, 3);
        assert_eq!(counter, 4);
        reorder_categories(&mut cats, 1, &[3, 1]).unwrap();
        let order: Vec<_> = sorted_categories(1, &cats).iter().map(|c| c.id).collect();
        assert_eq!(order, vec![3, 1, 2]);
        assert!(reorder_categories(&mut cats, 2, &[1]).is_err());
        assert!(reorder_categories(&mut cats, 1, &[42]).is_err());
    }

    fn rows() -> Vec<Row> {
        // strip(None) y0-10 | ch1 (None) 10-30 | header A 40-60 | ch2 A 60-80 | ch3 A 80-100 | header B 110-130
        let r = |kind, top, bottom| Row { kind, top, bottom };
        vec![
            r(RowKind::Header(None), 0.0, 10.0),
            r(
                RowKind::Channel {
                    channel: 1,
                    category: None,
                },
                10.0,
                30.0,
            ),
            r(RowKind::Header(Some(10)), 40.0, 60.0),
            r(
                RowKind::Channel {
                    channel: 2,
                    category: Some(10),
                },
                60.0,
                80.0,
            ),
            r(
                RowKind::Channel {
                    channel: 3,
                    category: Some(10),
                },
                80.0,
                100.0,
            ),
            r(RowKind::Header(Some(11)), 110.0, 130.0),
        ]
    }

    #[test]
    fn channel_drop_picks_group_and_half() {
        let rows = rows();
        let d = |dragged, y| locate_channel_drop(&rows, dragged, &[], y).unwrap();
        assert_eq!(
            d(9, -5.0),
            ChannelDrop {
                category: None,
                index: 0
            }
        );
        assert_eq!(
            d(9, 12.0),
            ChannelDrop {
                category: None,
                index: 0
            }
        );
        assert_eq!(
            d(9, 28.0),
            ChannelDrop {
                category: None,
                index: 1
            }
        );
        assert_eq!(
            d(9, 50.0),
            ChannelDrop {
                category: Some(10),
                index: 0
            }
        );
        assert_eq!(
            d(9, 65.0),
            ChannelDrop {
                category: Some(10),
                index: 0
            }
        );
        assert_eq!(
            d(9, 75.0),
            ChannelDrop {
                category: Some(10),
                index: 1
            }
        );
        assert_eq!(
            d(9, 95.0),
            ChannelDrop {
                category: Some(10),
                index: 2
            }
        );
        // empty category B: below everything lands on its header, at its start
        assert_eq!(
            d(9, 500.0),
            ChannelDrop {
                category: Some(11),
                index: 0
            }
        );
        // dragging ch2 over ch3's lower half: index excludes the dragged row itself
        assert_eq!(
            d(2, 95.0),
            ChannelDrop {
                category: Some(10),
                index: 1
            }
        );
        // over its own row: unchanged position
        assert_eq!(
            d(2, 70.0),
            ChannelDrop {
                category: Some(10),
                index: 0
            }
        );
        // a gap between rows resolves to the next row below
        assert_eq!(
            d(9, 35.0),
            ChannelDrop {
                category: Some(10),
                index: 0
            }
        );
    }

    #[test]
    fn collapsed_category_accepts_drops_only_at_its_end() {
        let rows = vec![
            Row {
                kind: RowKind::Header(Some(10)),
                top: 0.0,
                bottom: 20.0,
            },
            Row {
                kind: RowKind::Channel {
                    channel: 2,
                    category: Some(10),
                },
                top: 20.0,
                bottom: 40.0,
            },
        ];
        let d = locate_channel_drop(&rows, 9, &[10], 10.0).unwrap();
        assert_eq!(
            d,
            ChannelDrop {
                category: Some(10),
                index: usize::MAX
            }
        );
    }

    #[test]
    fn indicator_sits_before_the_target_row_after_the_group_or_under_an_empty_header() {
        let rows = rows();
        let at = |category, index, dragged| {
            channel_indicator_y(&rows, dragged, ChannelDrop { category, index }).unwrap()
        };
        assert_eq!(at(Some(10), 0, 9), 59.0);
        assert_eq!(at(Some(10), 1, 9), 79.0);
        assert_eq!(at(Some(10), 2, 9), 101.0);
        // the dragged row does not count: moving ch2 to index 1 lands after ch3
        assert_eq!(at(Some(10), 1, 2), 101.0);
        assert_eq!(at(Some(11), 0, 9), 131.0);
        assert_eq!(at(None, 1, 9), 31.0);
    }

    #[test]
    fn category_drop_and_reorder() {
        let blocks = [(0.0, 100.0), (100.0, 160.0), (160.0, 200.0)];
        assert_eq!(locate_category_drop(&blocks, 10.0), 0);
        assert_eq!(locate_category_drop(&blocks, 90.0), 1);
        assert_eq!(locate_category_drop(&blocks, 150.0), 2);
        assert_eq!(locate_category_drop(&blocks, 500.0), 3);
        assert_eq!(reordered(&[1, 2, 3], 0, 3), vec![2, 3, 1]);
        assert_eq!(reordered(&[1, 2, 3], 2, 0), vec![3, 1, 2]);
        assert_eq!(reordered(&[1, 2, 3], 1, 2), vec![1, 2, 3]);
        assert_eq!(reordered(&[1, 2, 3], 1, 1), vec![1, 2, 3]);
    }
}
