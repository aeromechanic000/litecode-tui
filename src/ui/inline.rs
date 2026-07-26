//! Inline output rendering via `terminal.insert_before()`.
//!
//! Renders OutputLine variants into a ratatui Buffer for insertion above
//! the inline viewport. Content scrolls into the terminal's native scrollback.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::{OutputLine, theme::Theme};

/// Wrap a string into lines that fit within `max_width` visible columns.
/// Handles multi-byte characters correctly.
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut col: usize = 0;
        for ch in line.chars() {
            let w = char_width(ch);
            if col + w > max_width && !current.is_empty() {
                result.push(current);
                current = String::new();
                col = 0;
            }
            current.push(ch);
            col += w;
        }
        if !current.is_empty() || line.is_empty() {
            result.push(current);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Visible display width of a character (CJK = 2, else 1).
fn char_width(ch: char) -> usize {
    if ch.is_ascii() {
        1
    } else if is_cjk(ch) {
        2
    } else {
        1
    }
}

fn is_cjk(ch: char) -> bool {
    let c = ch as u32;
    (0x4E00..=0x9FFF).contains(&c)      // CJK Unified Ideographs
        || (0x3000..=0x33FF).contains(&c) // CJK Symbols, Hiragana, Katakana
        || (0xF900..=0xFAFF).contains(&c) // CJK Compatibility Ideographs
        || (0xFF00..=0xFFEF).contains(&c) // Fullwidth forms
}

/// Render a slice of OutputLine into the given buffer area for `insert_before()`.
pub fn render_output_lines(buf: &mut Buffer, lines: &[OutputLine], theme: &Theme) {
    let area = buf.area;
    let width = area.width as usize;
    let mut row: u16 = 0;

    for ol in lines {
        if row >= area.height {
            break;
        }
        let rendered = render_line(ol, theme, width);
        for line in rendered {
            if row >= area.height {
                break;
            }
            let line_rect = Rect::new(area.x, area.y + row, area.width, 1);
            let paragraph = ratatui::widgets::Paragraph::new(line);
            paragraph.render(line_rect, buf);
            row += 1;
        }
    }
}

/// Render a single OutputLine into one or more styled Lines, wrapping to `width`.
fn render_line(ol: &OutputLine, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    match ol {
        OutputLine::User(text) => {
            let prefix = "\u{25b6} "; // ▶
            let prefix_w = 2;
            wrap_styled(text, prefix, prefix_w, width, |_s| {
                Style::default().add_modifier(Modifier::BOLD)
            }, |_| {
                Style::default().add_modifier(Modifier::BOLD)
            })
        }

        OutputLine::Assistant(text) => {
            if text.is_empty() {
                return vec![Line::from(Span::styled(
                    "\u{25cf} ", // ●
                    Style::default(),
                ))];
            }
            let prefix = "\u{25cf} "; // ●
            let prefix_w = 2;
            wrap_plain_text(text, prefix, prefix_w, width)
        }

        OutputLine::System(text) => {
            let prefix = "\u{203b} "; // ※
            let prefix_w = 2;
            wrap_plain_text_styled(
                text,
                prefix,
                prefix_w,
                width,
                Style::default().fg(Color::Reset),
            )
        }

        OutputLine::Error(text) => {
            let prefix = "\u{2717} "; // ✗
            let prefix_w = 2;
            wrap_styled(
                text,
                prefix,
                prefix_w,
                width,
                |_| Style::default().fg(theme.warning).add_modifier(Modifier::BOLD),
                |_| Style::default().fg(theme.warning),
            )
        }

        OutputLine::Code { language, code } => {
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("\u{250c}\u{2500} {} \u{2500}", language), // ┌─ lang ─
                Style::default().fg(theme.accent),
            )));
            let indent = 2;
            let content_w = width.saturating_sub(indent);
            for code_line in code.lines() {
                for wrapped in wrap_text(code_line, content_w) {
                    lines.push(Line::from(Span::raw(format!("{}{}", " ".repeat(indent), wrapped))));
                }
            }
            lines.push(Line::from(Span::styled(
                "\u{2514}\u{2500}", // └─
                Style::default().fg(theme.accent),
            )));
            lines
        }

        OutputLine::Diff { added, removed } => {
            let mut lines = Vec::new();
            for r in removed {
                for wrapped in wrap_text(&format!("- {}", r), width) {
                    lines.push(Line::from(Span::styled(wrapped, Style::default().fg(Color::Red))));
                }
            }
            for a in added {
                for wrapped in wrap_text(&format!("+ {}", a), width) {
                    lines.push(Line::from(Span::styled(wrapped, Style::default().fg(Color::Green))));
                }
            }
            lines
        }

        OutputLine::Thinking(_) => vec![],

        OutputLine::Pending(text) => {
            let prefix = "\u{25b8} "; // ▸
            let prefix_w = 2;
            wrap_plain_text_styled(
                &format!("{} (queued)", text),
                prefix,
                prefix_w,
                width,
                Style::default().fg(theme.accent),
            )
        }

        OutputLine::Plan(plan) => {
            let mut lines = vec![Line::from(Span::styled(
                "\u{25c6} Plan", // ◆
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            ))];
            let indent = 2;
            let content_w = width.saturating_sub(indent);
            for step in plan.lines() {
                let trimmed = step.trim();
                if !trimmed.is_empty() {
                    for wrapped in wrap_text(trimmed, content_w) {
                        lines.push(Line::from(Span::raw(format!("{}{}", " ".repeat(indent), wrapped))));
                    }
                }
            }
            lines
        }

        OutputLine::Phase(name) => vec![Line::from(Span::styled(
            format!("\u{25c6} {}", name), // ◆
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ))],

        OutputLine::Separator => vec![Line::from(vec![
            Span::styled(
                "\u{2500}".repeat(20), // ────
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                " done ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "\u{2500}".repeat(20),
                Style::default().fg(Color::DarkGray),
            ),
        ])],
    }
}

/// Wrap multi-line text with a styled prefix on the first line and continuation lines.
fn wrap_styled<F1, F2>(
    text: &str,
    prefix: &str,
    prefix_w: usize,
    width: usize,
    prefix_style: F1,
    text_style: F2,
) -> Vec<Line<'static>>
where
    F1: Fn(&str) -> Style,
    F2: Fn(&str) -> Style,
{
    let content_w = width.saturating_sub(prefix_w);
    let wrapped = wrap_text(text, content_w);
    let mut lines = Vec::new();
    for (i, line) in wrapped.iter().enumerate() {
        if i == 0 {
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), prefix_style(prefix)),
                Span::styled(line.clone(), text_style(line)),
            ]));
        } else {
            lines.push(Line::from(Span::styled(line.clone(), text_style(line))));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            prefix.to_string(),
            prefix_style(prefix),
        )));
    }
    lines
}

/// Wrap multi-line plain text with a prefix marker on the first line.
fn wrap_plain_text(text: &str, prefix: &str, prefix_w: usize, width: usize) -> Vec<Line<'static>> {
    let content_w = width.saturating_sub(prefix_w);
    let mut all_wrapped = Vec::new();
    for (i, text_line) in text.lines().enumerate() {
        let wrapped = wrap_text(text_line, content_w);
        for (j, w) in wrapped.iter().enumerate() {
            if i == 0 && j == 0 {
                all_wrapped.push(Line::from(vec![
                    Span::styled(prefix.to_string(), Style::default()),
                    Span::raw(w.clone()),
                ]));
            } else {
                all_wrapped.push(Line::from(Span::raw(w.clone())));
            }
        }
    }
    if all_wrapped.is_empty() {
        all_wrapped.push(Line::from(Span::styled(
            prefix.to_string(),
            Style::default(),
        )));
    }
    all_wrapped
}

/// Wrap multi-line text with a prefix and uniform text style.
fn wrap_plain_text_styled(
    text: &str,
    prefix: &str,
    prefix_w: usize,
    width: usize,
    style: Style,
) -> Vec<Line<'static>> {
    let content_w = width.saturating_sub(prefix_w);
    let mut all_wrapped = Vec::new();
    for (i, text_line) in text.lines().enumerate() {
        let wrapped = wrap_text(text_line, content_w);
        for (j, w) in wrapped.iter().enumerate() {
            if i == 0 && j == 0 {
                all_wrapped.push(Line::from(vec![
                    Span::styled(prefix.to_string(), style),
                    Span::styled(w.clone(), style),
                ]));
            } else {
                all_wrapped.push(Line::from(Span::styled(w.clone(), style)));
            }
        }
    }
    if all_wrapped.is_empty() {
        all_wrapped.push(Line::from(Span::styled(prefix.to_string(), style)));
    }
    all_wrapped
}

/// Estimate how many terminal rows a set of OutputLines will occupy when rendered.
/// Used as the `line_count` parameter for `insert_before()`.
pub fn estimate_line_count(lines: &[OutputLine], width: u16) -> u16 {
    let w = width as usize;
    let mut count: u16 = 0;
    for ol in lines {
        match ol {
            OutputLine::User(text) => {
                count += estimate_wrapped_lines(text, 2, w);
            }
            OutputLine::Assistant(text) => {
                if text.is_empty() {
                    count += 1;
                } else {
                    let mut first = true;
                    for line in text.lines() {
                        let prefix_w = if first { 2 } else { 0 };
                        count += estimate_wrapped_lines(line, prefix_w, w);
                        first = false;
                    }
                    if count == 0 {
                        count = 1;
                    }
                }
            }
            OutputLine::System(text) => {
                let mut first = true;
                for line in text.lines() {
                    let prefix_w = if first { 2 } else { 0 };
                    count += estimate_wrapped_lines(line, prefix_w, w);
                    first = false;
                }
                count = count.max(1);
            }
            OutputLine::Error(text) => {
                count += estimate_wrapped_lines(text, 2, w);
            }
            OutputLine::Code { code, .. } => {
                count += 2; // header + footer
                let indent = 2;
                for line in code.lines() {
                    count += estimate_wrapped_lines(line, indent, w);
                }
            }
            OutputLine::Diff { added, removed } => {
                for r in removed {
                    count += estimate_wrapped_lines(r, 2, w);
                }
                for a in added {
                    count += estimate_wrapped_lines(a, 2, w);
                }
            }
            OutputLine::Thinking(_) => {}
            OutputLine::Pending(text) => {
                count += estimate_wrapped_lines(text, 2, w);
            }
            OutputLine::Plan(plan) => {
                count += 1; // header
                let indent = 2;
                for step in plan.lines() {
                    let trimmed = step.trim();
                    if !trimmed.is_empty() {
                        count += estimate_wrapped_lines(trimmed, indent, w);
                    }
                }
            }
            OutputLine::Phase(_) => count += 1,
            OutputLine::Separator => count += 1,
        }
    }
    count.max(1)
}

/// Estimate how many rows a single text line occupies after wrapping.
fn estimate_wrapped_lines(text: &str, prefix_w: usize, total_w: usize) -> u16 {
    if total_w == 0 {
        return 1;
    }
    let content_w = total_w.saturating_sub(prefix_w);
    if content_w == 0 {
        return 1;
    }
    let text_width: usize = text.chars().map(char_width).sum();
    if text_width == 0 {
        return 1;
    }
    ((text_width + content_w - 1) / content_w).max(1) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_ascii() {
        let lines = wrap_text("hello world", 5);
        assert_eq!(lines, vec!["hello", " worl", "d"]);
    }

    #[test]
    fn wrap_exact_fit() {
        let lines = wrap_text("hello", 5);
        assert_eq!(lines, vec!["hello"]);
    }

    #[test]
    fn wrap_short() {
        let lines = wrap_text("hi", 10);
        assert_eq!(lines, vec!["hi"]);
    }

    #[test]
    fn wrap_empty() {
        let lines = wrap_text("", 10);
        assert_eq!(lines, vec![""]);
    }

    #[test]
    fn wrap_multi_line() {
        let lines = wrap_text("hello\nworld", 5);
        assert_eq!(lines, vec!["hello", "world"]);
    }

    #[test]
    fn wrap_multi_line_long() {
        let lines = wrap_text("hello world\nfoo bar baz", 5);
        assert_eq!(lines, vec!["hello", " worl", "d", "foo b", "ar ba", "z"]);
    }

    #[test]
    fn estimate_lines_short() {
        assert_eq!(estimate_wrapped_lines("hello", 2, 80), 1);
    }

    #[test]
    fn estimate_lines_long() {
        // 80 chars, width 20, prefix 2 → content_w = 18 → 80/18 ≈ 5
        let text = "a".repeat(80);
        assert_eq!(estimate_wrapped_lines(&text, 2, 20), 5);
    }

    #[test]
    fn estimate_lines_empty() {
        assert_eq!(estimate_wrapped_lines("", 2, 80), 1);
    }

    #[test]
    fn estimate_line_count_user() {
        let lines = vec![OutputLine::User("hello world".into())];
        // width 80, fits in one line
        assert_eq!(estimate_line_count(&lines, 80), 1);
    }

    #[test]
    fn estimate_line_count_long_user() {
        let lines = vec![OutputLine::User("a".repeat(200))];
        // width 20, prefix 2, content 18, 200 chars → ceil(200/18) = 12
        assert_eq!(estimate_line_count(&lines, 20), 12);
    }
}
