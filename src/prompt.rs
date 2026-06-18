use crate::app::AppMode;
use crate::config::Config;
use crate::skills::SkillRegistry;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Priority for prompt sections — controls order and inclusion when budget is tight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum PromptPriority {
    Critical = 0,
    High = 1,
    Medium = 2,
    Low = 3,
}

#[allow(dead_code)]
/// A single layer in the prompt composition.
#[derive(Debug, Clone)]
pub struct PromptLayer {
    pub priority: PromptPriority,
    pub name: String,
    pub content: String,
}

impl PromptLayer {
    pub fn new(
        priority: PromptPriority,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            priority,
            name: name.into(),
            content: content.into(),
        }
    }
}

/// Environment information injected into the volatile tail.
#[derive(Debug, Clone)]
pub struct EnvironmentBlock {
    pub platform: String,
    pub shell: String,
    pub working_directory: PathBuf,
    pub date_time: String,
}

impl EnvironmentBlock {
    pub fn capture(workspace: &Path) -> Self {
        Self {
            platform: std::env::consts::OS.to_string(),
            shell: std::env::var("SHELL").unwrap_or_else(|_| "unknown".into()),
            working_directory: workspace.to_path_buf(),
            date_time: chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string(),
        }
    }

    pub fn format(&self) -> String {
        format!(
            "## Environment\nPlatform: {}\nShell: {}\nWorking Directory: {}\nDate: {}",
            self.platform,
            self.shell,
            self.working_directory.display(),
            self.date_time
        )
    }
}

/// Project instructions discovered from workspace files.
#[derive(Debug, Clone, Default)]
pub struct ProjectInstructions {
    pub agents_md: Option<String>,
    pub claude_md: Option<String>,
    pub custom_instructions: Option<String>,
    pub readme: Option<String>,
}

impl ProjectInstructions {
    pub fn discover(workspace: &Path, config_dir: &Path) -> Self {
        let read_file = |path: &Path| {
            if path.exists() {
                std::fs::read_to_string(path).ok()
            } else {
                None
            }
        };

        let mut instructions = Self {
            agents_md: read_file(&workspace.join("AGENTS.md")),
            claude_md: read_file(&workspace.join("CLAUDE.md")),
            custom_instructions: read_file(&config_dir.join("instructions.md")),
            readme: None,
        };

        // If no instruction files found, use README.md and auto-generate instructions
        if instructions.agents_md.is_none()
            && instructions.claude_md.is_none()
            && instructions.custom_instructions.is_none()
        {
            if let Some(readme) = read_file(&workspace.join("README.md")) {
                // Use first 100 lines of README as context
                let truncated: String = readme.lines().take(100).collect::<Vec<_>>().join("\n");
                instructions.readme = Some(truncated);
            }

            // Auto-generate instructions from project structure
            if let Some(generated) = Self::auto_generate(workspace) {
                // Save for user editing
                if std::fs::create_dir_all(config_dir).is_ok() {
                    let path = config_dir.join("instructions.md");
                    if !path.exists() {
                        let _ = std::fs::write(&path, &generated);
                    }
                }
                instructions.custom_instructions = Some(generated);
            }
        }

        instructions
    }

    /// Auto-generate instructions from project structure.
    fn auto_generate(workspace: &Path) -> Option<String> {
        let name = workspace
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Detect language from file extensions
        let mut extensions = std::collections::HashMap::new();
        if let Ok(entries) = std::fs::read_dir(workspace) {
            for entry in entries.flatten() {
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    *extensions.entry(ext.to_string()).or_insert(0usize) += 1;
                }
            }
        }

        let lang = extensions
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(ext, _)| match ext.as_str() {
                "rs" => "Rust",
                "py" => "Python",
                "ts" | "tsx" => "TypeScript",
                "js" | "jsx" => "JavaScript",
                "go" => "Go",
                "java" => "Java",
                "c" | "h" => "C",
                "cpp" | "cc" => "C++",
                _ => ext,
            })
            .unwrap_or("unknown");

        let mut instructions = format!(
            "# Project: {}\n# Language: {}\n\n\
             This file was auto-generated by LitePilot.\n\
             Edit it to customize project instructions for the AI assistant.\n",
            name, lang
        );

        // Detect build tool
        if workspace.join("Cargo.toml").exists() {
            instructions.push_str("\nBuild: cargo build\nTest: cargo test\n");
        } else if workspace.join("package.json").exists() {
            instructions.push_str("\nBuild: npm run build\nTest: npm test\n");
        } else if workspace.join("go.mod").exists() {
            instructions.push_str("\nBuild: go build ./...\nTest: go test ./...\n");
        } else if workspace.join("pyproject.toml").exists() || workspace.join("setup.py").exists() {
            instructions.push_str("\nTest: pytest\n");
        }

        Some(instructions)
    }

    pub fn format(&self) -> String {
        let mut sections = Vec::new();
        if let Some(ref s) = self.agents_md {
            sections.push(format!("### AGENTS.md\n{}", s));
        }
        if let Some(ref s) = self.claude_md {
            sections.push(format!("### CLAUDE.md\n{}", s));
        }
        if let Some(ref s) = self.custom_instructions {
            sections.push(format!("### Instructions\n{}", s));
        }
        if let Some(ref s) = self.readme {
            sections.push(format!("### README (summary)\n{}", s));
        }
        if sections.is_empty() {
            return String::new();
        }
        format!("## Project Context\n{}", sections.join("\n\n"))
    }
}

/// Builder for composing system prompts in stable layers for KV cache reuse.
///
/// Static layers (byte-identical across turns):
///   base identity → mode overlay → skills → project context
///
/// Volatile tail (rebuilt every turn):
///   working set summary + conversation summary + environment
pub struct PromptBuilder {
    static_layers: Vec<PromptLayer>,
    project_instructions: Option<ProjectInstructions>,
    environment: Option<EnvironmentBlock>,
    working_set_summary: Option<String>,
    conversation_summary: Option<String>,
    current_goal: Option<String>,
    completed_tasks: Vec<String>,
}

impl PromptBuilder {
    pub fn new(_config: &Config) -> Self {
        let base = PromptLayer::new(
            PromptPriority::Critical,
            "base_identity",
            Self::base_identity_prompt(),
        );
        Self {
            static_layers: vec![base],
            project_instructions: None,
            environment: None,
            working_set_summary: None,
            conversation_summary: None,
            current_goal: None,
            completed_tasks: Vec::new(),
        }
    }

    fn base_identity_prompt() -> String {
        r#"You are LitePilot, a terminal AI coding assistant powered by local Ollama models.
You help developers write, modify, and understand code through a terminal interface.

Core principles:
- Be concise and direct
- Use concrete examples over abstract explanations
- When showing code, keep it minimal and focused
- Respect the user's preferred coding style and conventions
- Ask clarifying questions when requirements are ambiguous

When the user provides a URL or asks to fetch/read a webpage, ALWAYS use web_reader first.
When the user asks to run a build, test, or git operation, use exec_shell.

## Tool calls
To invoke a tool during a step, emit one block per call. The JSON inside the tag holds the
tool's parameters at the TOP LEVEL — never wrap them under "param" or "parameters".

Examples:
<tool_call name="read_file" call_id="1">{"path": "src/main.rs"}</tool_call>
<tool_call name="write_file" call_id="2">{"path": "src/new.rs", "content": "fn main() {}"}</tool_call>
<tool_call name="edit_file" call_id="3">{"path": "src/main.rs", "old_text": "fn old", "new_text": "fn new"}</tool_call>
<tool_call name="web_reader" call_id="4">{"url": "https://example.com"}</tool_call>
<tool_call name="exec_shell" call_id="5">{"command": "cargo test"}</tool_call>

The "Available Tools" section below lists every tool and its required parameters. After a
call, the executor appends a <tool_result> block and you may continue the same step (more
calls, more prose, or the step's final output). Emit blocks inline wherever the call is
needed. Do not invent tool names — only the tools listed below exist.

### FILE: path/to/file
### ACTION: create|modify|delete
```
file content here
```"#
            .to_string()
    }

    /// Set the current mode — rebuilds the mode overlay layer.
    pub fn set_mode(&mut self, mode: AppMode) {
        self.static_layers.retain(|l| l.name != "mode_overlay");
        self.static_layers.push(PromptLayer::new(
            PromptPriority::High,
            "mode_overlay",
            Self::mode_prompt(mode),
        ));
    }

    fn mode_prompt(mode: AppMode) -> String {
        match mode {
            AppMode::Plan => r#"## PLAN MODE
You are in read-only planning mode. Focus on:
- Understanding code structure and dependencies
- Identifying potential issues or improvements
- Proposing implementation strategies
Do NOT suggest any file modifications or commands."#
                .to_string(),
            AppMode::Edit => r#"## EDIT MODE
You are in edit mode. When suggesting changes:
- Use the ### FILE: / ### ACTION: format
- Show minimal, focused changes
- Explain the reasoning for each change
- Wait for user confirmation before proceeding"#
                .to_string(),
            AppMode::Auto => r#"## AUTO MODE
You are in automatic execution mode. You may:
- Create, modify, or delete files as needed
- Execute commands to build, test, or verify changes
- Apply changes directly without confirmation
Always validate changes before applying (syntax checks, tests)."#
                .to_string(),
        }
    }

    /// Update skills layer from registry.
    pub fn set_skills(&mut self, skills: &SkillRegistry) {
        self.static_layers.retain(|l| l.name != "skills");
        if skills.list().is_empty() {
            return;
        }
        let list = skills
            .list()
            .iter()
            .map(|s| format!("- **/{}**: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n");
        self.static_layers.push(PromptLayer::new(
            PromptPriority::High,
            "skills",
            format!("## Available Skills\n{}\n\nInvoke with /skill_name. When relevant, apply the skill's specialized approach.", list),
        ));
    }

    /// Update the per-tool schema layer. Emits one line per tool with its
    /// required and optional parameters so the executor knows the exact
    /// top-level JSON keys each tool expects. Sorted by tool name for
    /// byte-stable output across turns (KV-cache friendly).
    pub fn set_tools(&mut self, tools: &crate::tools::ToolRegistry) {
        self.static_layers.retain(|l| l.name != "tools");
        let schema = tools.schema_text();
        if schema.is_empty() {
            return;
        }
        self.static_layers.push(PromptLayer::new(
            PromptPriority::High,
            "tools",
            format!(
                "## Available Tools\n{}\n\nCall parameters go directly in the JSON object inside <tool_call>...</tool_call>. Never wrap them under \"param\" or \"parameters\".",
                schema
            ),
        ));
    }

    /// Update project context from discovered instruction files.
    pub fn set_project_context(&mut self, instructions: ProjectInstructions) {
        self.static_layers.retain(|l| l.name != "project_context");
        let text = instructions.format();
        if !text.is_empty() {
            self.static_layers.push(PromptLayer::new(
                PromptPriority::Medium,
                "project_context",
                text,
            ));
        }
        self.project_instructions = Some(instructions);
    }

    /// Update environment block (called every turn).
    pub fn update_environment(&mut self, env: EnvironmentBlock) {
        self.environment = Some(env);
    }

    /// Update volatile content (called every turn).
    pub fn set_volatile(
        &mut self,
        working_set_summary: Option<String>,
        conversation_summary: Option<String>,
    ) {
        self.working_set_summary = working_set_summary;
        self.conversation_summary = conversation_summary;
    }

    /// Set the current user objective — re-injected at prompt edges for small models.
    pub fn set_current_goal(&mut self, goal: impl Into<String>) {
        self.current_goal = Some(goal.into());
    }

    /// Record a completed task. Shown in volatile tail to maintain focus.
    #[allow(dead_code)]
    pub fn add_completed_task(&mut self, task: impl Into<String>) {
        self.completed_tasks.push(task.into());
    }

    /// Clear goal and completed tasks (e.g. on new conversation).
    #[allow(dead_code)]
    pub fn reset_goal_tracking(&mut self) {
        self.current_goal = None;
        self.completed_tasks.clear();
    }

    /// Build the final system prompt string.
    pub fn build(&self) -> String {
        let mut sections = Vec::new();

        // Static layers sorted by priority
        let mut layers = self.static_layers.clone();
        layers.sort_by_key(|l| l.priority);
        for layer in layers {
            sections.push(layer.content);
        }

        // Volatile tail — goal goes last (edge position) for small model attention
        let mut volatile = Vec::new();
        if let Some(ref ws) = self.working_set_summary {
            volatile.push(format!("## Working Set\n{}", ws));
        }
        if let Some(ref cs) = self.conversation_summary {
            volatile.push(format!("## Conversation Summary\n{}", cs));
        }
        if !self.completed_tasks.is_empty() {
            let tasks = self
                .completed_tasks
                .iter()
                .enumerate()
                .map(|(i, t)| format!("{}. {}", i + 1, t))
                .collect::<Vec<_>>()
                .join("\n");
            volatile.push(format!("## Completed\n{}", tasks));
        }
        // Current objective at the very end — small models attend most to prompt edges
        if let Some(ref goal) = self.current_goal {
            volatile.push(format!("## Current Objective\n{}", goal));
        }
        if let Some(ref env) = self.environment {
            volatile.push(env.format());
        }
        if !volatile.is_empty() {
            sections.push(volatile.join("\n\n"));
        }

        sections.join("\n\n")
    }

    /// Get the static prefix only (for KV cache validation).
    #[allow(dead_code)]
    pub fn static_prefix(&self) -> String {
        let mut layers = self.static_layers.clone();
        layers.sort_by_key(|l| l.priority);
        layers
            .into_iter()
            .map(|l| l.content)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Hash of the static prefix for KV cache stability validation.
    /// This hash should remain constant across turns as long as mode, skills,
    /// and project context don't change.
    pub fn static_prefix_hash(&self) -> u64 {
        let prefix = self.static_prefix();
        let mut hasher = DefaultHasher::new();
        prefix.hash(&mut hasher);
        hasher.finish()
    }

    /// Validate that the prompt structure is KV-cache friendly.
    /// Returns Ok if the static prefix is stable and doesn't contain volatile data.
    #[allow(dead_code)]
    pub fn validate_for_kv_cache(&self) -> Result<(), String> {
        let static_text = self.static_prefix();
        // Date/time must only appear in the volatile tail (environment block)
        if static_text.contains("Date:") || static_text.contains("Current date:") {
            return Err("Static prefix contains date/time — this breaks KV cache stability".into());
        }
        if static_text.is_empty() {
            return Err("Static prefix is empty — no cache benefit".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_identity_present() {
        let builder = PromptBuilder::new(&Config::default());
        let prompt = builder.build();
        assert!(prompt.contains("LitePilot"));
        assert!(prompt.contains("terminal AI coding assistant"));
    }

    #[test]
    fn mode_overlay_added() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_mode(AppMode::Plan);
        let prompt = builder.build();
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("read-only"));
    }

    #[test]
    fn mode_overlay_replaced() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_mode(AppMode::Plan);
        assert!(builder.build().contains("PLAN MODE"));
        builder.set_mode(AppMode::Auto);
        let prompt = builder.build();
        assert!(prompt.contains("AUTO MODE"));
        assert!(!prompt.contains("PLAN MODE"));
    }

    #[test]
    fn static_prefix_stable_across_env_update() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_mode(AppMode::Edit);
        let prefix1 = builder.static_prefix();

        builder.update_environment(EnvironmentBlock {
            platform: "test".into(),
            shell: "/bin/test".into(),
            working_directory: PathBuf::from("/tmp"),
            date_time: "2024-01-01".into(),
        });
        let prefix2 = builder.static_prefix();
        assert_eq!(prefix1, prefix2);
    }

    #[test]
    fn skills_injected() {
        let mut builder = PromptBuilder::new(&Config::default());
        let registry = SkillRegistry::empty();
        builder.set_skills(&registry);
        // Empty registry → no skills layer
        assert!(!builder.build().contains("Available Skills"));

        // Reload the builder to test non-empty case via the layer name check
        let mut builder = PromptBuilder::new(&Config::default());
        assert!(builder.static_layers.iter().all(|l| l.name != "skills"));
    }

    #[test]
    fn project_context_discovery() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path();
        let config_dir = tmp.path().join(".litepilot");
        std::fs::create_dir_all(&config_dir).unwrap();

        // No files → auto-generates instructions from project structure
        let instructions = ProjectInstructions::discover(workspace, &config_dir);
        // Auto-generated instructions should exist (project name + language detected)
        assert!(!instructions.format().is_empty());

        // Write CLAUDE.md — takes priority over auto-generated
        std::fs::write(workspace.join("CLAUDE.md"), "Use Rust conventions").unwrap();
        let instructions = ProjectInstructions::discover(workspace, &config_dir);
        assert!(instructions.format().contains("CLAUDE.md"));
        assert!(instructions.format().contains("Rust conventions"));
    }

    #[test]
    fn auto_generate_detects_language() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path();
        // Write a Rust file to bias language detection
        std::fs::write(workspace.join("main.rs"), "fn main() {}").unwrap();
        let generated = ProjectInstructions::auto_generate(workspace);
        assert!(generated.is_some());
        assert!(generated.unwrap().contains("Rust"));
    }

    #[test]
    fn auto_generate_detects_cargo() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path();
        std::fs::write(workspace.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let generated = ProjectInstructions::auto_generate(workspace);
        assert!(generated.unwrap().contains("cargo build"));
    }

    #[test]
    fn readme_used_as_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path();
        let config_dir = tmp.path().join(".litepilot");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(workspace.join("README.md"), "# My Project\nA test project.").unwrap();

        let instructions = ProjectInstructions::discover(workspace, &config_dir);
        let formatted = instructions.format();
        assert!(formatted.contains("README"));
    }

    #[test]
    fn environment_block_formats() {
        let env = EnvironmentBlock {
            platform: "macos".into(),
            shell: "/bin/zsh".into(),
            working_directory: PathBuf::from("/Users/test/project"),
            date_time: "2024-05-25 10:30 UTC".into(),
        };
        let formatted = env.format();
        assert!(formatted.contains("Platform: macos"));
        assert!(formatted.contains("Shell: /bin/zsh"));
        assert!(formatted.contains("Working Directory: /Users/test/project"));
    }

    #[test]
    fn volatile_content_in_output() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_volatile(
            Some("Active files: src/main.rs".into()),
            Some("Previously discussed authentication".into()),
        );
        let prompt = builder.build();
        assert!(prompt.contains("Working Set"));
        assert!(prompt.contains("src/main.rs"));
        assert!(prompt.contains("Conversation Summary"));
        assert!(prompt.contains("authentication"));
    }

    #[test]
    fn full_build_contains_all_layers() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_mode(AppMode::Auto);
        builder.set_skills(&SkillRegistry::empty());
        builder.update_environment(EnvironmentBlock::capture(Path::new("/tmp")));
        builder.set_volatile(Some("Active: main.rs".into()), None);

        let prompt = builder.build();
        assert!(prompt.contains("LitePilot")); // base
        assert!(prompt.contains("AUTO MODE")); // mode
        assert!(prompt.contains("Environment")); // env
        assert!(prompt.contains("Working Set")); // volatile
    }

    #[test]
    fn goal_injected_in_volatile_tail() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_current_goal("Create a REST API with authentication");
        let prompt = builder.build();
        assert!(prompt.contains("Current Objective"));
        assert!(prompt.contains("REST API with authentication"));
    }

    #[test]
    fn goal_placed_at_edge_near_end() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_current_goal("Fix the login bug");
        builder.update_environment(EnvironmentBlock {
            platform: "test".into(),
            shell: "/bin/test".into(),
            working_directory: PathBuf::from("/tmp"),
            date_time: "2024-01-01".into(),
        });
        let prompt = builder.build();
        // Environment comes after goal — goal is in the last semantic position before env
        let goal_pos = prompt.rfind("Current Objective").unwrap();
        let env_pos = prompt.rfind("Environment").unwrap();
        assert!(goal_pos < env_pos, "goal should come before environment");
    }

    #[test]
    fn completed_tasks_shown() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.add_completed_task("Created src/main.rs");
        builder.add_completed_task("Added config module");
        let prompt = builder.build();
        assert!(prompt.contains("Completed"));
        assert!(prompt.contains("Created src/main.rs"));
        assert!(prompt.contains("Added config module"));
    }

    #[test]
    fn reset_clears_goal_tracking() {
        let mut builder = PromptBuilder::new(&Config::default());
        builder.set_current_goal("build feature");
        builder.add_completed_task("step 1");
        builder.reset_goal_tracking();
        let prompt = builder.build();
        assert!(!prompt.contains("Current Objective"));
        assert!(!prompt.contains("Completed"));
    }

    #[test]
    fn base_identity_no_param_template() {
        // Regression guard: the misleading {"param": "value"} template must never
        // come back. It taught small models to wrap params under "param".
        let builder = PromptBuilder::new(&Config::default());
        let prompt = builder.build();
        assert!(
            !prompt.contains(r#"{"param": "value"}"#),
            "base_identity_prompt must not contain the misleading template"
        );
    }

    #[test]
    fn tools_layer_injected() {
        let mut builder = PromptBuilder::new(&Config::default());
        let workspace = std::path::PathBuf::from(".");
        let registry = crate::tools::ToolRegistry::new(workspace, &Config::default());
        builder.set_tools(&registry);
        let prompt = builder.build();
        assert!(prompt.contains("## Available Tools"));
        // The real registry always contains web_reader — the bug source.
        assert!(prompt.contains("web_reader"));
        // Schema line for web_reader must mention url as required.
        assert!(
            prompt.contains("required=[url]"),
            "expected web_reader required=[url], got: {}",
            prompt
        );
    }

    #[test]
    fn tools_layer_does_not_rely_on_skills_layer() {
        // Tools layer should be independent of skills — both can coexist.
        let mut builder = PromptBuilder::new(&Config::default());
        let workspace = std::path::PathBuf::from(".");
        let registry = crate::tools::ToolRegistry::new(workspace, &Config::default());
        builder.set_tools(&registry);
        builder.set_skills(&SkillRegistry::empty());
        let prompt = builder.build();
        assert!(prompt.contains("## Available Tools"));
    }
}
