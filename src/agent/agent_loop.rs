use crate::agent::tools_parser::parse_tool_calls_with_diagnostics;
use crate::ollama::chat::ChatMessage;
use crate::ollama::OllamaClient;
use crate::tools::{ToolRegistry, ToolResult};
use anyhow::Result;

/// Max retries for tool call format correction before giving up.
const MAX_CORRECTION_RETRIES: usize = 2;

/// Keywords that suggest the model is trying to call a tool but formatting wrong.
#[allow(dead_code)]
const TOOL_NAME_HINTS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "exec_shell",
    "web_search",
    "web_reader",
];

/// Configuration for the agent loop.
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    pub max_steps: usize,
    pub enable_tools: bool,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 10,
            enable_tools: true,
        }
    }
}

/// Events emitted during the agent loop.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    ToolStart {
        tool_name: String,
        call_id: String,
    },
    ToolResult {
        result: ToolResult,
    },
    TextChunk {
        content: String,
    },
    Done {
        content: String,
        steps: usize,
    },
    Error {
        message: String,
    },
}

/// Run the agent loop: LLM → parse tool calls → execute → feed back → repeat.
///
/// This is the core "tool-use loop" pattern shared by all production coding agents.
pub async fn run_agent_loop(
    client: &OllamaClient,
    model: &str,
    tools: &ToolRegistry,
    system_prompt: &str,
    user_request: &str,
    config: &AgentLoopConfig,
    mut event_sink: impl FnMut(AgentEvent),
) -> Result<String> {
    let mut messages = vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(user_request),
    ];

    let mut step = 0;
    let mut final_content = String::new();
    let mut prev_tool_signature = String::new();
    let mut correction_retries = 0;

    loop {
        if step >= config.max_steps {
            event_sink(AgentEvent::Error {
                message: format!("Agent loop exceeded max steps ({})", config.max_steps),
            });
            break;
        }

        // Build tool definitions for the request
        let tool_defs = if config.enable_tools {
            tools.ollama_tool_definitions()
        } else {
            vec![]
        };

        // Call LLM (streaming — emits TextChunk events as text arrives)
        let sink = &mut event_sink;
        let response = match client
            .chat_with_tools_streaming(model, messages.clone(), &tool_defs, |chunk| {
                sink(AgentEvent::TextChunk {
                    content: chunk.to_string(),
                });
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                event_sink(AgentEvent::Error {
                    message: format!("LLM call failed: {}", e),
                });
                return Err(e);
            }
        };

        let content = response.content;

        // Native tool_calls (from message.tool_calls) take precedence over text-format
        // parsing — models like qwen3.5 emit them this way when given a `tools` field,
        // leaving `content` empty. Without this branch, those calls were silently dropped.
        let native_calls: Vec<crate::agent::tools_parser::ToolCall> = response
            .tool_calls
            .iter()
            .enumerate()
            .map(|(i, tc)| crate::agent::tools_parser::ToolCall {
                name: tc.name.clone(),
                call_id: format!("native-{}", i),
                parameters: tc.arguments.clone(),
            })
            .collect();

        let parse_result = if !native_calls.is_empty() {
            crate::agent::tools_parser::ParseResult {
                calls: native_calls,
                diagnostics: crate::agent::tools_parser::ParseDiagnostics {
                    hints_found: vec![],
                    failure_reasons: vec![],
                },
            }
        } else if config.enable_tools {
            parse_tool_calls_with_diagnostics(&content)
        } else {
            crate::agent::tools_parser::ParseResult {
                calls: vec![],
                diagnostics: crate::agent::tools_parser::ParseDiagnostics {
                    hints_found: vec![],
                    failure_reasons: vec![],
                },
            }
        };

        if parse_result.calls.is_empty() {
            // Check if this was a failed tool attempt that deserves a correction retry
            if parse_result.is_failed_attempt() && correction_retries < MAX_CORRECTION_RETRIES {
                correction_retries += 1;
                let diag_text = parse_result.diagnostics.format_for_correction();
                // Use reflexion prompt on the final attempt
                let correction_msg = if correction_retries >= MAX_CORRECTION_RETRIES {
                    format!(
                        "{}\n\n{}\n\n{}\n\nThis is your last attempt. Think carefully.",
                        crate::agent::prompts::TOOL_CORRECTION_PROMPT,
                        diag_text,
                        crate::agent::prompts::REFLEXION_PROMPT,
                    )
                } else {
                    format!(
                        "{}\n\n{}\n\nPlease output your tool call again using the correct format.",
                        crate::agent::prompts::TOOL_CORRECTION_PROMPT,
                        diag_text
                    )
                };
                tracing::warn!(
                    "tool parse failed (attempt {}), injecting correction",
                    correction_retries
                );

                // Feed the model's response + correction back and retry
                messages.push(ChatMessage::assistant(&content));
                messages.push(ChatMessage {
                    role: "tool".into(),
                    content: format!("ERROR: Tool call parsing failed.\n\n{}", correction_msg),
                });
                step += 1;
                continue;
            }

            // No tool calls and no correction needed — final response
            final_content = content;
            break;
        }

        // Valid tool calls parsed — reset correction counter
        correction_retries = 0;
        let tool_calls = parse_result.calls;

        // Detect infinite loop: same tool call signature as previous iteration
        let current_sig = tool_calls
            .iter()
            .map(|c| format!("{}:{}", c.name, c.parameters))
            .collect::<Vec<_>>()
            .join("|");
        if current_sig == prev_tool_signature {
            tracing::warn!("agent loop detected repeating tool call, breaking");
            final_content = content;
            break;
        }
        prev_tool_signature = current_sig;

        // Execute tool calls with validation
        let mut tool_results = Vec::new();
        let available_names = tools.list_names();
        for call in &tool_calls {
            // Validate tool name exists
            if !tools.has_tool(&call.name) {
                let err_msg = format!(
                    "Unknown tool '{}'. Available tools: {}",
                    call.name,
                    available_names.join(", ")
                );
                tool_results.push(ToolResult::err(&call.name, &call.call_id, err_msg));
                event_sink(AgentEvent::ToolResult {
                    result: tool_results.last().unwrap().clone(),
                });
                continue;
            }

            // Validate required parameters
            if let Ok(validation) = tools.validate_params(&call.name, &call.parameters) {
                if !validation.is_empty() {
                    tool_results.push(ToolResult::err(&call.name, &call.call_id, validation));
                    event_sink(AgentEvent::ToolResult {
                        result: tool_results.last().unwrap().clone(),
                    });
                    continue;
                }
            }

            event_sink(AgentEvent::ToolStart {
                tool_name: call.name.clone(),
                call_id: call.call_id.clone(),
            });

            let result = tools
                .get(&call.name)
                .unwrap()
                .execute(call.parameters.clone(), call.call_id.clone())
                .unwrap_or_else(|e| ToolResult::err(&call.name, &call.call_id, format!("{}", e)));

            event_sink(AgentEvent::ToolResult {
                result: result.clone(),
            });
            tool_results.push(result);
        }

        // Add assistant message (without tool call markers for clarity)
        let text_part = strip_tool_call_markers(&content);
        if !text_part.is_empty() {
            messages.push(ChatMessage::assistant(&text_part));
        } else {
            messages.push(ChatMessage::assistant("Executing tools..."));
        }

        // Add tool result messages
        for result in tool_results {
            let result_text = if result.success {
                format!(
                    "Tool {} ({}):\n{}",
                    result.tool_name, result.call_id, result.output
                )
            } else {
                format!(
                    "Tool {} ({}) FAILED:\n{}",
                    result.tool_name, result.call_id, result.output
                )
            };
            messages.push(ChatMessage {
                role: "tool".into(),
                content: result_text,
            });
        }

        step += 1;
    }

    event_sink(AgentEvent::Done {
        content: final_content.clone(),
        steps: step,
    });
    Ok(final_content)
}

/// Strip <tool_call...>...</tool_call markers from text to get the prose part.
fn strip_tool_call_markers(text: &str) -> String {
    let re = regex::Regex::new(r#"<tool_call[^>]*>.*?</tool_call\s*>"#).unwrap();
    let cleaned = re.replace_all(text, "").trim().to_string();
    // Also strip text-format Call: lines
    cleaned
        .lines()
        .filter(|l| !l.trim().starts_with("Call:"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Detect if a response looks like a failed tool call attempt (mentions tool names
/// but parsing returned nothing). Used to inject correction prompts for small models.
#[allow(dead_code)]
pub fn looks_like_failed_tool_call(text: &str) -> bool {
    let lower = text.to_lowercase();
    TOOL_NAME_HINTS.iter().any(|hint| lower.contains(hint))
        && !text.contains("<tool_call")
        && !text.contains("Call:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.max_steps, 10);
        assert!(config.enable_tools);
    }

    #[test]
    fn strip_markers_removes_tool_calls() {
        let text = "I'll read the file.\n<tool_call name=\"read_file\">{\"path\":\"a.rs\"}</tool_call >\nThen I'll edit it.";
        let stripped = strip_tool_call_markers(text);
        assert!(!stripped.contains("tool_call"));
        assert!(stripped.contains("I'll read the file"));
        assert!(stripped.contains("Then I'll edit it"));
    }

    #[test]
    fn strip_markers_removes_text_calls() {
        let text = "Let me check.\nCall: read_file(path=\"main.rs\")\nDone.";
        let stripped = strip_tool_call_markers(text);
        assert!(!stripped.contains("Call:"));
        assert!(stripped.contains("Let me check"));
        assert!(stripped.contains("Done."));
    }

    #[test]
    fn detects_failed_tool_attempt() {
        // Mentions tool names but no valid format
        assert!(looks_like_failed_tool_call(
            "I should use read_file to check main.rs"
        ));
        assert!(looks_like_failed_tool_call(
            "Let me exec_shell(cargo test) now"
        ));
    }

    #[test]
    fn no_false_positive_on_valid_calls() {
        // Has proper format — should NOT be flagged
        assert!(!looks_like_failed_tool_call(
            "I'll read it.\n<tool_call name=\"read_file\">{\"path\":\"a.rs\"}</tool_call"
        ));
        assert!(!looks_like_failed_tool_call(
            "Call: read_file(path=\"main.rs\")"
        ));
    }

    #[test]
    fn no_match_on_regular_text() {
        assert!(!looks_like_failed_tool_call("Hello, how are you?"));
    }
}
