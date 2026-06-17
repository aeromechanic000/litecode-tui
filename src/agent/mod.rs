pub mod auto_run;
pub mod diagnostics;
pub mod editor;
pub mod planner;
pub mod prompts;
pub mod retry;
pub mod summarizer;
pub mod syntax;
pub mod tools_parser;

use crate::codebase::CodeBase;
use crate::config::Config;
use crate::ollama::chat::ChatMessage;
use crate::ollama::OllamaClient;
use crate::sandbox::Sandbox;
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Plan {
    pub strategic_goal: String,
    pub analysis: String,
    pub steps: Vec<String>,
    pub files: Vec<String>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileChange {
    pub path: PathBuf,
    pub content: String,
    pub action: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuditResult {
    pub passed: bool,
    pub issues: Vec<String>,
    pub fixes: Vec<FileChange>,
}

#[allow(dead_code)]
pub struct AgentPipeline<'a> {
    client: &'a OllamaClient,
    config: &'a Config,
    sandbox: &'a Sandbox,
    workspace: PathBuf,
}

#[allow(dead_code)]
impl<'a> AgentPipeline<'a> {
    pub fn new(
        client: &'a OllamaClient,
        config: &'a Config,
        sandbox: &'a Sandbox,
        workspace: PathBuf,
    ) -> Self {
        Self {
            client,
            config,
            sandbox,
            workspace,
        }
    }

    pub async fn plan(&self, user_request: &str, context: &str) -> Result<Plan> {
        let messages = vec![
            ChatMessage::system(prompts::PLANNING_SYSTEM),
            ChatMessage::user(format!(
                "Project context:\n{}\n\nUser request:\n{}\n\nOutput a structured plan with steps, files to create/modify, and dependencies.",
                context, user_request
            )),
        ];
        let model = &self.config.exec_model;
        let response = self.client.chat(model, messages, true).await?;
        Ok(Self::parse_plan(&response.content))
    }

    pub async fn plan_with_templates(
        &self,
        user_request: &str,
        context: &str,
        codebase: &CodeBase,
    ) -> Result<Plan> {
        let loaded = crate::codebase::retrieval::retrieve(
            self.client,
            self.config,
            codebase,
            user_request,
            context,
        )
        .await;

        let code_base_section = if loaded.refs.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nReference code from built-in library:\n{}",
                loaded.refs.join("\n")
            )
        };

        let messages = vec![
            ChatMessage::system(prompts::PLANNING_SYSTEM),
            ChatMessage::user(format!(
                "Project context:\n{}{}\n\nUser request:\n{}\n\nOutput a structured plan with steps, files to create/modify, and dependencies.",
                context, code_base_section, user_request
            )),
        ];
        let model = &self.config.exec_model;
        let response = self.client.chat(model, messages, true).await?;
        Ok(Self::parse_plan(&response.content))
    }

    pub async fn implement(
        &self,
        plan: &Plan,
        context: &str,
        template_refs: &[String],
    ) -> Result<Vec<FileChange>> {
        let code_base_section = if template_refs.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nReference code from built-in library:\n{}",
                template_refs.join("\n")
            )
        };

        let messages = vec![
            ChatMessage::system(prompts::CODING_SYSTEM),
            ChatMessage::user(format!(
                "Project context:\n{}{}\n\n[STRATEGIC GOAL: {}]\n\nPlan:\n{}\n\nImplement all changes. For each file, output:\n### FILE: path/to/file\n### ACTION: create|modify|delete\n```\nfile content\n```\n",
                context, code_base_section,
                plan.strategic_goal,
                plan.steps.join("\n")
            )),
        ];
        let model = &self.config.exec_model;
        let response = self.client.chat(model, messages, true).await?;
        Ok(Self::parse_file_changes(&response.content))
    }

    pub async fn audit(&self, changes: &[FileChange], context: &str) -> Result<AuditResult> {
        let changes_str: Vec<String> = changes
            .iter()
            .map(|c| format!("--- {} ({}) ---\n{}", c.path.display(), c.action, c.content))
            .collect();
        let messages = vec![
            ChatMessage::system(prompts::AUDIT_SYSTEM),
            ChatMessage::user(format!(
                "Project context:\n{}\n\nChanges to review:\n{}\n\nCheck for bugs, logic errors, and consistency issues. Output PASS or FAIL followed by any issues.",
                context,
                changes_str.join("\n\n")
            )),
        ];
        let model = self.config.effective_eval_model();
        let response = self.client.chat(model, messages, true).await?;
        Ok(Self::parse_audit(&response.content))
    }

    fn parse_plan(text: &str) -> Plan {
        // Extract strategic goal from first non-empty line
        let strategic_goal = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();

        Plan {
            strategic_goal,
            analysis: text.to_string(),
            steps: text
                .lines()
                .filter(|l| {
                    l.trim().starts_with("- ")
                        || l.trim().starts_with("* ")
                        || l.trim().chars().next().is_some_and(|c| c.is_ascii_digit())
                })
                .map(|l| l.trim().to_string())
                .collect(),
            files: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    pub fn parse_file_changes(text: &str) -> Vec<FileChange> {
        let mut changes = Vec::new();
        let mut current_path = String::new();
        let mut current_action = String::new();
        let mut current_content = String::new();
        let mut in_code_block = false;

        for line in text.lines() {
            if line.starts_with("### FILE:") {
                if !current_path.is_empty() && !current_content.is_empty() {
                    changes.push(FileChange {
                        path: PathBuf::from(&current_path),
                        content: current_content.trim_end().to_string(),
                        action: current_action.clone(),
                    });
                }
                current_path = line.trim_start_matches("### FILE:").trim().to_string();
                current_content.clear();
                in_code_block = false;
            } else if line.starts_with("### ACTION:") {
                current_action = line.trim_start_matches("### ACTION:").trim().to_string();
            } else if line.trim() == "```" {
                in_code_block = !in_code_block;
            } else if in_code_block {
                current_content.push_str(line);
                current_content.push('\n');
            }
        }

        if !current_path.is_empty() && !current_content.is_empty() {
            changes.push(FileChange {
                path: PathBuf::from(&current_path),
                content: current_content.trim_end().to_string(),
                action: current_action,
            });
        }

        changes
    }

    fn parse_audit(text: &str) -> AuditResult {
        let upper = text.to_uppercase();
        let passed = upper.contains("PASS") && !upper.contains("FAIL");
        let issues: Vec<String> = text
            .lines()
            .filter(|l| {
                let lower = l.to_lowercase();
                lower.contains("error")
                    || lower.contains("bug")
                    || lower.contains("issue")
                    || lower.contains("problem")
            })
            .map(|l| l.to_string())
            .collect();
        AuditResult {
            passed,
            issues,
            fixes: Vec::new(),
        }
    }
}

/// Detect if a response has drifted from the strategic goal.
/// Uses keyword overlap heuristic: if the response shares fewer than
/// MIN_OVERLAP keywords with the goal, it's likely drifted.
#[allow(dead_code)]
pub fn detect_drift(goal: &str, response: &str) -> bool {
    const MIN_OVERLAP: usize = 2;
    let stop_words: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through",
        "during", "before", "after", "above", "below", "between", "out", "off", "over", "under",
        "again", "further", "then", "once", "and", "but", "or", "nor", "not", "so", "yet", "both",
        "either", "neither", "each", "every", "all", "any", "few", "more", "most", "other", "some",
        "such", "no", "only", "own", "same", "than", "too", "very", "just", "because", "if",
        "when", "where", "how", "what", "which", "who", "this", "that", "these", "those", "i",
        "me", "my", "we", "our", "you", "your", "he", "him", "his", "she", "her", "it", "its",
        "they", "them", "their",
    ];

    let goal_lower = goal.to_lowercase();
    let goal_words: Vec<&str> = goal_lower
        .split_whitespace()
        .filter(|w| w.len() > 2 && !stop_words.contains(w))
        .collect();

    if goal_words.len() < 2 {
        return false; // Goal too short to meaningfully check
    }

    let resp_lower = response.to_lowercase();
    let overlap = goal_words
        .iter()
        .filter(|w| resp_lower.contains(*w))
        .count();

    overlap < MIN_OVERLAP
}

/// Build a drift warning message to inject when drift is detected.
#[allow(dead_code)]
pub fn drift_warning(goal: &str) -> String {
    format!(
        "WARNING: You appear to have drifted from the objective. Refocus on: {}\n\
         Continue working toward this goal. Do not introduce unrelated changes.",
        goal
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_extracts_steps() {
        let text =
            "# Plan\n\nSteps:\n- Create main.rs\n- Add config module\n- Write tests\n1. First do X";
        let plan = AgentPipeline::parse_plan(&text);
        assert!(!plan.steps.is_empty());
        assert!(plan.analysis.contains("Plan"));
    }

    #[test]
    fn parse_file_changes_extracts_blocks() {
        let text = r#"### FILE: src/main.rs
### ACTION: create
```
fn main() {
    println!("hello");
}
```

### FILE: src/lib.rs
### ACTION: create
```
pub fn add(a: i32, b: i32) -> i32 { a + b }
```
"#;
        let changes = AgentPipeline::parse_file_changes(&text);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, PathBuf::from("src/main.rs"));
        assert!(changes[0].content.contains("hello"));
        assert_eq!(changes[1].path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn parse_audit_pass() {
        let text = "Review: PASS\nCode looks good, clean implementation.";
        let result = AgentPipeline::parse_audit(text);
        assert!(result.passed);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn parse_audit_fail() {
        let text = "Review: FAIL\nIssue: Missing error handling on line 42\nBug: potential null dereference";
        let result = AgentPipeline::parse_audit(text);
        assert!(!result.passed);
        assert!(!result.issues.is_empty());
    }

    #[test]
    fn parse_empty_changes() {
        let changes = AgentPipeline::parse_file_changes("");
        assert!(changes.is_empty());
    }

    #[test]
    fn plan_extracts_strategic_goal() {
        let text = "Build a REST API with authentication\n- Create routes\n- Add middleware";
        let plan = AgentPipeline::parse_plan(text);
        assert_eq!(plan.strategic_goal, "Build a REST API with authentication");
    }

    #[test]
    fn detect_drift_when_unrelated() {
        let goal = "Implement user authentication with JWT tokens";
        let response = "I noticed the CSS colors are wrong. Let me fix the background gradient.";
        assert!(detect_drift(goal, response));
    }

    #[test]
    fn no_drift_when_on_topic() {
        let goal = "Implement user authentication with JWT tokens";
        let response = "I'll create the JWT token validation and user authentication middleware.";
        assert!(!detect_drift(goal, response));
    }

    #[test]
    fn drift_warning_contains_goal() {
        let warning = drift_warning("Fix the login bug");
        assert!(warning.contains("Fix the login bug"));
        assert!(warning.contains("WARNING"));
    }
}
