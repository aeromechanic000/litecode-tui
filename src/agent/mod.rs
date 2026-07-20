pub mod diagnostics;
pub mod prompts;
pub mod retry;
pub mod summarizer;
pub mod syntax;
pub mod tools_parser;

use std::path::PathBuf;

/// A file change parsed from an assistant response's `### FILE:` / `### ACTION:`
/// blocks. Produced by [`parse_file_changes`] and consumed by every apply path
/// (Auto, Edit, `/apply`, headless) to turn the model's output into writes.
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub content: String,
    pub action: String,
}

/// Parse `### FILE:` / `### ACTION:` / fenced-code blocks from an assistant
/// response into concrete file changes.
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let changes = parse_file_changes(&text);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, PathBuf::from("src/main.rs"));
        assert!(changes[0].content.contains("hello"));
        assert_eq!(changes[1].path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn parse_empty_changes() {
        let changes = parse_file_changes("");
        assert!(changes.is_empty());
    }

    #[test]
    fn parse_file_changes_handles_delete_action() {
        let text = "### FILE: old.rs\n### ACTION: delete\n```\n```\n";
        let changes = parse_file_changes(text);
        // Empty content body → not emitted (content must be non-empty).
        assert!(changes.is_empty());
    }
}
