//! Markdown-like message formatting: a parser that produces a small
//! intermediate tree (`Block` / `Inline`) and an egui renderer for it.
//!
//! Supported: **bold**, *italic*, __underline__, ~~strike~~, combinations
//! (`***x***`, nesting), `inline code`, fenced code blocks, lists, quotes,
//! `||spoilers||` and bare http(s) links. Unclosed delimiters stay literal.

use super::{theme, widgets};
use egui::{Color32, RichText};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text { text: String, style: Style },
    Code(String),
    Link(String),
    LabeledLink { label: String, url: String },
    Emoji { name: String, id: Option<u64> },
    Spoiler(Vec<Inline>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    /// One line of text.
    Paragraph(Vec<Inline>),
    Code {
        lang: Option<String>,
        text: String,
    },
    /// Consecutive `> ` lines.
    Quote(Vec<Vec<Inline>>),
    List {
        start: Option<u64>,
        items: Vec<Vec<Inline>>,
    },
}

// ---------------------------------------------------------------- parser

pub fn parse(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=3).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            blocks.push(Block::Heading {
                level: hashes as u8,
                content: parse_inline(trimmed[hashes + 1..].trim()),
            });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("```") {
            // Single-line form: ```code```
            if let Some(body) = rest.strip_suffix("```") {
                if !body.is_empty() {
                    blocks.push(Block::Code {
                        lang: None,
                        text: body.to_string(),
                    });
                    i += 1;
                    continue;
                }
            }
            if let Some(end) = (i + 1..lines.len()).find(|&j| lines[j].trim() == "```") {
                let lang = rest.trim();
                blocks.push(Block::Code {
                    lang: (!lang.is_empty()).then(|| lang.to_string()),
                    text: lines[i + 1..end].join("\n"),
                });
                i = end + 1;
                continue;
            }
            // Unclosed fence: keep as literal text.
        }
        if trimmed.starts_with("> ") || trimmed == ">" {
            let mut quote = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim_start();
                let content = if let Some(c) = t.strip_prefix("> ") {
                    c
                } else if t == ">" {
                    ""
                } else {
                    break;
                };
                quote.push(parse_inline(content));
                i += 1;
            }
            blocks.push(Block::Quote(quote));
            continue;
        }
        if let Some((start, _)) = list_marker(trimmed) {
            let mut items = Vec::new();
            let ordered = start.is_some();
            while i < lines.len() {
                match list_marker(lines[i].trim_start()) {
                    Some((s, content)) if s.is_some() == ordered => {
                        items.push(parse_inline(content));
                        i += 1;
                    }
                    _ => break,
                }
            }
            blocks.push(Block::List { start, items });
            continue;
        }
        if !line.trim().is_empty() {
            blocks.push(Block::Paragraph(parse_inline(line)));
        }
        i += 1;
    }
    blocks
}

/// Returns (ordered start number, content) for `- x`, `* x` or `1. x`.
fn list_marker(line: &str) -> Option<(Option<u64>, &str)> {
    for p in ["- ", "* "] {
        if let Some(rest) = line.strip_prefix(p) {
            if !rest.trim().is_empty() {
                return Some((None, rest));
            }
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        if let Some(rest) = line[digits..].strip_prefix(". ") {
            if !rest.trim().is_empty() {
                return Some((line[..digits].parse().ok(), rest));
            }
        }
    }
    None
}

pub fn parse_inline(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    inline(&chars, Style::default())
}

fn starts(chars: &[char], at: usize, pat: &str) -> bool {
    for (i, c) in (at..).zip(pat.chars()) {
        if chars.get(i) != Some(&c) {
            return false;
        }
    }
    true
}

/// Finds the closing delimiter at or after `from`; the content must be non-empty.
fn find_close(chars: &[char], from: usize, delim: &str) -> Option<usize> {
    let single_star = delim == "*";
    let mut j = from;
    while j < chars.len() {
        if chars[j] == '\\' {
            j += 2;
            continue;
        }
        if chars[j] == '`' && !delim.starts_with('`') {
            // Skip over inline code so delimiters inside it don't count.
            if let Some(e) = chars[j + 1..].iter().position(|&c| c == '`') {
                j += e + 2;
                continue;
            }
        }
        if single_star && chars[j] == '*' && chars.get(j + 1) == Some(&'*') {
            j += 2;
            continue;
        }
        if starts(chars, j, delim) {
            return (j > from).then_some(j);
        }
        j += 1;
    }
    None
}

fn push_text(out: &mut Vec<Inline>, buf: &mut String, style: Style) {
    if buf.is_empty() {
        return;
    }
    let text = std::mem::take(buf);
    if let Some(Inline::Text {
        text: prev,
        style: s,
    }) = out.last_mut()
    {
        if *s == style {
            prev.push_str(&text);
            return;
        }
    }
    out.push(Inline::Text { text, style });
}

fn inline(chars: &[char], style: Style) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() {
            buf.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '`' {
            if let Some(e) = chars[i + 1..]
                .iter()
                .position(|&x| x == '`')
                .filter(|&e| e > 0)
            {
                push_text(&mut out, &mut buf, style);
                out.push(Inline::Code(chars[i + 1..i + 1 + e].iter().collect()));
                i += e + 2;
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = chars[i + 1..]
                .iter()
                .position(|c| *c == ']')
                .map(|n| i + 1 + n)
            {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = chars[close + 2..]
                        .iter()
                        .position(|c| *c == ')')
                        .map(|n| close + 2 + n)
                    {
                        let url: String = chars[close + 2..end].iter().collect();
                        let label: String = chars[i + 1..close].iter().collect();
                        if !label.is_empty()
                            && (url.starts_with("https://") || url.starts_with("http://"))
                            && !url.chars().any(char::is_whitespace)
                        {
                            push_text(&mut out, &mut buf, style);
                            out.push(Inline::LabeledLink { label, url });
                            i = end + 1;
                            continue;
                        }
                    }
                }
            }
        }
        if c == '<' && chars.get(i + 1) == Some(&':') {
            if let Some(end) = chars[i + 2..]
                .iter()
                .take(96)
                .position(|c| *c == '>')
                .map(|n| i + 2 + n)
            {
                let token: String = chars[i + 2..end].iter().collect();
                if let Some((name, id)) = token
                    .rsplit_once(':')
                    .and_then(|(name, id)| id.parse::<u64>().ok().map(|id| (name, id)))
                {
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        push_text(&mut out, &mut buf, style);
                        out.push(Inline::Emoji {
                            name: name.into(),
                            id: Some(id),
                        });
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        if c == ':' {
            if let Some(end) = chars[i + 1..]
                .iter()
                .take(33)
                .position(|c| *c == ':')
                .map(|n| i + 1 + n)
            {
                let name: String = chars[i + 1..end].iter().collect();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    push_text(&mut out, &mut buf, style);
                    out.push(Inline::Emoji { name, id: None });
                    i = end + 1;
                    continue;
                }
            }
        }
        let word_start = i == 0 || !chars[i - 1].is_alphanumeric();
        if word_start && (starts(chars, i, "http://") || starts(chars, i, "https://")) {
            let len = chars[i..].iter().take_while(|c| !c.is_whitespace()).count();
            push_text(&mut out, &mut buf, style);
            out.push(Inline::Link(chars[i..i + len].iter().collect()));
            i += len;
            continue;
        }
        // (delimiter, style change); longest first.
        type StyleChange = (&'static str, fn(&mut Style));
        let delims: [StyleChange; 7] = [
            ("***", |s| {
                s.bold = true;
                s.italic = true;
            }),
            ("**", |s| s.bold = true),
            ("__", |s| s.underline = true),
            ("~~", |s| s.strike = true),
            ("*", |s| s.italic = true),
            ("_", |s| s.italic = true),
            ("||", |_| {}),
        ];
        let mut matched = false;
        for (d, apply) in delims {
            if !starts(chars, i, d) {
                continue;
            }
            if d == "_" && !word_start {
                continue; // snake_case stays literal
            }
            let n = d.chars().count();
            if let Some(end) = find_close(chars, i + n, d) {
                push_text(&mut out, &mut buf, style);
                let body = &chars[i + n..end];
                if d == "||" {
                    out.push(Inline::Spoiler(inline(body, style)));
                } else {
                    let mut s = style;
                    apply(&mut s);
                    out.extend(inline(body, s));
                }
                i = end + n;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        buf.push(c);
        i += 1;
    }
    push_text(&mut out, &mut buf, style);
    out
}

// -------------------------------------------------------------- renderer

pub fn render(ui: &mut egui::Ui, text: &str) {
    // Scope ids by text so spoiler state is stable and distinct per message.
    ui.push_id(text, |ui| render_blocks(ui, text));
}

type EmojiImages = Vec<(u64, String, egui::TextureHandle)>;
pub fn render_custom(ui: &mut egui::Ui, text: &str, images: EmojiImages) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::unique("message-emoji-images"), images));
    render(ui, text);
}

fn render_blocks(ui: &mut egui::Ui, text: &str) {
    let font_size = ui.style().text_styles[&egui::TextStyle::Body].size;
    let blocks = parse(text);
    let mut counter = 0usize;
    for block in &blocks {
        match block {
            Block::Heading { level, content } => {
                let text = inline_text(content);
                ui.add_space(6.0);
                ui.label(
                    RichText::new(text)
                        .strong()
                        .size(
                            font_size / 15.0
                                * match level {
                                    1 => 26.0,
                                    2 => 22.0,
                                    _ => 18.0,
                                },
                        )
                        .color(theme::TEXT_BRIGHT),
                );
            }
            Block::Paragraph(inlines) => render_line(ui, inlines, &mut counter, None),
            Block::Code { text, lang } => {
                egui::Frame::new()
                    .fill(theme::color(theme::RAIL))
                    .corner_radius(4)
                    .inner_margin(8)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(lang.as_deref().unwrap_or("Código"))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                            if ui.small_button("Copiar").clicked() {
                                ui.ctx().copy_text(text.clone());
                            }
                        });
                        egui::ScrollArea::horizontal()
                            .id_salt(("code", counter))
                            .show(ui, |ui| {
                                let mut job = syntax_job(text, lang.as_deref());
                                for section in &mut job.sections {
                                    section.format.font_id.size *= font_size / 15.0;
                                }
                                ui.add(
                                    egui::Label::new(job)
                                        .selectable(true)
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            });
                        counter += 1;
                    });
            }
            Block::Quote(lines) => {
                let rect = egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 0))
                    .show(ui, |ui| {
                        for l in lines {
                            render_line(ui, l, &mut counter, Some(theme::TEXT_MUTED));
                        }
                    })
                    .response
                    .rect;
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        rect.left_top(),
                        egui::pos2(rect.left() + 3.0, rect.bottom()),
                    ),
                    2.0,
                    theme::TEXT_FAINT,
                );
            }
            Block::List { start, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(s) => format!("{}.", s + n as u64),
                        None => "•".to_string(),
                    };
                    ui.horizontal_top(|ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new(marker).size(font_size));
                        ui.vertical(|ui| render_line(ui, item, &mut counter, None));
                    });
                }
            }
        }
    }
}

fn render_line(ui: &mut egui::Ui, inlines: &[Inline], counter: &mut usize, color: Option<Color32>) {
    if inlines.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        render_inlines(ui, inlines, counter, color, None);
    });
}

fn render_inlines(
    ui: &mut egui::Ui,
    inlines: &[Inline],
    counter: &mut usize,
    color: Option<Color32>,
    hidden: Option<egui::Id>,
) {
    let font_size = ui.style().text_styles[&egui::TextStyle::Body].size;
    for inl in inlines {
        match inl {
            Inline::Text { text, style } => {
                let mut rt = RichText::new(text).size(font_size);
                if style.bold {
                    rt = rt.strong();
                }
                if style.italic {
                    rt = rt.italics();
                }
                if style.underline {
                    rt = rt.underline();
                }
                if style.strike {
                    rt = rt.strikethrough();
                }
                leaf(ui, rt, color, hidden);
            }
            Inline::Code(text) => {
                let rt = RichText::new(text)
                    .monospace()
                    .size(font_size * 14.0 / 15.0)
                    .background_color(theme::RAIL);
                leaf(ui, rt, color, hidden);
            }
            Inline::Link(url) => {
                if hidden.is_some() {
                    leaf(ui, RichText::new(url).size(font_size), color, hidden);
                } else {
                    widgets::link(ui, url, url);
                }
            }
            Inline::LabeledLink { label, url } => {
                if hidden.is_some() {
                    leaf(ui, RichText::new(label).size(font_size), color, hidden);
                } else {
                    widgets::link(ui, label, url);
                }
            }
            Inline::Emoji { name, id } => {
                let image = ui
                    .ctx()
                    .data(|d| d.get_temp::<EmojiImages>(egui::Id::unique("message-emoji-images")))
                    .and_then(|images| {
                        images.into_iter().find(|(image_id, image_name, _)| {
                            id.map_or(image_name.as_str() == name.as_str(), |id| id == *image_id)
                        })
                    });
                if hidden.is_none() {
                    if let Some((_, _, texture)) = image {
                        let side = font_size * 24.0 / 15.0;
                        ui.add(egui::Image::new((texture.id(), egui::vec2(side, side))))
                            .on_hover_text(format!(":{name}:"));
                    } else {
                        leaf(
                            ui,
                            RichText::new(if let Some(id) = id {
                                format!("<:{name}:{id}>")
                            } else {
                                format!(":{name}:")
                            })
                            .size(font_size),
                            color,
                            None,
                        );
                    }
                } else {
                    leaf(
                        ui,
                        RichText::new(format!(":{name}:")).size(font_size),
                        color,
                        hidden,
                    );
                }
            }
            Inline::Spoiler(children) => {
                let id = ui.scope_id().with(("spoiler", *counter));
                *counter += 1;
                let revealed = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                if revealed {
                    render_inlines(ui, children, counter, color, None);
                } else {
                    render_inlines(ui, children, counter, color, Some(id));
                }
            }
        }
    }
}

fn leaf(ui: &mut egui::Ui, rt: RichText, color: Option<Color32>, hidden: Option<egui::Id>) {
    if let Some(id) = hidden {
        let rt = rt.color(theme::INPUT).background_color(theme::INPUT);
        let resp = ui
            .add(egui::Label::new(rt).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(id, true));
        }
    } else {
        let rt = match color {
            Some(c) => rt.color(c),
            None => rt,
        };
        ui.add(egui::Label::new(rt).selectable(true));
    }
}

fn inline_text(items: &[Inline]) -> String {
    items
        .iter()
        .map(|item| match item {
            Inline::Text { text, .. } | Inline::Code(text) | Inline::Link(text) => text.clone(),
            Inline::LabeledLink { label, .. } => label.clone(),
            Inline::Emoji { name, .. } => format!(":{name}:"),
            Inline::Spoiler(children) => inline_text(children),
        })
        .collect()
}

/// Small lexical highlighter for the common chat languages. Unknown languages
/// remain plain code; source text is preserved exactly and never executed.
fn syntax_job(text: &str, lang: Option<&str>) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let language = lang.unwrap_or("").to_ascii_lowercase();
    let supported = matches!(
        language.as_str(),
        "rust"
            | "rs"
            | "javascript"
            | "js"
            | "typescript"
            | "ts"
            | "python"
            | "py"
            | "sql"
            | "json"
            | "c"
            | "cpp"
            | "cs"
            | "go"
            | "java"
            | "sh"
            | "bash"
    );
    let append = |job: &mut egui::text::LayoutJob, value: &str, color: Color32| {
        job.append(
            value,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::monospace(14.0),
                color,
                ..Default::default()
            },
        )
    };
    if !supported {
        append(&mut job, text, theme::TEXT);
        return job;
    }
    let keywords = [
        "fn", "let", "mut", "pub", "use", "mod", "impl", "struct", "enum", "trait", "self", "Self",
        "return", "if", "else", "match", "for", "while", "in", "loop", "break", "continue",
        "const", "static", "async", "await", "move", "type", "where", "class", "function", "var",
        "new", "import", "from", "export", "default", "def", "with", "as", "try", "except",
        "catch", "throw", "raise", "finally", "pass", "lambda", "yield", "select", "from", "where",
        "join", "on", "insert", "into", "update", "delete", "create", "table", "order", "by",
        "limit", "and", "or", "not",
    ];
    let mut i = 0;
    while i < text.len() {
        let start = i;
        let c = text[i..].chars().next().unwrap();
        let line_comment = text[i..].starts_with("//")
            || (matches!(language.as_str(), "python" | "py" | "sh" | "bash") && c == '#')
            || (language == "sql" && text[i..].starts_with("--"));
        let color = if line_comment {
            i += text[i..].find('\n').unwrap_or(text.len() - i);
            theme::TEXT_MUTED
        } else if c == '"' || c == '\'' {
            i += c.len_utf8();
            let mut escaped = false;
            while i < text.len() {
                let next = text[i..].chars().next().unwrap();
                i += next.len_utf8();
                if !escaped && next == c {
                    break;
                }
                escaped = !escaped && next == '\\';
            }
            theme::GREEN
        } else if c.is_ascii_digit() {
            i += 1;
            while i < text.len() && text.as_bytes()[i].is_ascii_alphanumeric() {
                i += 1;
            }
            theme::YELLOW
        } else if c.is_ascii_alphabetic() || c == '_' {
            i += 1;
            while i < text.len()
                && (text.as_bytes()[i].is_ascii_alphanumeric() || text.as_bytes()[i] == b'_')
            {
                i += 1;
            }
            let word = &text[start..i];
            if keywords.contains(&word.to_ascii_lowercase().as_str()) {
                theme::ACCENT.lerp_to_gamma(theme::TEXT_BRIGHT, 0.4)
            } else if ["true", "false", "null", "None", "Some", "Ok", "Err"].contains(&word) {
                theme::YELLOW
            } else {
                theme::TEXT
            }
        } else {
            i += c.len_utf8();
            theme::TEXT
        };
        append(&mut job, &text[start..i], color);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str, bold: bool, italic: bool, underline: bool, strike: bool) -> Inline {
        Inline::Text {
            text: text.into(),
            style: Style {
                bold,
                italic,
                underline,
                strike,
            },
        }
    }
    fn plain(text: &str) -> Inline {
        t(text, false, false, false, false)
    }

    #[test]
    fn empty_text() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n  \n").is_empty());
        assert!(parse_inline("").is_empty());
    }

    #[test]
    fn headings_and_safe_labeled_links() {
        assert!(matches!(
            &parse("## Olá")[0],
            Block::Heading { level: 2, .. }
        ));
        assert_eq!(
            parse_inline("[site](https://example.com)"),
            vec![Inline::LabeledLink {
                label: "site".into(),
                url: "https://example.com".into()
            }]
        );
        assert!(!parse_inline("[run](javascript:alert(1))")
            .iter()
            .any(|i| matches!(i, Inline::LabeledLink { .. })));
        assert!(matches!(&parse("#### texto")[0], Block::Paragraph(_)));
    }

    #[test]
    fn highlighter_preserves_unicode_source() {
        let source = "let ação = \"Olá\"; // comentário\n42";
        let job = syntax_job(source, Some("rust"));
        assert_eq!(job.text, source);
        assert!(job.sections.len() > 3);
        assert_eq!(syntax_job(source, Some("unknown")).text, source);
    }

    #[test]
    fn simple_styles() {
        assert_eq!(
            parse_inline("a **b** c"),
            vec![plain("a "), t("b", true, false, false, false), plain(" c")]
        );
        assert_eq!(parse_inline("*i*"), vec![t("i", false, true, false, false)]);
        assert_eq!(parse_inline("_i_"), vec![t("i", false, true, false, false)]);
        assert_eq!(
            parse_inline("__u__"),
            vec![t("u", false, false, true, false)]
        );
        assert_eq!(
            parse_inline("~~s~~"),
            vec![t("s", false, false, false, true)]
        );
    }

    #[test]
    fn combined_and_nested() {
        assert_eq!(
            parse_inline("***x***"),
            vec![t("x", true, true, false, false)]
        );
        assert_eq!(
            parse_inline("**a *b* c**"),
            vec![
                t("a ", true, false, false, false),
                t("b", true, true, false, false),
                t(" c", true, false, false, false)
            ]
        );
        assert_eq!(
            parse_inline("__~~x~~__"),
            vec![t("x", false, false, true, true)]
        );
        assert_eq!(
            parse_inline("||**x** y||"),
            vec![Inline::Spoiler(vec![
                t("x", true, false, false, false),
                plain(" y")
            ])]
        );
    }

    #[test]
    fn unclosed_stays_literal() {
        assert_eq!(parse_inline("**abc"), vec![plain("**abc")]);
        assert_eq!(parse_inline("a * b"), vec![plain("a * b")]);
        assert_eq!(parse_inline("||oops"), vec![plain("||oops")]);
        assert_eq!(parse_inline("`x"), vec![plain("`x")]);
        assert_eq!(parse_inline("**"), vec![plain("**")]);
        assert!(!parse_inline("~~a **b~~ c**").is_empty());
    }

    #[test]
    fn inline_code_is_verbatim() {
        assert_eq!(parse_inline("`**x**`"), vec![Inline::Code("**x**".into())]);
        assert_eq!(
            parse_inline("**a `b*` c**"),
            vec![
                t("a ", true, false, false, false),
                Inline::Code("b*".into()),
                t(" c", true, false, false, false)
            ]
        );
    }

    #[test]
    fn escapes_and_snake_case() {
        assert_eq!(parse_inline(r"\*a\*"), vec![plain("*a*")]);
        assert_eq!(
            parse_inline("snake_case_name"),
            vec![plain("snake_case_name")]
        );
    }

    #[test]
    fn links() {
        assert_eq!(
            parse_inline("see https://a.b/c_d_ ok"),
            vec![
                plain("see "),
                Inline::Link("https://a.b/c_d_".into()),
                plain(" ok")
            ]
        );
    }

    #[test]
    fn code_blocks() {
        assert_eq!(
            parse("```rust\nlet a = 1;\n**x**\n```"),
            vec![Block::Code {
                lang: Some("rust".into()),
                text: "let a = 1;\n**x**".into()
            }]
        );
        assert_eq!(
            parse("```hi```"),
            vec![Block::Code {
                lang: None,
                text: "hi".into()
            }]
        );
        assert_eq!(
            parse("```\n```"),
            vec![Block::Code {
                lang: None,
                text: String::new()
            }]
        );
        // Unclosed fence is literal text.
        assert_eq!(
            parse("```\nabc"),
            vec![
                Block::Paragraph(vec![plain("```")]),
                Block::Paragraph(vec![plain("abc")])
            ]
        );
    }

    #[test]
    fn lists_and_quotes() {
        assert_eq!(
            parse("- a\n- **b**"),
            vec![Block::List {
                start: None,
                items: vec![vec![plain("a")], vec![t("b", true, false, false, false)]]
            }]
        );
        assert_eq!(
            parse("3. a\n4. b"),
            vec![Block::List {
                start: Some(3),
                items: vec![vec![plain("a")], vec![plain("b")]]
            }]
        );
        assert_eq!(
            parse("> a\n> *b*\nc"),
            vec![
                Block::Quote(vec![
                    vec![plain("a")],
                    vec![t("b", false, true, false, false)]
                ]),
                Block::Paragraph(vec![plain("c")])
            ]
        );
        assert_eq!(parse("-"), vec![Block::Paragraph(vec![plain("-")])]);
    }
}
