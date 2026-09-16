//! Markdown subset renderer (Concord parity):
//! headings `#/##/###`, quotes `>`, bullets `-/*`, bold `**`, italic `*`,
//! underline `__`, strike `~~`, inline code, fenced code blocks with
//! syntect highlighting, raw URLs underlined.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use syntect::{highlighting::ThemeSet, parsing::SyntaxSet};
use unicode_width::UnicodeWidthStr;

pub fn render_markdown<'a>(input: &'a str) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    let mut in_fence: Option<String> = None;
    let mut fence_buf: Vec<&'a str> = Vec::new();

    for raw in input.lines() {
        if let Some(rest) = raw.strip_prefix("```") {
            if in_fence.is_some() {
                let lang = in_fence.take().unwrap_or_default();
                out.extend(render_code_block(&fence_buf, &lang));
                fence_buf.clear();
            } else {
                in_fence = Some(rest.trim().to_string());
            }
            continue;
        }
        if in_fence.is_some() {
            fence_buf.push(raw);
            continue;
        }
        out.push(render_line(raw));
    }
    if !fence_buf.is_empty() {
        let lang = in_fence.unwrap_or_default();
        out.extend(render_code_block(&fence_buf, &lang));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

fn render_line<'a>(raw: &'a str) -> Line<'a> {
    // Headings
    for (prefix, style) in [
        ("### ", Style::new().bold()),
        ("## ", Style::new().bold().underline_color(Color::Gray)),
        ("# ", Style::new().fg(Color::Cyan).bold()),
    ] {
        if let Some(body) = raw.strip_prefix(prefix) {
            let mut spans = vec![Span::styled(prefix, Style::new().fg(Color::DarkGray))];
            spans.extend(inline(body));
            return Line::from(spans).style(style);
        }
    }
    if let Some(body) = raw.strip_prefix("> ") {
        let mut spans = vec![Span::styled("▌ ", Style::new().fg(Color::DarkGray))];
        spans.extend(inline(body));
        return Line::from(spans).style(Style::new().fg(Color::Gray));
    }
    for prefix in ["- ", "* "] {
        if let Some(body) = raw.strip_prefix(prefix) {
            let mut spans = vec![Span::styled("• ", Style::new().fg(Color::DarkGray))];
            spans.extend(inline(body));
            return Line::from(spans);
        }
    }
    Line::from(inline(raw))
}

fn inline<'a>(s: &'a str) -> Vec<Span<'a>> {
    // Tiny recursive-descent-ish scanner for **bold**, *italic*, __underline__,
    // ~~strike~~, `code`, URLs. Good enough for chat; Concord parity subset.
    let mut spans = Vec::new();
    let mut i = 0;
    let mut plain_start = 0;

    macro_rules! flush_plain {
        ($up_to:expr) => {
            if $up_to > plain_start {
                spans.extend(linkify(&s[plain_start..$up_to]));
            }
        };
    }

    while i < s.len() {
        if !s.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &s[i..];
        if let Some(end) = find_closer(rest, "**") {
            if end > 2 {
                flush_plain!(i);
                let inner = &rest[2..end];
                spans.push(Span::styled(inner.to_string(), Style::new().bold()));
                i += end + 2;
                plain_start = i;
                continue;
            }
        }
        if let Some(end) = find_closer(rest, "~~") {
            if end > 2 {
                flush_plain!(i);
                let inner = &rest[2..end];
                spans.push(Span::styled(
                    inner.to_string(),
                    Style::new().add_modifier(Modifier::CROSSED_OUT),
                ));
                i += end + 2;
                plain_start = i;
                continue;
            }
        }
        if let Some(end) = find_closer(rest, "__") {
            if end > 2 {
                flush_plain!(i);
                let inner = &rest[2..end];
                spans.push(Span::styled(inner.to_string(), Style::new().underlined()));
                i += end + 2;
                plain_start = i;
                continue;
            }
        }
        if rest.starts_with('`') {
            if let Some(end) = rest[1..].find('`') {
                flush_plain!(i);
                let inner = &rest[1..1 + end];
                spans.push(Span::styled(
                    inner.to_string(),
                    Style::new().fg(Color::Rgb(255, 165, 0)),
                ));
                i += 1 + end + 1;
                plain_start = i;
                continue;
            }
        }
        // *italic* (single, avoid **)
        if rest.starts_with('*') && !rest.starts_with("**") {
            if let Some(end) = rest[1..].find('*') {
                flush_plain!(i);
                let inner = &rest[1..1 + end];
                spans.push(Span::styled(inner.to_string(), Style::new().italic()));
                i += 1 + end + 1;
                plain_start = i;
                continue;
            }
        }
        // Advance by one char (UTF-8 safe for emoji).
        i += rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    flush_plain!(s.len());
    if spans.is_empty() {
        spans.push(Span::raw(""));
    }
    spans
}

fn find_closer(s: &str, delim: &str) -> Option<usize> {
    let tail = s.get(delim.len()..)?;
    tail.find(delim).map(|p| p + delim.len())
}

fn linkify<'a>(chunk: &'a str) -> Vec<Span<'a>> {
    // Underline http(s):// URLs (Concord: `o` opens them).
    let mut out = Vec::new();
    let mut rest = chunk;
    while let Some(pos) = rest.find("http") {
        let (before, after) = rest.split_at(pos);
        if !before.is_empty() {
            out.push(Span::raw(before.to_string()));
        }
        let end = after
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after.len());
        let (url, tail) = after.split_at(end);
        if url.starts_with("http://") || url.starts_with("https://") {
            out.push(Span::styled(url.to_string(), Style::new().fg(Color::Cyan).underlined()));
        } else {
            out.push(Span::raw(url.to_string()));
        }
        rest = tail;
    }
    if !rest.is_empty() {
        out.push(Span::raw(rest.to_string()));
    }
    out
}

fn render_code_block<'a>(lines: &[&'a str], lang: &str) -> Vec<Line<'a>> {
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];
    let syntax = ps.find_syntax_by_token(lang).unwrap_or_else(|| ps.find_syntax_plain_text());
    let mut out = vec![Line::from(Span::styled(
        format!(" {} ", if lang.is_empty() { "code" } else { lang }),
        Style::new().fg(Color::Black).bg(Color::DarkGray),
    ))];
    let mut h = syntect::easy::HighlightLines::new(syntax, theme);
    for line in lines {
        let text = format!("{line}\n");
        let mut spans = Vec::new();
        for (style, part) in h.highlight_line(&text, &ps).unwrap_or_default() {
            let fg = style.foreground;
            // Strip trailing newline syntect keeps; ratatui wants clean spans.
            let part = part.trim_end_matches('\n').to_string();
            if part.is_empty() {
                continue;
            }
            spans.push(Span::styled(
                part,
                Style::new().fg(Color::Rgb(fg.r, fg.g, fg.b)),
            ));
        }
        if spans.is_empty() {
            spans.push(Span::raw(""));
        }
        out.push(Line::from(spans));
    }
    out.push(Line::from(Span::styled("─".repeat(24), Style::new().fg(Color::DarkGray))));
    out
}

/// Fuzzy matcher (Concord `tui/fuzzy.rs` parity): subsequence scoring with
/// exact/prefix bonus. Returns score (higher = better) or None.
pub fn fuzzy_score(query: &str, target: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    if t.contains(&q) {
        let mut score = 100i64;
        if t.starts_with(&q) {
            score += 50;
        }
        return Some(score - (t.len() as i64 - q.len() as i64));
    }
    let mut qi = q.chars();
    let mut cur = qi.next()?;
    let mut matched = 0;
    for c in t.chars() {
        if c == cur {
            matched += 1;
            if let Some(n) = qi.next() {
                cur = n;
            } else {
                return Some(matched as i64 * 10 - t.len() as i64);
            }
        }
    }
    None
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut w = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        if w + cw > max.saturating_sub(1) {
            out.push('…');
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_basics() {
        let lines = render_markdown("# hi\n**bold** and `code` https://example.com");
        assert!(lines.len() >= 2);
    }

    #[test]
    fn fuzzy() {
        assert!(fuzzy_score("gen", "general").unwrap() > fuzzy_score("gen", "random").unwrap_or(-999));
        assert!(fuzzy_score("xyz", "general").is_none());
    }
}
