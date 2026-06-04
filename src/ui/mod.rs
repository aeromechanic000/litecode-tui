pub mod theme;
pub mod inline;

use crate::app::AppState;
use crate::ui::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::path::PathBuf;

pub struct UiState {
    pub theme: Theme,
    pub input_text: String,
    pub input_cursor: usize,
    /// Output lines kept for session persistence (not rendered in viewport)
    pub output_lines: Vec<OutputLine>,
    /// Lines queued for insert_before() rendering above the viewport
    pub pending_inline: Vec<OutputLine>,
    /// Partial streaming line text rendered in the viewport activity row
    pub streaming_partial: String,
    /// Accumulates streaming tokens for assistant output
    pub streaming_buffer: String,
    /// Whether the thinking indicator is active (rendered in viewport)
    pub is_thinking: bool,
    /// Tick counter for animating the thinking indicator dots
    pub thinking_tick: u8,
    pub workspace_hint: PathBuf,
    /// Stores full multi-line paste content; input_text shows the summary
    paste_buffer: Option<String>,
    /// Remembers the last user input for plan approval flow
    pub last_user_input: String,
}

#[derive(Debug, Clone)]
pub enum OutputLine {
    User(String),
    Assistant(String),
    System(String),
    Error(String),
    #[allow(dead_code)]
    Code {
        language: String,
        code: String,
    },
    #[allow(dead_code)]
    Diff {
        added: Vec<String>,
        removed: Vec<String>,
    },
    #[allow(dead_code)]
    Thinking(()),
    Pending(String),
    Plan(String),
    Separator,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            input_text: String::new(),
            input_cursor: 0,
            output_lines: Vec::new(),
            pending_inline: Vec::new(),
            streaming_partial: String::new(),
            streaming_buffer: String::new(),
            is_thinking: false,
            thinking_tick: 0,
            workspace_hint: PathBuf::new(),
            paste_buffer: None,
            last_user_input: String::new(),
        }
    }
}

impl UiState {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            theme: Theme::from_config(&config.theme),
            ..Self::default()
        }
    }

    pub fn with_workspace(mut self, workspace: PathBuf) -> Self {
        self.workspace_hint = workspace;
        self
    }
}

impl UiState {
    /// Add an output line. Queues it for insert_before() rendering and keeps
    /// it in output_lines for session persistence.
    pub fn add_output(&mut self, line: OutputLine) {
        // Don't render Thinking inline — it's shown in the viewport
        if !matches!(line, OutputLine::Thinking(_)) {
            self.pending_inline.push(line.clone());
        }
        self.output_lines.push(line);
    }

    /// Append a streaming token chunk. Complete lines are flushed to
    /// pending_inline; the partial remainder is shown in the viewport.
    pub fn append_stream_chunk(&mut self, chunk: &str) {
        self.streaming_buffer.push_str(chunk);

        // Flush complete lines to pending_inline
        while let Some(pos) = self.streaming_buffer.find('\n') {
            let line: String = self.streaming_buffer[..pos].to_string();
            self.streaming_buffer = self.streaming_buffer[pos + 1..].to_string();
            if line.is_empty() {
                continue;
            }
            self.pending_inline
                .push(OutputLine::Assistant(line));
        }

        // Update the partial line for viewport rendering
        self.streaming_partial = self.streaming_buffer.clone();
    }

    /// Show the thinking indicator in the viewport.
    pub fn start_thinking(&mut self) {
        self.is_thinking = true;
        self.thinking_tick = 0;
    }

    /// Hide the thinking indicator. Returns true if thinking was active.
    pub fn stop_thinking(&mut self) -> bool {
        if self.is_thinking {
            self.is_thinking = false;
            true
        } else {
            false
        }
    }

    /// Finish streaming — finalizes the buffer. Returns the full content.
    pub fn finish_stream(&mut self) -> String {
        // Flush any remaining partial line
        if !self.streaming_partial.is_empty() {
            self.pending_inline
                .push(OutputLine::Assistant(self.streaming_partial.clone()));
            self.streaming_partial.clear();
        }
        let content = std::mem::take(&mut self.streaming_buffer);
        if content.is_empty()
            && !self
                .output_lines
                .last()
                .is_some_and(|l| matches!(l, OutputLine::Assistant(_)))
        {
            self.add_output(OutputLine::Assistant(String::new()));
        }
        content
    }

    #[allow(dead_code)]
    pub fn clear_output(&mut self) {
        self.output_lines.clear();
        self.pending_inline.clear();
        self.streaming_partial.clear();
        self.streaming_buffer.clear();
        self.is_thinking = false;
    }

    pub fn push_char(&mut self, c: char) {
        self.paste_buffer = None;
        self.input_text.insert(self.input_cursor, c);
        self.input_cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.paste_buffer.is_some() {
            self.paste_buffer = None;
            self.input_text.clear();
            self.input_cursor = 0;
            return;
        }
        if self.input_cursor > 0 {
            let prev = self.input_text[..self.input_cursor]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.input_cursor -= prev;
            self.input_text.remove(self.input_cursor);
        }
    }

    pub fn take_input(&mut self) -> String {
        let input = self
            .paste_buffer
            .take()
            .unwrap_or_else(|| self.input_text.clone());
        self.input_text.clear();
        self.input_cursor = 0;
        input
    }

    /// Handle pasted text.
    pub fn set_paste(&mut self, text: String) {
        let line_count = text.lines().count();
        if line_count > 1 {
            self.paste_buffer = Some(text);
            self.input_text = format!("[{} copied lines]", line_count);
            self.input_cursor = self.input_text.len();
        } else {
            self.paste_buffer = None;
            for c in text.chars() {
                self.input_text.insert(self.input_cursor, c);
                self.input_cursor += c.len_utf8();
            }
        }
    }
}

/// Draw the 3-row inline viewport: activity | status | input.
pub fn draw(f: &mut Frame, app: &AppState, ui: &mut UiState) {
    let area = f.area();
    let rows = split_rows(area, 3);

    draw_activity_row(f, ui, rows[0]);
    draw_compact_status(f, app, ui, rows[1]);
    draw_input_row(f, ui, rows[2]);
    set_input_cursor(f, ui, rows[2]);
}

fn split_rows(area: Rect, count: u16) -> Vec<Rect> {
    let mut rows = Vec::with_capacity(count as usize);
    for i in 0..count {
        rows.push(Rect::new(area.x, area.y + i, area.width, 1));
    }
    rows
}

fn think_mode_label(enabled: &bool) -> &'static str {
    if *enabled {
        "THINK"
    } else {
        "DIRECT"
    }
}

/// Row 0: compact status line with mode, context usage, workspace.
fn draw_compact_status(f: &mut Frame, app: &AppState, ui: &UiState, area: Rect) {
    let theme = &ui.theme;
    let (mode_label, mode_color) = theme.mode_indicator(&app.mode);
    let think_label = think_mode_label(&app.think_enabled);

    let think_color = if app.think_enabled {
        theme.accent
    } else {
        theme.warning
    };

    let core_model = &app.config.exec_model;
    let context_window = crate::ollama::model::estimate_context_window(core_model);
    let usage_pct = app.context_manager.context_usage_percent(context_window);
    let usage_color = if usage_pct >= 100.0 {
        Color::Red
    } else if usage_pct >= 80.0 {
        theme.warning
    } else {
        theme.accent
    };

    let mut spans = vec![
        Span::styled(
            format!("[{}]", mode_label),
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("[{}]", think_label),
            Style::default().fg(think_color),
        ),
        Span::raw(" "),
        Span::styled(
            format!("ctx:{:.0}%", usage_pct),
            Style::default().fg(usage_color),
        ),
    ];

    if !app.pending_queue.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("({} queued)", app.pending_queue.len()),
            Style::default().fg(theme.accent),
        ));
    }

    // Workspace (right-aligned would be complex; just append with separator)
    let ws = truncate_path(&app.workspace.to_string_lossy(), 30);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(ws, Style::default().fg(Color::DarkGray)));

    let status = Paragraph::new(Line::from(spans));
    f.render_widget(status, area);
}

/// Row 1: streaming partial line or thinking indicator.
fn draw_activity_row(f: &mut Frame, ui: &mut UiState, area: Rect) {
    let theme = &ui.theme;

    if ui.is_thinking {
        ui.thinking_tick = ui.thinking_tick.wrapping_add(1);
        let dots = ".".repeat(((ui.thinking_tick / 4) % 3 + 1) as usize);
        let line = Line::from(Span::styled(
            format!("\u{2234} thinking{}", dots), // ∴
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::ITALIC),
        ));
        let para = Paragraph::new(line);
        f.render_widget(para, area);
    } else if !ui.streaming_partial.is_empty() {
        let line = Line::from(vec![
            Span::styled("\u{25cf} ", Style::default()), // ●
            Span::raw(ui.streaming_partial.clone()),
        ]);
        let para = Paragraph::new(line);
        f.render_widget(para, area);
    } else {
        // Separator line
        let line = Line::from(Span::styled(
            "\u{2500}".repeat(area.width as usize), // ────
            Style::default().fg(Color::DarkGray),
        ));
        let para = Paragraph::new(line);
        f.render_widget(para, area);
    }
}

/// Row 2: input prompt.
fn draw_input_row(f: &mut Frame, ui: &UiState, area: Rect) {
    let theme = &ui.theme;
    let text_width = area.width.saturating_sub(3) as usize;

    // Truncate input display to fit
    let display_text = if ui.input_text.chars().count() > text_width {
        let chars: Vec<char> = ui.input_text.chars().collect();
        let start = chars.len().saturating_sub(text_width);
        chars[start..].iter().collect()
    } else {
        ui.input_text.clone()
    };

    let line = Line::from(vec![
        Span::styled(
            " > ",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(display_text),
    ]);
    let para = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::NONE),
    );
    f.render_widget(para, area);
}

/// Position the terminal cursor in the input row.
fn set_input_cursor(f: &mut Frame, ui: &UiState, area: Rect) {
    let text_width = area.width.saturating_sub(3) as usize;
    if text_width == 0 {
        return;
    }

    let chars_before: usize = ui.input_text[..ui.input_cursor].chars().count();
    let chars_total: usize = ui.input_text.chars().count();

    // If input overflows, the display is right-aligned
    let display_offset = if chars_total > text_width {
        chars_before.saturating_sub(chars_total.saturating_sub(text_width))
    } else {
        chars_before
    };

    let x = area.x + 3 + display_offset.min(text_width) as u16;
    let y = area.y;

    f.set_cursor_position((x, y));
}

fn truncate_path(path: &str, max: usize) -> String {
    if path.len() <= max {
        path.to_string()
    } else {
        format!("...{}", &path[path.len() - max + 3..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_state_push_and_take_input() {
        let mut ui = UiState::default();
        for c in "hello".chars() {
            ui.push_char(c);
        }
        assert_eq!(ui.input_text, "hello");
        let taken = ui.take_input();
        assert_eq!(taken, "hello");
        assert!(ui.input_text.is_empty());
    }

    #[test]
    fn ui_state_backspace() {
        let mut ui = UiState::default();
        for c in "ab".chars() {
            ui.push_char(c);
        }
        ui.backspace();
        assert_eq!(ui.input_text, "a");
        ui.backspace();
        assert_eq!(ui.input_text, "");
        ui.backspace(); // no-op on empty
        assert_eq!(ui.input_text, "");
    }

    #[test]
    fn ui_state_add_output() {
        let mut ui = UiState::default();
        ui.add_output(OutputLine::User("test".into()));
        ui.add_output(OutputLine::Assistant("response".into()));
        assert_eq!(ui.output_lines.len(), 2);
        assert_eq!(ui.pending_inline.len(), 2);
    }

    #[test]
    fn thinking_not_queued_inline() {
        let mut ui = UiState::default();
        ui.add_output(OutputLine::Thinking(()));
        assert_eq!(ui.output_lines.len(), 1);
        assert!(ui.pending_inline.is_empty()); // Thinking not queued
    }

    #[test]
    fn streaming_flushes_complete_lines() {
        let mut ui = UiState::default();
        ui.append_stream_chunk("Hello ");
        ui.append_stream_chunk("world\nNext ");
        assert_eq!(ui.pending_inline.len(), 1); // "Hello world" flushed
        assert_eq!(ui.streaming_partial, "Next ");
    }

    #[test]
    fn finish_stream_flushes_partial() {
        let mut ui = UiState::default();
        ui.append_stream_chunk("partial line");
        assert!(ui.pending_inline.is_empty());
        ui.finish_stream();
        assert_eq!(ui.pending_inline.len(), 1); // partial flushed
    }

    #[test]
    fn truncate_path_fn() {
        assert_eq!(truncate_path("/short", 10), "/short");
        let long = "/very/long/path/to/some/file";
        let truncated = truncate_path(long, 15);
        assert!(truncated.starts_with("..."));
        assert!(truncated.len() <= 15);
    }
}
