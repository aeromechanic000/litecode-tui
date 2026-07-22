use crate::agent::summarizer::SummarizerConfig;

use crate::config::Config;
use crate::prompt::PromptBuilder;
use crate::session::Session;
use crate::skills::SkillRegistry;
use crate::working_set::WorkingSet;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single message in the conversation history sent to the LLM as context.
#[derive(Debug, Clone)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    pub tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppMode {
    Plan,
    Edit,
    Auto,
}

impl std::fmt::Display for AppMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppMode::Plan => write!(f, "PLAN"),
            AppMode::Edit => write!(f, "EDIT"),
            AppMode::Auto => write!(f, "AUTO"),
        }
    }
}

impl AppMode {
    pub fn cycle(self) -> Self {
        match self {
            AppMode::Plan => AppMode::Edit,
            AppMode::Edit => AppMode::Auto,
            AppMode::Auto => AppMode::Plan,
        }
    }

    #[allow(dead_code)]
    pub fn can_write_file(self) -> bool {
        !matches!(self, AppMode::Plan)
    }

    #[allow(dead_code)]
    pub fn can_execute_command(self) -> bool {
        !matches!(self, AppMode::Plan)
    }

    #[allow(dead_code)]
    pub fn needs_confirmation(self) -> bool {
        matches!(self, AppMode::Edit)
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "plan" => AppMode::Plan,
            "auto" => AppMode::Auto,
            _ => AppMode::Edit,
        }
    }
}

pub struct AppState {
    pub mode: AppMode,
    pub config: Config,
    pub workspace: PathBuf,
    pub web_search_enabled: bool,
    pub think_enabled: bool,
    pub pending_confirmations: Vec<PendingAction>,
    pub awaiting_confirmation: bool,
    pub input_history: Vec<String>,
    pub history_index: usize,
    pub skills: SkillRegistry,
    pub is_processing: bool,
    pub pending_queue: Vec<String>,
    pub conversation_history: Vec<ConversationMessage>,
    pub pending_plan: Option<String>,
    pub prompt_builder: PromptBuilder,
    pub conversation_summary: Option<String>,
    pub summarizer_config: SummarizerConfig,
    pub working_set: WorkingSet,
    pub current_session: Session,
    pub awaiting_destructive_confirm: bool,
    pub awaiting_other_input: bool,
    pub snapshot_manager: crate::snapshot::SnapshotManager,
    pub event_sink: crate::hooks::JsonlSink,
    /// Rough estimate of the last request's prompt token count, used to
    /// compute `ctx:N%` against `config.context_window_limit`. Not a KV-cache
    /// measurement — native tool calling goes through `/api/chat`, which does
    /// not expose the KV-cache context handle.
    pub total_prompt_tokens: usize,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    WriteFile {
        path: PathBuf,
        content: String,
        #[allow(dead_code)]
        diff_preview: String,
    },
    DeleteFile {
        path: PathBuf,
    },
    #[allow(dead_code)]
    ExecuteCommand {
        cmd: String,
        args: Vec<String>,
    },
}

impl AppState {
    pub fn new(config: Config, workspace: PathBuf) -> Self {
        let mode = AppMode::from_str_lossy(&config.default_mode);

        let skills = Config::skills_dir()
            .map(|dir| SkillRegistry::load_from_dir(&dir))
            .unwrap_or_else(|_| SkillRegistry::empty());

        let mut prompt_builder = PromptBuilder::new(&config);
        prompt_builder.set_mode(mode);
        prompt_builder.set_skills(&skills);
        let tool_registry = crate::tools::ToolRegistry::new(workspace.clone(), &config);
        prompt_builder.set_tools(&tool_registry);

        let config_dir = Config::effective_dir(&workspace);
        let instructions = crate::prompt::ProjectInstructions::discover(&workspace, &config_dir);
        prompt_builder.set_project_context(instructions);

        let input_history = Self::load_input_history();

        Self {
            mode,
            config: config.clone(),
            workspace: workspace.clone(),
            web_search_enabled: config.enable_free_web_search,
            think_enabled: true,
            pending_confirmations: Vec::new(),
            awaiting_confirmation: false,
            input_history,
            history_index: 0,
            skills,
            is_processing: false,
            pending_queue: Vec::new(),
            conversation_history: Vec::new(),
            pending_plan: None,
            prompt_builder,
            conversation_summary: None,
            summarizer_config: SummarizerConfig::default(),
            working_set: WorkingSet::new(),
            current_session: Session::new(),
            awaiting_destructive_confirm: false,
            awaiting_other_input: false,
            snapshot_manager: crate::snapshot::SnapshotManager::new(&workspace, &config_dir),
            total_prompt_tokens: 0,
            event_sink: crate::hooks::JsonlSink::open(
                &config_dir.join("logs").join("events.jsonl"),
            )
            .unwrap_or_else(|_| {
                // Fallback to /tmp if config dir is unavailable
                crate::hooks::JsonlSink::open(std::path::Path::new("/tmp/litepilot-events.jsonl"))
                    .expect("Cannot create event sink")
            }),
        }
    }

    pub fn switch_mode(&mut self) -> AppMode {
        self.mode = self.mode.cycle();
        self.mode
    }

    pub fn push_pending(&mut self, action: PendingAction) {
        self.pending_confirmations.push(action);
    }

    const MAX_HISTORY: usize = 500;
    const HISTORY_FILE: &str = "input_history.json";

    fn history_path() -> Option<PathBuf> {
        Config::config_dir().ok().map(|d| d.join(Self::HISTORY_FILE))
    }

    fn load_input_history() -> Vec<String> {
        let path = match Self::history_path() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        serde_json::from_str(&data).unwrap_or_else(|_| Vec::new())
    }

    pub fn save_input_history(&self) {
        let path = match Self::history_path() {
            Some(p) => p,
            None => return,
        };
        if let Ok(data) = serde_json::to_string(&self.input_history) {
            let _ = std::fs::write(&path, data);
        }
    }

    pub fn record_input(&mut self, input: String) {
        if !input.is_empty() {
            self.input_history.push(input);
            if self.input_history.len() > Self::MAX_HISTORY {
                self.input_history.drain(..self.input_history.len() - Self::MAX_HISTORY);
            }
            self.history_index = 0;
            self.save_input_history();
        }
    }

    #[allow(dead_code)]
    pub fn pop_pending(&mut self) -> Option<PendingAction> {
        self.pending_confirmations.pop()
    }

    pub fn clear_pending(&mut self) {
        self.pending_confirmations.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        AppState::new(Config::default(), PathBuf::from("/tmp/test"))
    }

    #[test]
    fn mode_cycle() {
        assert_eq!(AppMode::Plan.cycle(), AppMode::Edit);
        assert_eq!(AppMode::Edit.cycle(), AppMode::Auto);
        assert_eq!(AppMode::Auto.cycle(), AppMode::Plan);
    }

    #[test]
    fn plan_permissions() {
        let mode = AppMode::Plan;
        assert!(!mode.can_write_file());
        assert!(!mode.can_execute_command());
        assert!(!mode.needs_confirmation());
    }

    #[test]
    fn edit_permissions() {
        let mode = AppMode::Edit;
        assert!(mode.can_write_file());
        assert!(mode.can_execute_command());
        assert!(mode.needs_confirmation());
    }

    #[test]
    fn auto_permissions() {
        let mode = AppMode::Auto;
        assert!(mode.can_write_file());
        assert!(mode.can_execute_command());
        assert!(!mode.needs_confirmation());
    }

    #[test]
    fn state_switch_mode() {
        let mut state = test_state();
        assert_eq!(state.mode, AppMode::Edit);
        state.switch_mode();
        assert_eq!(state.mode, AppMode::Auto);
        state.switch_mode();
        assert_eq!(state.mode, AppMode::Plan);
    }

    #[test]
    fn pending_actions() {
        let mut state = test_state();
        state.push_pending(PendingAction::ExecuteCommand {
            cmd: "cargo".into(),
            args: vec!["build".into()],
        });
        assert_eq!(state.pending_confirmations.len(), 1);
        let popped = state.pop_pending();
        assert!(popped.is_some());
        assert!(state.pending_confirmations.is_empty());
    }

    #[test]
    fn from_str_lossy() {
        assert_eq!(AppMode::from_str_lossy("plan"), AppMode::Plan);
        assert_eq!(AppMode::from_str_lossy("edit"), AppMode::Edit);
        assert_eq!(AppMode::from_str_lossy("auto"), AppMode::Auto);
        assert_eq!(AppMode::from_str_lossy("unknown"), AppMode::Edit);
    }
}
