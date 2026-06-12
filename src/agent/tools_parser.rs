use serde::{Deserialize, Serialize};

/// A tool call parsed from an LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub call_id: String,
    pub parameters: serde_json::Value,
}

/// Diagnostic info about why tool call parsing failed.
#[derive(Debug, Clone)]
pub struct ParseDiagnostics {
    /// Tags found in text that suggest a tool call was attempted.
    pub hints_found: Vec<String>,
    /// Description of what was tried and why it failed.
    pub failure_reasons: Vec<String>,
}

impl ParseDiagnostics {
    pub fn has_hints(&self) -> bool {
        !self.hints_found.is_empty()
    }

    /// Human-readable summary for injecting into correction prompts.
    pub fn format_for_correction(&self) -> String {
        let mut parts = Vec::new();
        if !self.hints_found.is_empty() {
            parts.push(format!(
                "Detected tool references: {}",
                self.hints_found.join(", ")
            ));
        }
        if !self.failure_reasons.is_empty() {
            parts.push(format!(
                "Parse failures:\n{}",
                self.failure_reasons
                    .iter()
                    .enumerate()
                    .map(|(i, r)| format!("  {}. {}", i + 1, r))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        parts.join("\n")
    }
}

/// Known tool names used to detect failed tool call attempts.
const KNOWN_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "exec_shell",
    "web_search",
    "web_reader",
    "search_files",
    "run_command",
];

/// Result of parsing tool calls from LLM response.
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub calls: Vec<ToolCall>,
    pub diagnostics: ParseDiagnostics,
}

impl ParseResult {
    /// Convenience: true if any valid tool calls were parsed.
    #[allow(dead_code)]
    pub fn has_calls(&self) -> bool {
        !self.calls.is_empty()
    }

    /// Convenience: true if the text looks like a tool attempt but parsing failed.
    pub fn is_failed_attempt(&self) -> bool {
        self.calls.is_empty() && self.diagnostics.has_hints()
    }
}

/// Parse tool calls from LLM response text with diagnostic info.
///
/// Supports two formats:
/// 1. JSON: `<tool_call name="..." call_id="...">{"key": "value"}</tool_call`
/// 2. Text: `Call: tool_name(key="value")`
pub fn parse_tool_calls_with_diagnostics(text: &str) -> ParseResult {
    let mut hints = Vec::new();
    let mut reasons = Vec::new();
    let lower = text.to_lowercase();

    // Detect tool name references in text
    for tool in KNOWN_TOOLS {
        if lower.contains(tool) {
            hints.push(tool.to_string());
        }
    }

    // Try JSON-format tool calls first
    let json_calls = parse_json_calls_inner(text, &mut reasons);

    let calls = if !json_calls.is_empty() {
        json_calls
    } else {
        // If JSON found tags but no valid calls, record why
        if text.contains("<tool_call") {
            reasons.push(
                "Found <tool_call tag but could not parse the full structure. Check: closing tag, name attribute, valid JSON params.".into()
            );
        }
        // Fallback to text format
        let text_calls = parse_text_calls(text);
        if text_calls.is_empty()
            && hints.iter().any(|h| text.contains(h))
            && !text.contains("<tool_call")
            && !text.contains("Call:")
        {
            reasons.push(
                "Tool names found but no <tool_call...> tags or 'Call:' lines. Use one of the supported formats.".into()
            );
        }
        text_calls
    };

    ParseResult {
        calls,
        diagnostics: ParseDiagnostics {
            hints_found: hints,
            failure_reasons: reasons,
        },
    }
}

/// Parse tool calls from LLM response text (simple API, no diagnostics).
#[allow(dead_code)]
pub fn parse_tool_calls(text: &str) -> Vec<ToolCall> {
    parse_tool_calls_with_diagnostics(text).calls
}

/// Parse JSON-formatted tool calls, recording failures.
fn parse_json_calls_inner(text: &str, reasons: &mut Vec<String>) -> Vec<ToolCall> {
    let re = regex::Regex::new(
        r#"<tool_call\s+name="(?P<name>[^"]+)"(?:\s+call_id="(?P<id>[^"]+)")?\s*>(?P<params>\{[^<]*\})\s*</tool_call"#,
    )
    .ok();

    let re = match re {
        Some(r) => r,
        None => return Vec::new(),
    };

    let mut calls = Vec::new();
    for cap in re.captures_iter(text) {
        let name = cap
            .name("name")
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let call_id = cap
            .name("id")
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(uuid_short);
        let params_str = cap.name("params").map(|m| m.as_str()).unwrap_or("{}");
        let parameters = match serde_json::from_str::<serde_json::Value>(params_str) {
            Ok(v) if v.is_object() => v,
            Ok(_) => {
                reasons.push(format!(
                    "Parameters for '{}' are not a JSON object: {}",
                    name, params_str
                ));
                serde_json::json!({})
            }
            Err(e) => {
                reasons.push(format!(
                    "Invalid JSON in parameters for '{}': {} — raw: {}",
                    name, e, params_str
                ));
                serde_json::json!({})
            }
        };

        if !name.is_empty() {
            calls.push(ToolCall {
                name,
                call_id,
                parameters,
            });
        }
    }

    calls
}

/// Parse text-formatted tool calls: Call: tool_name(key="value", key2="value2")
fn parse_text_calls(text: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("Call:") {
            continue;
        }
        if let Some(call) = parse_single_text_call(trimmed) {
            calls.push(call);
        }
    }
    calls
}

fn parse_single_text_call(line: &str) -> Option<ToolCall> {
    let rest = line.strip_prefix("Call:")?.trim();
    let paren_idx = rest.find('(')?;
    let name = rest[..paren_idx].trim().to_string();
    let params_str = rest[paren_idx + 1..].trim_end_matches(')');

    let mut params = serde_json::Map::new();
    for pair in params_str.split(',') {
        let pair = pair.trim();
        if let Some(eq_idx) = pair.find('=') {
            let key = pair[..eq_idx].trim();
            let value = pair[eq_idx + 1..]
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            params.insert(key.to_string(), serde_json::json!(value));
        }
    }

    Some(ToolCall {
        name,
        call_id: uuid_short(),
        parameters: serde_json::Value::Object(params),
    })
}

fn uuid_short() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

/// Sanitize LLM text output by redacting incomplete or invalid tool call markers.
/// Valid, complete tool calls are preserved. Only applies to text shown to the user
/// — the parser still sees the raw input for tool dispatch.
pub fn sanitize_output(text: &str) -> String {
    // Redact incomplete <tool_call ...> tags (no closing </tool_call).
    // We can't use lookahead in Rust's regex crate, so instead match the opening
    // tag and then verify the absence of a closing tag manually.
    let mut result = text.to_string();
    let opening_re = regex::Regex::new(
        r#"<tool_call\s+name="[^"]*"(?:\s+call_id="[^"]*")?\s*>"#
    ).unwrap();
    for mat in opening_re.find_iter(text).collect::<Vec<_>>().into_iter().rev() {
        let after = &text[mat.end()..];
        // If there's no closing tag after this opening tag, it's incomplete
        if !after.contains("</tool_call") {
            result.replace_range(mat.start()..text.len(), "[invalid tool call]");
            break; // only need to handle the last (trailing) incomplete one
        }
    }

    // Redact lone opening <tool_call tags without any attributes
    let lone_tag_re = regex::Regex::new(r#"<tool_call[^>]*>"#).unwrap();
    // Only strip if it wasn't already replaced above
    result = lone_tag_re
        .replace_all(&result, |caps: &regex::Captures| {
            let tag = caps.get(0).unwrap().as_str();
            // If it's a complete opening tag for a valid tool call (will have closing tag), keep it
            if tag.contains("name=\"") {
                tag.to_string()
            } else {
                "[invalid tool call]".to_string()
            }
        })
        .to_string();

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_tool_call() {
        let text = r#"I'll read the file.
<tool_call name="read_file" call_id="abc123">{"path": "src/main.rs"}</tool_call"#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].call_id, "abc123");
        assert_eq!(calls[0].parameters["path"], "src/main.rs");
    }

    #[test]
    fn parse_json_without_call_id() {
        let text = r#"<tool_call name="list_dir">{}</tool_call"#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[0].call_id.len(), 8); // auto-generated
    }

    #[test]
    fn parse_text_tool_call() {
        let text = "Call: read_file(path=\"src/main.rs\")";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].parameters["path"], "src/main.rs");
    }

    #[test]
    fn parse_multiple_text_calls() {
        let text = "Call: read_file(path=\"main.rs\")\nSome text\nCall: list_dir(path=\"src\")";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[1].name, "list_dir");
    }

    #[test]
    fn no_tool_calls_in_plain_text() {
        let text = "This is just a regular response with no tool calls.";
        let calls = parse_tool_calls(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn json_priority_over_text() {
        let text = r#"<tool_call name="read_file">{"path": "a.rs"}</tool_call
Call: write_file(path="b.rs")"#;
        let calls = parse_tool_calls(text);
        // JSON parsed first, text fallback skipped
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
    }

    #[test]
    fn diagnostics_detect_failed_attempt() {
        let text = "I should use read_file to check main.rs";
        let result = parse_tool_calls_with_diagnostics(text);
        assert!(result.calls.is_empty());
        assert!(result.is_failed_attempt());
        assert!(result
            .diagnostics
            .hints_found
            .contains(&"read_file".to_string()));
    }

    #[test]
    fn diagnostics_no_hints_in_plain_text() {
        let text = "Here is a simple response with no tools.";
        let result = parse_tool_calls_with_diagnostics(text);
        assert!(result.calls.is_empty());
        assert!(!result.is_failed_attempt());
    }

    #[test]
    fn diagnostics_malformed_tag() {
        let text = r#"<tool_call name="read_file">path: src/main.rs</tool_call"#;
        let result = parse_tool_calls_with_diagnostics(text);
        assert!(result.is_failed_attempt());
        assert!(!result.diagnostics.failure_reasons.is_empty());
    }

    #[test]
    fn diagnostics_format_readable() {
        let text = "Let me use read_file and exec_shell here";
        let result = parse_tool_calls_with_diagnostics(text);
        let formatted = result.diagnostics.format_for_correction();
        assert!(formatted.contains("read_file"));
        assert!(formatted.contains("exec_shell"));
    }

    #[test]
    fn multiple_params_text_call() {
        let text = "Call: edit_file(path=\"main.rs\", old_text=\"fn old\", new_text=\"fn new\")";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].parameters["path"], "main.rs");
        assert_eq!(calls[0].parameters["old_text"], "fn old");
        assert_eq!(calls[0].parameters["new_text"], "fn new");
    }
}
