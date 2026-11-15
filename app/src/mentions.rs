//! Unambiguous user mentions.
//!
//! A message stores mentions as `<@id>` tokens (written by the composer), so a mention is bound
//! to an identity rather than a name. Older messages and older clients wrote a literal `@name`;
//! those are still understood, matching only at word boundaries and preferring the longest name.

use crate::model::UserId;

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Parses `<@digits>` at the start of `s`; returns the id and the byte length of the token.
fn token_at(s: &str) -> Option<(UserId, usize)> {
    let rest = s.strip_prefix("<@")?;
    let end = rest.find('>')?;
    let digits = &rest[..end];
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse().ok()?, end + 3))
}

/// Candidates sorted so that the longest name is tried first.
fn by_length(names: &[(UserId, String)]) -> Vec<&(UserId, String)> {
    let mut sorted: Vec<_> = names.iter().filter(|(_, n)| !n.is_empty()).collect();
    sorted.sort_by_key(|item| std::cmp::Reverse(item.1.chars().count()));
    sorted
}

/// If a literal `@name` starts at byte `i` of `text` (at a word boundary on both sides),
/// returns the matched user and the byte length including the `@`.
fn legacy_at(text: &str, i: usize, sorted: &[&(UserId, String)]) -> Option<(UserId, usize)> {
    if !text[i..].starts_with('@') {
        return None;
    }
    if text[..i].chars().next_back().is_some_and(is_word) {
        return None;
    }
    let after = &text[i + 1..];
    sorted.iter().find_map(|(id, name)| {
        let tail = after.strip_prefix(name.as_str())?;
        (!tail.chars().next().is_some_and(is_word)).then_some((*id, name.len() + 1))
    })
}

/// Byte ranges that are code (fenced or inline) and therefore never mention anyone.
fn code_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with("```") {
            let end = text[i + 3..]
                .find("```")
                .map_or(text.len(), |e| i + 3 + e + 3);
            out.push((i, end));
            i = end;
        } else if text[i..].starts_with('`') {
            match text[i + 1..].find('`') {
                Some(e) => {
                    out.push((i, i + 1 + e + 1));
                    i += e + 2;
                }
                None => i += 1,
            }
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

/// Walks the text yielding `(start, len, user)` for every mention, outside code.
fn scan(text: &str, names: &[(UserId, String)]) -> Vec<(usize, usize, UserId)> {
    let sorted = by_length(names);
    let code = code_ranges(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(&(_, end)) = code.iter().find(|(s, e)| i >= *s && i < *e) {
            i = end;
            continue;
        }
        match token_at(&text[i..]).or_else(|| legacy_at(text, i, &sorted)) {
            Some((id, len)) => {
                out.push((i, len, id));
                i += len;
            }
            None => i += text[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

/// Distinct users mentioned by `text`. `members` are the people who can be mentioned.
pub fn mentioned(text: &str, members: &[(UserId, String)]) -> Vec<UserId> {
    let mut ids = Vec::new();
    for (_, _, id) in scan(text, members) {
        // Only real, mentionable members count (a forged `<@id>` is ignored).
        if members.iter().any(|(m, _)| *m == id) && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Rewrites literal `@name` (boundary-matched, longest name wins) into `<@id>` tokens.
pub fn encode(text: &str, members: &[(UserId, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, len, id) in scan(text, members) {
        if text[start..].starts_with("<@") {
            continue;
        }
        out.push_str(&text[last..start]);
        out.push_str(&format!("<@{id}>"));
        last = start + len;
    }
    out.push_str(&text[last..]);
    out
}

/// Replaces `<@id>` tokens by a highlighted `@name` for display. Unknown ids are left untouched.
pub fn display(text: &str, name_of: impl Fn(UserId) -> Option<String>) -> String {
    let code = code_ranges(text);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some(&(_, end)) = code.iter().find(|(s, e)| i >= *s && i < *e) {
            out.push_str(&text[i..end]);
            i = end;
            continue;
        }
        if let Some((id, len)) = token_at(&text[i..]) {
            if let Some(name) = name_of(id) {
                out.push_str(&format!("**@{name}**"));
                i += len;
                continue;
            }
        }
        let c = text[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// The `@query` being typed at the end of `before_cursor`: byte offset of the `@` and the query.
pub fn active_query(before_cursor: &str) -> Option<(usize, &str)> {
    let at = before_cursor.rfind('@')?;
    if before_cursor[..at].chars().next_back().is_some_and(is_word) {
        return None;
    }
    let query = &before_cursor[at + 1..];
    (query.chars().count() <= 32 && !query.contains('\n')).then_some((at, query))
}

/// Members whose name matches `query` (case-insensitive), prefix matches first.
pub fn suggestions(members: &[(UserId, String)], query: &str) -> Vec<(UserId, String)> {
    let q = query.to_lowercase();
    let mut hits: Vec<_> = members
        .iter()
        .filter(|(_, n)| n.to_lowercase().contains(&q))
        .cloned()
        .collect();
    hits.sort_by_key(|(_, n)| (!n.to_lowercase().starts_with(&q), n.to_lowercase()));
    hits.truncate(8);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> Vec<(UserId, String)> {
        vec![
            (1, "Ana".into()),
            (2, "Anabela".into()),
            (3, "Bob Lee".into()),
            (4, "Bob".into()),
        ]
    }

    #[test]
    fn legacy_mentions_respect_word_boundaries() {
        assert_eq!(mentioned("oi @Anabela!", &members()), vec![2]);
        assert_eq!(mentioned("oi @Ana, tudo bem", &members()), vec![1]);
        assert_eq!(mentioned("oi @Anabelas", &members()), Vec::<UserId>::new());
        assert_eq!(mentioned("mail@Ana", &members()), Vec::<UserId>::new());
    }

    #[test]
    fn longest_name_wins() {
        assert_eq!(mentioned("@Bob Lee chegou", &members()), vec![3]);
        assert_eq!(mentioned("@Bob chegou", &members()), vec![4]);
        assert_eq!(mentioned("@Ana @Anabela", &members()), vec![1, 2]);
    }

    #[test]
    fn tokens_bind_to_identity_and_ignore_forged_ids() {
        assert_eq!(mentioned("<@2> e <@1>", &members()), vec![2, 1]);
        assert_eq!(
            mentioned("<@99> <@x> <@>", &members()),
            Vec::<UserId>::new()
        );
        assert_eq!(
            mentioned("`@Ana` e ```<@2>```", &members()),
            Vec::<UserId>::new()
        );
    }

    #[test]
    fn encode_and_display_round_trip() {
        let encoded = encode("oi @Anabela e @Ana.", &members());
        assert_eq!(encoded, "oi <@2> e <@1>.");
        assert_eq!(encode(&encoded, &members()), encoded);
        let shown = display(&encoded, |id| {
            members().iter().find(|m| m.0 == id).map(|m| m.1.clone())
        });
        assert_eq!(shown, "oi **@Anabela** e **@Ana**.");
        assert_eq!(display("<@9>", |_| None), "<@9>");
    }

    #[test]
    fn autocomplete_query_and_ranking() {
        assert_eq!(active_query("oi @An"), Some((3, "An")));
        assert_eq!(active_query("a@An"), None);
        assert_eq!(active_query("sem"), None);
        let hits = suggestions(&members(), "an");
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![1, 2]);
    }
}
