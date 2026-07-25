pub mod file_ops;
pub mod search;
pub mod shell;
pub mod web_reader;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Definition of a tool for the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_name: String,
    pub call_id: String,
    pub success: bool,
    pub output: String,
}

impl ToolResult {
    pub fn ok(
        tool_name: impl Into<String>,
        call_id: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            call_id: call_id.into(),
            success: true,
            output: output.into(),
        }
    }

    pub fn err(
        tool_name: impl Into<String>,
        call_id: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            call_id: call_id.into(),
            success: false,
            output: error.into(),
        }
    }
}

/// Trait that all tools implement.
pub trait Tool: Send + Sync {
    fn execute(&self, params: serde_json::Value, call_id: String) -> Result<ToolResult>;
    fn definition(&self) -> ToolDef;
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new(workspace: std::path::PathBuf, config: &crate::config::Config) -> Self {
        let sandbox = std::sync::Arc::new(crate::sandbox::Sandbox::new(workspace));
        let mut reg = Self {
            tools: HashMap::new(),
        };
        reg.register(Box::new(file_ops::ReadFile::new(sandbox.clone())));
        reg.register(Box::new(file_ops::WriteFile::new(sandbox.clone())));
        reg.register(Box::new(file_ops::EditFile::new(sandbox.clone())));
        reg.register(Box::new(file_ops::ListDir::new(sandbox)));
        reg.register(Box::new(shell::ExecShell::new()));
        reg.register(Box::new(search::WebSearch::from_config(config)));
        reg.register(Box::new(web_reader::WebReader::from_config(config)));
        reg
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.definition().name.clone();
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Check if a tool exists in the registry.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// List all registered tool names.
    pub fn list_names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Validate that required parameters exist for a tool call.
    ///
    /// Currently exercised only by tests; retained for callers that want to
    /// pre-check native tool calls before dispatch.
    #[allow(dead_code)]
    pub fn validate_params(&self, name: &str, params: &serde_json::Value) -> Result<String> {
        let tool = match self.tools.get(name) {
            Some(t) => t,
            None => return Ok(format!("Unknown tool: {}", name)),
        };
        let schema = &tool.definition().parameters;
        let required = schema
            .get("required")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let mut missing = Vec::new();
        for field in &required {
            let field_name = field.as_str().unwrap_or("");
            if params.get(field_name).is_none() {
                missing.push(field_name.to_string());
            }
        }
        if missing.is_empty() {
            Ok(String::new())
        } else {
            // Echo back the bad payload and the expected keys so the model can
            // see its own mistake (e.g. params nested under "param") instead of
            // getting a generic "missing" error it can't reason about.
            let expected: Vec<String> = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default();
            Ok(format!(
                "Missing required parameters: {}. You sent: {}. Expected top-level keys: {}. \
                 Parameters must sit at the top level of the JSON object inside <tool_call>, \
                 not nested under 'param' or 'parameters'.",
                missing.join(", "),
                params,
                expected.join(", ")
            ))
        }
    }

    pub fn definitions(&self) -> Vec<ToolDef> {
        let mut defs: Vec<_> = self.tools.values().map(|t| t.definition()).collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Plain-text listing of tool names + descriptions, one per line. Used to
    /// teach the planner what tools exist without enabling native tool-calling
    /// (the planner only emits text steps that reference tools by name).
    pub fn descriptions_text(&self) -> String {
        self.definitions()
            .into_iter()
            .map(|d| format!("- {}: {}", d.name, d.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Per-tool parameter schema, one line per tool. Sorts by name so the byte
    /// output is stable across turns (KV-cache friendly). Each line lists the
    /// tool name, its parameters with required/optional markers and defaults,
    /// and the short description. Used to teach the executor exactly which
    /// top-level keys each tool expects.
    pub fn schema_text(&self) -> String {
        self.definitions()
            .into_iter()
            .map(|d| format_tool_schema(&d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Ollama-format tool definitions for the API request.
    /// Currently exercised only by tests after the agent-loop removal — retained for
    /// any future caller that wants `/api/chat`-style native tool-calling.
    #[allow(dead_code)]
    pub fn ollama_tool_definitions(&self) -> Vec<serde_json::Value> {
        self.definitions()
            .into_iter()
            .map(|d| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": d.name,
                        "description": d.description,
                        "parameters": d.parameters,
                    }
                })
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new(
            std::path::PathBuf::from("."),
            &crate::config::Config::default(),
        )
    }
}

/// Render a single tool definition as one schema line for `schema_text()`.
/// Format: `- {name}: required=[a, b], optional=[c]. {description}`
/// Tools with no parameters omit the param list. Sorted order is the caller's job.
fn format_tool_schema(def: &ToolDef) -> String {
    let required: Vec<String> = def
        .parameters
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let all_props: Vec<String> = def
        .parameters
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    let optional: Vec<&str> = all_props
        .iter()
        .filter(|p| !required.iter().any(|r| r == *p))
        .map(String::as_str)
        .collect();

    let header = match (required.is_empty(), optional.is_empty()) {
        (true, true) => format!("- {}", def.name),
        (false, true) => format!("- {}: required=[{}]", def.name, required.join(", ")),
        (true, false) => format!("- {}: optional=[{}]", def.name, optional.join(", ")),
        (false, false) => format!(
            "- {}: required=[{}], optional=[{}]",
            def.name,
            required.join(", "),
            optional.join(", ")
        ),
    };
    format!("{}. {}", header, def.description)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_registry() -> ToolRegistry {
        ToolRegistry {
            tools: HashMap::new(),
        }
    }

    struct EchoTool;

    impl Tool for EchoTool {
        fn execute(&self, params: serde_json::Value, call_id: String) -> Result<ToolResult> {
            let msg = params.get("msg").and_then(|v| v.as_str()).unwrap_or("echo");
            Ok(ToolResult::ok("echo", call_id, msg))
        }
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "echo".into(),
                description: "Echo tool".into(),
                parameters: serde_json::json!({"type": "object", "properties": {"msg": {"type": "string"}}}),
            }
        }
    }

    struct NoOpTool;

    impl Tool for NoOpTool {
        fn execute(&self, _: serde_json::Value, call_id: String) -> Result<ToolResult> {
            Ok(ToolResult::ok("noop", call_id, ""))
        }
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "noop".into(),
                description: "Does nothing".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            }
        }
    }

    #[test]
    fn register_and_get() {
        let mut reg = empty_registry();
        reg.register(Box::new(EchoTool));
        assert!(reg.get("echo").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn execute_tool() {
        let mut reg = empty_registry();
        reg.register(Box::new(EchoTool));
        let tool = reg.get("echo").unwrap();
        let result = tool
            .execute(serde_json::json!({"msg": "hello"}), "c1".into())
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "hello");
    }

    struct RequiredTool;

    impl Tool for RequiredTool {
        fn execute(&self, params: serde_json::Value, call_id: String) -> Result<ToolResult> {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ToolResult::ok("req_tool", call_id, path))
        }
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "req_tool".into(),
                description: "Tool with required params".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            }
        }
    }

    #[test]
    fn has_tool_checks() {
        let mut reg = empty_registry();
        reg.register(Box::new(EchoTool));
        assert!(reg.has_tool("echo"));
        assert!(!reg.has_tool("nonexistent"));
    }

    #[test]
    fn list_names_returns_all() {
        let mut reg = empty_registry();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(RequiredTool));
        let names = reg.list_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"req_tool"));
    }

    #[test]
    fn validate_params_passes_with_required() {
        let mut reg = empty_registry();
        reg.register(Box::new(RequiredTool));
        let result = reg
            .validate_params(
                "req_tool",
                &serde_json::json!({"path": "a.rs", "content": "hi"}),
            )
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn validate_params_catches_missing() {
        let mut reg = empty_registry();
        reg.register(Box::new(RequiredTool));
        let result = reg
            .validate_params("req_tool", &serde_json::json!({"path": "a.rs"}))
            .unwrap();
        assert!(result.contains("content"));
    }

    struct MixedTool;

    impl Tool for MixedTool {
        fn execute(&self, _: serde_json::Value, call_id: String) -> Result<ToolResult> {
            Ok(ToolResult::ok("mixed", call_id, ""))
        }
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "mixed".into(),
                description: "Has both required and optional params".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" },
                        "max_length": { "type": "integer" }
                    },
                    "required": ["url"]
                }),
            }
        }
    }

    #[test]
    fn schema_text_lists_required_and_optional_params() {
        let mut reg = empty_registry();
        reg.register(Box::new(MixedTool));
        let text = reg.schema_text();
        assert!(
            text.contains("required=[url]"),
            "expected required marker, got: {}",
            text
        );
        assert!(
            text.contains("optional=[max_length]"),
            "expected optional marker, got: {}",
            text
        );
        assert!(text.contains("mixed"), "expected tool name, got: {}", text);
    }

    #[test]
    fn schema_text_sorted_by_name() {
        let mut reg = empty_registry();
        // Register in reverse alphabetical order
        reg.register(Box::new(RequiredTool)); // "req_tool"
        reg.register(Box::new(EchoTool)); // "echo"
        reg.register(Box::new(NoOpTool)); // "noop"
        let text = reg.schema_text();
        // Match the line prefix `- {name}` so the lookup is robust to header format.
        let echo_pos = text.find("- echo").expect("echo missing");
        let noop_pos = text.find("- noop").expect("noop missing");
        let req_pos = text.find("- req_tool").expect("req_tool missing");
        assert!(echo_pos < noop_pos, "echo should come before noop");
        assert!(noop_pos < req_pos, "noop should come before req_tool");
    }

    #[test]
    fn validate_params_echoes_bad_payload() {
        let mut reg = empty_registry();
        reg.register(Box::new(MixedTool));
        // Model wrapped params under "param" — the bug we just fixed.
        let bad = serde_json::json!({"param": {"url": "https://x.com"}});
        let result = reg.validate_params("mixed", &bad).unwrap();
        assert!(
            result.contains("\"param\""),
            "error should echo the bad payload, got: {}",
            result
        );
        assert!(
            result.contains("url"),
            "error should name the missing required key, got: {}",
            result
        );
        assert!(
            result.contains("top level"),
            "error should explain the top-level rule, got: {}",
            result
        );
    }

    #[test]
    fn validate_params_unknown_tool() {
        let reg = empty_registry();
        let result = reg
            .validate_params("unknown", &serde_json::json!({}))
            .unwrap();
        assert!(result.contains("Unknown tool"));
    }

    #[test]
    fn ollama_definitions_format() {
        let mut reg = empty_registry();
        reg.register(Box::new(EchoTool));
        let defs = reg.ollama_tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["type"], "function");
        assert_eq!(defs[0]["function"]["name"], "echo");
    }

    #[test]
    fn descriptions_text_lists_name_and_description() {
        let mut reg = empty_registry();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(NoOpTool));
        let text = reg.descriptions_text();
        // Each registered tool contributes one line, prefixed with "- " + name.
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("- echo: "));
        assert!(text.contains("- noop: "));
    }
}
