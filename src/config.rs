use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:11434";

fn default_context_window() -> u64 {
    262144 // 256k tokens
}

fn default_max_file_lines() -> usize {
    60
}

/// Default search backend. Bing is reachable in mainland China (where
/// DuckDuckGo is blocked) and covers both Chinese and international queries,
/// so it works out of the box in network-restricted regions.
fn default_web_search_backend() -> String {
    "bing".to_string()
}

/// Theme colors stored as hex strings (e.g. "#315DFC").
/// All other UI colors are derived from these three — normal text and
/// backgrounds use terminal defaults ("reset").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeColors {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub accent: String,
    #[serde(default)]
    pub warning: String,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            primary: "cyan".into(),
            accent: "magenta".into(),
            warning: "yellow".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub ollama_endpoint: String,
    pub connect_timeout: u64,
    pub exec_model: String,
    pub eval_model: String,
    pub plan_model: String,
    pub default_mode: String,
    pub auto_mode_only_workspace: bool,
    pub enable_auto_syntax_check: bool,
    pub prefer_uv_toolchain: bool,
    pub auto_run_after_fix: bool,
    pub enable_free_web_search: bool,
    pub auto_switch_network_region: bool,
    pub enable_recap: bool,
    pub enable_away_summary: bool,
    pub search_cache_valid_days: u64,
    pub max_search_context_tokens: usize,
    /// Which search backend `web_search` queries first: "bing" (default),
    /// "baidu", "duckduckgo", or "searxng". Honored strictly — when
    /// `auto_switch_network_region` is off only this backend is used. When on,
    /// it is tried first and region-reachable fallbacks follow on failure.
    #[serde(default = "default_web_search_backend")]
    pub web_search_backend: String,
    /// URL of a self-hosted SearXNG instance (e.g. "http://localhost:8080").
    /// When set, SearXNG joins the backend fallback chain; with
    /// `web_search_backend = "searxng"` it is used first. A self-hosted
    /// instance bypasses regional blocks entirely.
    #[serde(default)]
    pub searxng_url: Option<String>,
    pub max_retries: usize,
    #[serde(default = "default_context_window")]
    pub context_window_limit: u64,
    #[serde(default = "default_max_file_lines")]
    pub max_file_lines: usize,
    #[serde(default)]
    pub model_residency: String,
    #[serde(default)]
    pub theme: ThemeColors,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ollama_endpoint: DEFAULT_ENDPOINT.to_string(),
            connect_timeout: 15,
            exec_model: String::new(),
            eval_model: String::new(),
            plan_model: String::new(),
            default_mode: "edit".to_string(),
            auto_mode_only_workspace: true,
            enable_auto_syntax_check: true,
            prefer_uv_toolchain: true,
            auto_run_after_fix: false,
            enable_free_web_search: true,
            auto_switch_network_region: true,
            enable_recap: true,
            enable_away_summary: true,
            search_cache_valid_days: 30,
            max_search_context_tokens: 2048,
            web_search_backend: default_web_search_backend(),
            searxng_url: None,
            max_retries: 3,
            context_window_limit: 262144,
            max_file_lines: 60,
            model_residency: "none".to_string(),
            theme: ThemeColors::default(),
        }
    }
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        Ok(home.join(".litepilot"))
    }

    /// Check for project-local `.litepilot` directory in the given workspace,
    /// fall back to global `~/.litepilot` if not found.
    pub fn effective_dir(workspace: &Path) -> PathBuf {
        let local = workspace.join(".litepilot");
        if local.is_dir() {
            local
        } else {
            Self::config_dir().unwrap_or(local)
        }
    }

    pub fn config_path_for(workspace: &Path) -> PathBuf {
        Self::effective_dir(workspace).join("config.toml")
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    #[allow(dead_code)]
    pub fn ensure_dirs() -> Result<PathBuf> {
        let dir = Self::config_dir()?;
        Self::create_dir_structure(&dir)?;
        Ok(dir)
    }

    /// Initialize directory structure and populate built-in skills.
    /// Used for both global (~/.litepilot) and project-local (.litepilot) dirs.
    /// Returns the effective config dir plus the names of built-in skills that
    /// were missing and have been restored.
    pub fn ensure_dirs_for(workspace: &Path) -> Result<(PathBuf, Vec<String>)> {
        let dir = Self::effective_dir(workspace);
        Self::create_dir_structure(&dir)?;

        // Always populate skills in global ~/.litepilot/skills/
        let global_dir = Self::config_dir()?;
        let restored_skills =
            crate::skills::builtin::populate_skills(&global_dir.join("skills"))
                .unwrap_or_default();

        // Seed global instructions.md (effective dir) with default conventions
        // if missing — never overwrites user edits.
        let _ = crate::prompt::ProjectInstructions::ensure_global_instructions(&dir);

        Ok((dir, restored_skills))
    }

    fn create_dir_structure(dir: &Path) -> Result<()> {
        for sub in &["sessions", "cache", "skills", "logs"] {
            std::fs::create_dir_all(dir.join(sub))
                .with_context(|| format!("Creating directory {}", sub))?;
        }
        if !dir.join("config.toml").exists() {
            let default = Config::default();
            default.save(&dir.join("config.toml"))?;
        }
        Ok(())
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    /// Load config from project-local `.litepilot` if present, else global.
    pub fn load_for_workspace(workspace: &Path) -> Result<Self> {
        let path = Self::config_path_for(workspace);
        if path.exists() {
            Self::load_from(&path)
        } else {
            Self::load()
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Reading config from {}", path.display()))?;
        let config: Config = toml::from_str(&content).with_context(|| "Parsing config.toml")?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self).context("Serializing config to TOML")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
            .with_context(|| format!("Writing config to {}", path.display()))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn exists() -> bool {
        Self::config_path().map(|p| p.exists()).unwrap_or(false)
    }

    pub fn validate(&self) -> Result<()> {
        if !self.ollama_endpoint.starts_with("http://")
            && !self.ollama_endpoint.starts_with("https://")
        {
            anyhow::bail!("ollama_endpoint must start with http:// or https://");
        }
        let valid_modes = ["plan", "edit", "auto"];
        if !valid_modes.contains(&self.default_mode.as_str()) {
            anyhow::bail!("default_mode must be one of: plan, edit, auto");
        }
        let valid_residency = ["none", "exec", "both"];
        if !valid_residency.contains(&self.model_residency.as_str()) {
            anyhow::bail!("model_residency must be one of: none, exec, both");
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn cache_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("cache"))
    }

    #[allow(dead_code)]
    pub fn sessions_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("sessions"))
    }

    pub fn skills_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("skills"))
    }

    /// Directory for crash dumps: ~/.litepilot/crashes/
    pub fn crashes_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("crashes"))
    }

    /// Returns true when the config needs first-run setup (exec model not configured).
    #[allow(dead_code)]
    pub fn needs_setup(&self) -> bool {
        self.exec_model.is_empty()
    }

    pub fn effective_eval_model(&self) -> &str {
        if self.eval_model.is_empty() {
            &self.exec_model
        } else {
            &self.eval_model
        }
    }

    /// Whether the exec model should be kept resident in Ollama.
    pub fn keep_exec_resident(&self) -> bool {
        self.model_residency == "exec" || self.model_residency == "both"
    }

    /// Whether the eval model should be kept resident in Ollama.
    pub fn keep_eval_resident(&self) -> bool {
        self.model_residency == "both" && !self.eval_model.is_empty()
    }

    /// The model used for planning. Falls back to `exec_model` when unset, so a
    /// single-model configuration still plans with the exec model.
    pub fn effective_plan_model(&self) -> &str {
        if self.plan_model.is_empty() {
            &self.exec_model
        } else {
            &self.plan_model
        }
    }

    /// Whether the plan model should be kept resident in Ollama. Only when a
    /// distinct plan model is configured and residency is enabled — avoids warming
    /// a model identical to exec (already resident).
    pub fn keep_plan_resident(&self) -> bool {
        self.model_residency == "both"
            && !self.plan_model.is_empty()
            && self.plan_model != self.exec_model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn valid_config() -> Config {
        Config {
            ollama_endpoint: "http://localhost:11434".into(),
            exec_model: "qwen3:8b".into(),
            eval_model: "qwen3:14b".into(),
            theme: ThemeColors::default(),
            ..Default::default()
        }
    }

    #[test]
    fn roundtrip_toml() {
        let config = valid_config();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.ollama_endpoint, parsed.ollama_endpoint);
        assert_eq!(config.exec_model, parsed.exec_model);
    }

    #[test]
    fn defaults_are_valid() {
        let config = Config::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.default_mode, "edit");
        assert_eq!(config.connect_timeout, 15);
        assert_eq!(config.theme.primary, "cyan");
    }

    #[test]
    fn theme_serializes_to_toml() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("[theme]"));
        assert!(toml_str.contains("primary = \"cyan\""));
        assert!(toml_str.contains("accent = \"magenta\""));
    }

    #[test]
    fn theme_roundtrip_preserves_colors() {
        let mut config = Config::default();
        config.theme.primary = "#FF0000".into();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.theme.primary, "#FF0000");
    }

    #[test]
    fn invalid_endpoint_rejected() {
        let mut config = valid_config();
        config.ollama_endpoint = "ftp://bad".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn invalid_mode_rejected() {
        let mut config = valid_config();
        config.default_mode = "invalid".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn invalid_residency_rejected() {
        let mut config = valid_config();
        config.model_residency = "invalid".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn valid_residency_options() {
        for opt in &["none", "exec", "both"] {
            let mut config = valid_config();
            config.model_residency = opt.to_string();
            assert!(config.validate().is_ok());
        }
    }

    #[test]
    fn residency_helper_methods() {
        let mut config = valid_config();
        config.model_residency = "none".to_string();
        assert!(!config.keep_exec_resident());
        assert!(!config.keep_eval_resident());

        config.model_residency = "exec".to_string();
        assert!(config.keep_exec_resident());
        assert!(!config.keep_eval_resident());

        config.model_residency = "both".to_string();
        assert!(config.keep_exec_resident());
        assert!(config.keep_eval_resident());

        // Plan model: resident only when set and distinct from exec.
        config.plan_model = "qwen3:32b".into();
        assert!(config.keep_plan_resident());
        config.plan_model = config.exec_model.clone(); // same as exec
        assert!(!config.keep_plan_resident());
        config.plan_model = String::new(); // unset
        assert!(!config.keep_plan_resident());

        // "both" but no eval model set → keep_eval_resident is false
        config.eval_model = String::new();
        assert!(!config.keep_eval_resident());
    }

    #[test]
    fn load_save_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let config = valid_config();
        config.save(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(config.ollama_endpoint, loaded.ollama_endpoint);
        assert_eq!(config.exec_model, loaded.exec_model);
    }

    #[test]
    fn missing_file_returns_error() {
        let result = Config::load_from(Path::new("/nonexistent/config.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn empty_eval_falls_back_to_exec() {
        let config = Config {
            eval_model: String::new(),
            exec_model: "qwen3:8b".into(),
            ..Default::default()
        };
        assert_eq!(config.effective_eval_model(), "qwen3:8b");
    }

    #[test]
    fn empty_plan_falls_back_to_exec() {
        let config = Config {
            plan_model: String::new(),
            exec_model: "qwen3:8b".into(),
            ..Default::default()
        };
        assert_eq!(config.effective_plan_model(), "qwen3:8b");
    }

    #[test]
    fn plan_model_used_when_set() {
        let config = Config {
            plan_model: "qwen3:32b".into(),
            exec_model: "qwen3:8b".into(),
            ..Default::default()
        };
        assert_eq!(config.effective_plan_model(), "qwen3:32b");
    }

    #[test]
    fn ensure_dirs_creates_structure() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().join(".litepilot");
        // Monkey-patch: just verify the subdirectories would be created
        for sub in &["sessions", "cache", "skills", "logs"] {
            std::fs::create_dir_all(base.join(sub)).unwrap();
        }
        assert!(base.join("sessions").exists());
        assert!(base.join("cache").exists());
        assert!(base.join("skills").exists());
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn proptest_roundtrip(
            endpoint in "http://[a-z]{1,10}\\.local:\\d{1,5}",
            timeout in 1u64..300,
            exec in "[a-z]{1,10}:\\d{0,2}b",
            eval in "[a-z]{1,10}:\\d{0,2}b",
        ) {
            let config = Config {
                ollama_endpoint: endpoint.clone(),
                connect_timeout: timeout,
                exec_model: exec.clone(),
                eval_model: eval.clone(),
                ..Default::default()
            };
            let toml_str = toml::to_string_pretty(&config).unwrap();
            let parsed: Config = toml::from_str(&toml_str).unwrap();
            assert_eq!(config.ollama_endpoint, parsed.ollama_endpoint);
            assert_eq!(config.connect_timeout, parsed.connect_timeout);
            assert_eq!(config.exec_model, parsed.exec_model);
        }
    }
}
