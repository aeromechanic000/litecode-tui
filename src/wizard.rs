use crate::config::Config;
use crate::ollama;
use crate::ollama::model::ModelInfo;
use crate::ui::theme::Theme;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use std::io;

#[derive(Debug, Clone, PartialEq)]
pub enum WizardStep {
    UrlInput,
    Connecting,
    ContextSelect,
    ModelSelect,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModelSlot {
    Exec,
    Eval,
}

impl ModelSlot {
    fn label(&self) -> &'static str {
        match self {
            ModelSlot::Exec => "Exec",
            ModelSlot::Eval => "Eval",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            ModelSlot::Exec => "Main coding & planning",
            ModelSlot::Eval => "Review & quality assurance",
        }
    }

    fn next(self) -> Option<Self> {
        match self {
            ModelSlot::Exec => Some(ModelSlot::Eval),
            ModelSlot::Eval => None,
        }
    }
}

const CONTEXT_OPTIONS: [u64; 5] = [65536, 131072, 262144, 524288, 1048576];
const CONTEXT_LABELS: [&str; 5] = ["64k", "128k", "256k", "512k", "1M"];
const DEFAULT_CONTEXT_INDEX: usize = 2; // 256k

struct WizardState {
    step: WizardStep,
    url: String,
    input_text: String,
    input_cursor: usize,
    models: Vec<ModelInfo>,
    selected_index: usize,
    scroll_offset: usize,
    current_slot: ModelSlot,
    exec_model: Option<String>,
    eval_model: Option<String>,
    error_msg: Option<String>,
    context_index: usize,
}

impl WizardState {
    fn new(existing_config: &Config) -> Self {
        let url = if existing_config.ollama_endpoint.is_empty() {
            "http://127.0.0.1:11434".to_string()
        } else {
            existing_config.ollama_endpoint.clone()
        };
        let mut state = Self {
            step: WizardStep::UrlInput,
            url: url.clone(),
            input_text: url.clone(),
            input_cursor: url.len(),
            models: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            current_slot: ModelSlot::Exec,
            exec_model: None,
            eval_model: None,
            error_msg: None,
            context_index: DEFAULT_CONTEXT_INDEX,
        };
        // Pre-fill from existing config if present
        if !existing_config.exec_model.is_empty() {
            state.exec_model = Some(existing_config.exec_model.clone());
        }
        if !existing_config.eval_model.is_empty() {
            state.eval_model = Some(existing_config.eval_model.clone());
        }
        // Pre-fill context window from config
        if let Some(idx) = CONTEXT_OPTIONS
            .iter()
            .position(|&v| v == existing_config.context_window_limit)
        {
            state.context_index = idx;
        }
        state
    }

    fn push_char(&mut self, c: char) {
        self.input_text.insert(self.input_cursor, c);
        self.input_cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
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

    /// Position selected_index on the model already chosen for the current slot
    fn jump_to_existing_model(&mut self) {
        let existing = match self.current_slot {
            ModelSlot::Exec => self.exec_model.as_deref(),
            ModelSlot::Eval => self.eval_model.as_deref(),
        };
        if let Some(name) = existing {
            if let Some(idx) = self.models.iter().position(|m| m.name == name) {
                self.selected_index = idx;
                // Adjust scroll so selected item is visible
                let visible = 10; // approximate visible items
                if idx < self.scroll_offset {
                    self.scroll_offset = idx;
                } else if idx >= self.scroll_offset + visible {
                    self.scroll_offset = idx.saturating_sub(visible / 2);
                }
            }
        }
    }

    fn selected_model_for_slot(&self, slot: ModelSlot) -> &Option<String> {
        match slot {
            ModelSlot::Exec => &self.exec_model,
            ModelSlot::Eval => &self.eval_model,
        }
    }

    fn set_model_for_slot(&mut self, slot: ModelSlot, name: String) {
        match slot {
            ModelSlot::Exec => self.exec_model = Some(name),
            ModelSlot::Eval => self.eval_model = Some(name),
        }
    }

    fn step_number(&self) -> usize {
        match self.step {
            WizardStep::UrlInput => 1,
            WizardStep::Connecting => 2,
            WizardStep::ContextSelect => 3,
            WizardStep::ModelSelect => 4,
            WizardStep::Confirm => 5,
        }
    }

    fn into_config(self, base: &Config) -> Config {
        Config {
            ollama_endpoint: self.url,
            exec_model: self.exec_model.unwrap_or_default(),
            eval_model: self.eval_model.unwrap_or_default(),
            context_window_limit: CONTEXT_OPTIONS[self.context_index],
            ..base.clone()
        }
    }
}

/// Run the setup wizard. Returns the updated config.
pub fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: Config,
    workspace: &std::path::Path,
) -> Result<Config> {
    let mut state = WizardState::new(&config);
    let theme = Theme::from_config(&config.theme);

    loop {
        terminal.draw(|f| draw_wizard(f, &state, &theme))?;

        // Perform the connection attempt after the "Connecting" screen has rendered
        if state.step == WizardStep::Connecting {
            try_connect(&mut state);
            continue;
        }

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match handle_key(&mut state, key.modifiers, key.code) {
                    Action::Continue => {}
                    Action::Save => {
                        let new_config = state.into_config(&config);
                        let config_path = Config::config_path_for(workspace);
                        new_config.save(&config_path)?;
                        return Ok(new_config);
                    }
                    Action::Quit => {
                        return Ok(config);
                    }
                }
            }
        }
    }
}

enum Action {
    Continue,
    Save,
    Quit,
}

fn handle_key(state: &mut WizardState, modifiers: KeyModifiers, code: KeyCode) -> Action {
    match state.step {
        WizardStep::UrlInput => handle_url_input(state, modifiers, code),
        WizardStep::Connecting => Action::Continue,
        WizardStep::ContextSelect => handle_context_select(state, modifiers, code),
        WizardStep::ModelSelect => handle_model_select(state, modifiers, code),
        WizardStep::Confirm => handle_confirm(state, modifiers, code),
    }
}

/// Attempt to connect to Ollama and fetch models. Called after the "Connecting" screen renders.
fn try_connect(state: &mut WizardState) {
    let url = state.url.clone();
    let result: Result<Vec<ModelInfo>> = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            let temp_config = Config {
                ollama_endpoint: url,
                connect_timeout: 10,
                ..Config::default()
            };
            let client = ollama::OllamaClient::new(&temp_config)?;
            client.ping().await?;
            client.list_models().await
        })
    })
    .join()
    .unwrap_or(Err(anyhow::anyhow!("Thread panicked")));

    match result {
        Ok(models) => {
            if models.is_empty() {
                state.error_msg =
                    Some("No models found. Pull models with: ollama pull <model>".into());
            } else {
                state.models = models;
                state.selected_index = 0;
                state.scroll_offset = 0;
                state.current_slot = ModelSlot::Exec;
                state.jump_to_existing_model();
            }
            state.step = WizardStep::ContextSelect;
        }
        Err(e) => {
            state.error_msg = Some(format!("Connection failed: {}", e));
            state.step = WizardStep::UrlInput;
        }
    }
}

fn handle_url_input(state: &mut WizardState, modifiers: KeyModifiers, code: KeyCode) -> Action {
    match (modifiers, code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
        (KeyModifiers::NONE, KeyCode::Enter) => {
            let url = if state.input_text.is_empty() {
                "http://127.0.0.1:11434".to_string()
            } else {
                state.input_text.clone()
            };
            state.url = url;
            state.error_msg = None;
            state.step = WizardStep::Connecting;
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            state.backspace();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Char(c)) => {
            state.push_char(c);
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Left) => {
            if state.input_cursor > 0 {
                state.input_cursor -= 1;
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Right) => {
            if state.input_cursor < state.input_text.len() {
                state.input_cursor += 1;
            }
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn handle_context_select(
    state: &mut WizardState,
    modifiers: KeyModifiers,
    code: KeyCode,
) -> Action {
    match (modifiers, code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
        (KeyModifiers::NONE, KeyCode::Left) => {
            if state.context_index > 0 {
                state.context_index -= 1;
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Right) => {
            if state.context_index < CONTEXT_OPTIONS.len() - 1 {
                state.context_index += 1;
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            state.step = WizardStep::ModelSelect;
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Esc) => {
            state.step = WizardStep::UrlInput;
            state.error_msg = None;
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn handle_model_select(state: &mut WizardState, modifiers: KeyModifiers, code: KeyCode) -> Action {
    match (modifiers, code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
        (KeyModifiers::NONE, KeyCode::Up) => {
            if state.selected_index > 0 {
                state.selected_index -= 1;
                if state.selected_index < state.scroll_offset {
                    state.scroll_offset = state.selected_index;
                }
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            if !state.models.is_empty() && state.selected_index < state.models.len() - 1 {
                state.selected_index += 1;
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            if state.models.is_empty() {
                // Manual input mode — use typed text as model name
                let name = state.input_text.trim().to_string();
                if !name.is_empty() {
                    state.set_model_for_slot(state.current_slot, name);
                    state.input_text.clear();
                    state.input_cursor = 0;
                }
            } else {
                let name = state.models[state.selected_index].name.clone();
                state.set_model_for_slot(state.current_slot, name);
            }
            advance_slot(state)
        }
        (KeyModifiers::NONE, KeyCode::Tab) => {
            // Skip this slot
            advance_slot(state)
        }
        (KeyModifiers::NONE, KeyCode::Esc) => {
            state.step = WizardStep::UrlInput;
            state.error_msg = None;
            Action::Continue
        }
        // Manual text input when no models
        (KeyModifiers::NONE, KeyCode::Backspace) if state.models.is_empty() => {
            state.backspace();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Char(c)) if state.models.is_empty() => {
            state.push_char(c);
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn advance_slot(state: &mut WizardState) -> Action {
    match state.current_slot.next() {
        Some(next) => {
            state.current_slot = next;
            state.selected_index = 0;
            state.scroll_offset = 0;
            state.jump_to_existing_model();
            Action::Continue
        }
        None => {
            state.step = WizardStep::Confirm;
            Action::Continue
        }
    }
}

fn handle_confirm(state: &mut WizardState, modifiers: KeyModifiers, code: KeyCode) -> Action {
    match (modifiers, code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
        (KeyModifiers::NONE, KeyCode::Enter) => {
            // Ensure at least exec_model is set
            if state.exec_model.is_none() && !state.models.is_empty() {
                state.exec_model = Some(state.models[0].name.clone());
            }
            Action::Save
        }
        (KeyModifiers::NONE, KeyCode::Esc) => {
            state.current_slot = ModelSlot::Exec;
            state.selected_index = 0;
            state.scroll_offset = 0;
            state.jump_to_existing_model();
            state.step = WizardStep::ContextSelect;
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn draw_wizard(f: &mut ratatui::Frame, state: &WizardState, theme: &Theme) {
    let size = f.area();

    // Full-screen background — same as main UI
    let bg = Block::default();
    f.render_widget(bg, size);

    // Centered content area
    let content_width = std::cmp::min(size.width.saturating_sub(4), 64);
    let content_height = std::cmp::min(size.height.saturating_sub(4), 30);
    let x = (size.width.saturating_sub(content_width)) / 2;
    let y = (size.height.saturating_sub(content_height)) / 2;
    let area = Rect::new(x, y, content_width, content_height);

    let step_label = format!("Step {}/5", state.step_number());
    let title = format!(" LitePilot Setup — {} ", step_label);

    let container = Block::default()
        .title(Span::styled(
            &title,
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.primary))
        .style(Style::default());
    f.render_widget(container, area);

    let inner = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(4),
    );

    match state.step {
        WizardStep::UrlInput => draw_url_input(f, state, theme, inner),
        WizardStep::Connecting => draw_connecting(f, state, theme, inner),
        WizardStep::ContextSelect => draw_context_select(f, state, theme, inner),
        WizardStep::ModelSelect => draw_model_select(f, state, theme, inner),
        WizardStep::Confirm => draw_confirm(f, state, theme, inner),
    }
}

fn draw_url_input(f: &mut ratatui::Frame, state: &WizardState, theme: &Theme, area: Rect) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "LitePilot Setup",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Confirm or edit your Ollama connection settings."),
        Line::from("Press Enter to accept and continue."),
        Line::from(""),
        Line::from(Span::styled(
            "Ollama URL:",
            Style::default().fg(theme.accent),
        )),
    ];

    if let Some(ref err) = state.error_msg {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {}", err),
            Style::default().fg(theme.warning),
        )));
        lines.push(Line::from(""));
    }

    let text_line_count = lines.len() as u16;
    let content = Paragraph::new(lines).style(Style::default());
    f.render_widget(content, area);

    // Input field — positioned right after the text lines
    let input_y = area.y + text_line_count;
    let input_area = Rect::new(area.x, input_y, area.width, 3);
    let input_display = format!(" {}_", &state.input_text);
    let input = Paragraph::new(input_display).style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(input, input_area);

    // Help bar
    let help_y = area.y + area.height - 1;
    let help =
        Paragraph::new("Enter: connect  |  Ctrl+C: exit").style(Style::default().fg(theme.accent));
    f.render_widget(help, Rect::new(area.x, help_y, area.width, 1));
}

fn draw_connecting(f: &mut ratatui::Frame, state: &WizardState, theme: &Theme, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("Connecting to {}...", state.url),
            Style::default().fg(theme.primary),
        )),
        Line::from(""),
        Line::from("Please wait..."),
    ];
    let content = Paragraph::new(lines)
        .style(Style::default())
        .alignment(Alignment::Center);
    f.render_widget(content, area);
}

fn draw_context_select(f: &mut ratatui::Frame, state: &WizardState, theme: &Theme, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "Context Window Limit",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Maximum context size for model input + output."),
        Line::from("Larger values allow longer conversations but use more memory."),
        Line::from(""),
        Line::from("Select a size:"),
    ];
    let text_height = lines.len() as u16;
    let content = Paragraph::new(lines).style(Style::default());
    f.render_widget(content, area);

    // Draw option pills in a row
    let pill_y = area.y + text_height + 1;
    let pill_width = 8u16;
    let total_width = CONTEXT_LABELS.len() as u16 * (pill_width + 2);
    let start_x = area.x + (area.width.saturating_sub(total_width)) / 2;

    for (i, label) in CONTEXT_LABELS.iter().enumerate() {
        let is_selected = i == state.context_index;
        let x = start_x + (i as u16) * (pill_width + 2);

        let style = if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(theme.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent)
        };

        let pill = Paragraph::new(format!(" {} ", label))
            .style(style)
            .alignment(Alignment::Center);
        f.render_widget(pill, Rect::new(x, pill_y, pill_width, 1));
    }

    // Help bar
    let help_y = area.y + area.height - 1;
    let help = Paragraph::new("\u{2190}\u{2192}: select  |  Enter: continue  |  Esc: back")
        .style(Style::default().fg(theme.accent));
    f.render_widget(help, Rect::new(area.x, help_y, area.width, 1));
}

fn draw_model_select(f: &mut ratatui::Frame, state: &WizardState, theme: &Theme, area: Rect) {
    let max_visible = (area.height.saturating_sub(6)) as usize;

    // Header
    let header = vec![
        Line::from(Span::styled(
            format!("Select model for: {}", state.current_slot.label()),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            state.current_slot.description(),
            Style::default(),
        )),
        Line::from(""),
    ];

    let header_para = Paragraph::new(header).style(Style::default());
    f.render_widget(header_para, area);

    let list_y = area.y + 3;
    let list_height = area.height.saturating_sub(6);

    if state.models.is_empty() {
        // No models — show manual input
        let no_model_lines = vec![
            Line::from(Span::styled(
                if let Some(ref err) = state.error_msg {
                    format!("  {}", err)
                } else {
                    "  No models available.".into()
                },
                Style::default().fg(theme.warning),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Type a model name and press Enter:",
                Style::default(),
            )),
        ];
        let para = Paragraph::new(no_model_lines).style(Style::default());
        f.render_widget(para, Rect::new(area.x, list_y, area.width, list_height));

        // Input
        let input_y = list_y + list_height.saturating_sub(3);
        let input = Paragraph::new(format!(" {}_", state.input_text))
            .style(Style::default().add_modifier(Modifier::BOLD));
        f.render_widget(input, Rect::new(area.x, input_y, area.width, 1));
    } else {
        // Model list
        let visible_end = std::cmp::min(state.scroll_offset + max_visible, state.models.len());
        let mut list_lines: Vec<Line> = Vec::new();

        for (i, model) in state.models[state.scroll_offset..visible_end]
            .iter()
            .enumerate()
        {
            let actual_idx = i + state.scroll_offset;
            let is_selected = actual_idx == state.selected_index;

            let indicator = if is_selected { " > " } else { "   " };
            let size_info = format!(
                "{:>8}  {}",
                model.size_class.to_string(),
                model.quantization_level
            );
            let family_info = if model.family.is_empty() {
                String::new()
            } else {
                format!("  [{}]", model.family)
            };

            let style = if is_selected {
                Style::default()
                    .bg(theme.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            list_lines.push(Line::from(vec![
                Span::styled(indicator, style),
                Span::styled(format!("{:<24}", model.name), style),
                Span::styled(format!("{}{}", size_info, family_info), style),
            ]));
        }

        let list_para = Paragraph::new(list_lines).style(Style::default());
        f.render_widget(
            list_para,
            Rect::new(area.x, list_y, area.width, list_height),
        );
    }

    // Already selected
    let selected_y = area.y + area.height - 3;
    let mut selected_lines = vec![Line::from(Span::styled(
        "Selected:",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    for slot in &[ModelSlot::Exec, ModelSlot::Eval] {
        if let Some(ref name) = *state.selected_model_for_slot(*slot) {
            let marker = if *slot == state.current_slot {
                ">"
            } else {
                " "
            };
            selected_lines.push(Line::from(format!(
                "  {} {}: {}",
                marker,
                slot.label(),
                name
            )));
        } else if *slot == state.current_slot {
            selected_lines.push(Line::from(format!("  > {}: (selecting...)", slot.label())));
        }
    }
    let sel_para = Paragraph::new(selected_lines).style(Style::default());
    f.render_widget(sel_para, Rect::new(area.x, selected_y, area.width, 3));

    // Help bar
    let help_y = area.y + area.height - 1;
    let help = Paragraph::new("Enter: select  |  Tab: skip  |  Esc: back")
        .style(Style::default().fg(theme.accent));
    f.render_widget(help, Rect::new(area.x, help_y, area.width, 1));
}

fn draw_confirm(f: &mut ratatui::Frame, state: &WizardState, theme: &Theme, area: Rect) {
    let context_label = CONTEXT_LABELS[state.context_index];
    let mut lines = vec![
        Line::from(Span::styled(
            "Configuration Summary",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Ollama URL: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(&state.url, Style::default().fg(theme.primary)),
        ]),
        Line::from(vec![
            Span::styled(
                "  Context:    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} tokens", context_label),
                Style::default().fg(theme.primary),
            ),
        ]),
        Line::from(""),
    ];

    for (slot, model) in &[
        (ModelSlot::Exec, &state.exec_model),
        (ModelSlot::Eval, &state.eval_model),
    ] {
        let model_display = model
            .as_deref()
            .unwrap_or("(not set — will use exec model)");
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>12}: ", slot.label()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(model_display, Style::default().fg(theme.primary)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter: confirm and start  |  Esc: re-select models",
        Style::default(),
    )));

    let content = Paragraph::new(lines).style(Style::default());
    f.render_widget(content, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_slot_order() {
        assert_eq!(ModelSlot::Exec.next(), Some(ModelSlot::Eval));
        assert_eq!(ModelSlot::Eval.next(), None);
    }

    #[test]
    fn model_slot_labels() {
        assert!(ModelSlot::Exec.label().contains("Exec"));
        assert!(ModelSlot::Eval.label().contains("Eval"));
    }

    #[test]
    fn wizard_state_new_defaults() {
        let config = Config::default();
        let state = WizardState::new(&config);
        assert_eq!(state.step, WizardStep::UrlInput);
        assert_eq!(state.url, "http://127.0.0.1:11434");
        assert_eq!(state.current_slot, ModelSlot::Exec);
        assert!(state.exec_model.is_none());
        assert!(state.eval_model.is_none());
    }

    #[test]
    fn wizard_state_presets_from_config() {
        let config = Config {
            ollama_endpoint: "http://custom:9999".into(),
            exec_model: "qwen3:8b".into(),
            eval_model: "qwen3:14b".into(),
            ..Config::default()
        };
        let state = WizardState::new(&config);
        assert_eq!(state.url, "http://custom:9999");
        assert_eq!(state.exec_model.as_deref(), Some("qwen3:8b"));
        assert_eq!(state.eval_model.as_deref(), Some("qwen3:14b"));
    }

    #[test]
    fn wizard_input_editing() {
        let config = Config::default();
        let mut state = WizardState::new(&config);
        state.input_text.clear();
        state.input_cursor = 0;
        for c in "hello".chars() {
            state.push_char(c);
        }
        assert_eq!(state.input_text, "hello");
        assert_eq!(state.input_cursor, 5);
        state.backspace();
        assert_eq!(state.input_text, "hell");
        assert_eq!(state.input_cursor, 4);
    }

    #[test]
    fn set_model_for_slot() {
        let config = Config::default();
        let mut state = WizardState::new(&config);
        state.set_model_for_slot(ModelSlot::Exec, "qwen3:8b".into());
        assert_eq!(state.exec_model.as_deref(), Some("qwen3:8b"));
        state.set_model_for_slot(ModelSlot::Eval, "qwen3:14b".into());
        assert_eq!(state.eval_model.as_deref(), Some("qwen3:14b"));
    }

    #[test]
    fn into_config_applies_wizard_values() {
        let config = Config::default();
        let mut state = WizardState::new(&config);
        state.url = "http://ollama:1234".into();
        state.exec_model = Some("qwen3:8b".into());
        state.eval_model = Some("qwen3:14b".into());
        state.context_index = 3; // 512k
        let result = state.into_config(&config);
        assert_eq!(result.ollama_endpoint, "http://ollama:1234");
        assert_eq!(result.exec_model, "qwen3:8b");
        assert_eq!(result.eval_model, "qwen3:14b");
        assert_eq!(result.context_window_limit, 524288);
        // Other fields preserved from base config
        assert_eq!(result.default_mode, "edit");
    }

    #[test]
    fn step_number() {
        let config = Config::default();
        let mut state = WizardState::new(&config);
        assert_eq!(state.step_number(), 1);
        state.step = WizardStep::Connecting;
        assert_eq!(state.step_number(), 2);
        state.step = WizardStep::ContextSelect;
        assert_eq!(state.step_number(), 3);
        state.step = WizardStep::ModelSelect;
        assert_eq!(state.step_number(), 4);
        state.step = WizardStep::Confirm;
        assert_eq!(state.step_number(), 5);
    }
}
