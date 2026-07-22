use serde::{Deserialize, Serialize};

/// A tool call parsed from an LLM response. The native tool path populates this
/// from Ollama's `message.tool_calls` array; the field is kept for compatibility
/// with `parse_file_changes` and history types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub call_id: String,
    pub parameters: serde_json::Value,
}

/// Find every `<tool_call name="...">` opener that lacks a matching
/// `</tool_call>` close before the next opener (or end of text). Returns the
/// `(start, end)` byte ranges an orphan occupies: `start` is the opener's
/// start, `end` is the next opener's start or `text.len()` for trailing
/// orphans.
///
/// Tool calls never nest, so this pairs each opener with the next available
/// close that sits before the following opener. An opener with no close in
/// that window is malformed.
fn find_orphan_ranges(text: &str) -> Vec<(usize, usize)> {
    let opening_re = regex::Regex::new(
        r#"<tool_call\s+name="[^"]*"(?:\s+call_id="[^"]*")?\s*>"#,
    )
    .unwrap();
    let close_re = regex::Regex::new(r#"</tool_call"#).unwrap();

    let openings: Vec<_> = opening_re.find_iter(text).collect();
    let closes: Vec<_> = close_re.find_iter(text).collect();

    let mut ranges = Vec::new();
    for (i, open) in openings.iter().enumerate() {
        let open_end = open.end();
        let next_open_start = openings.get(i + 1).map(|o| o.start());
        let has_matching_close = closes.iter().any(|c| {
            c.start() >= open_end && next_open_start.map_or(true, |nop| c.start() < nop)
        });
        if !has_matching_close {
            let redact_end = next_open_start.unwrap_or(text.len());
            ranges.push((open.start(), redact_end));
        }
    }
    ranges
}

/// Sanitize LLM text output by redacting incomplete or invalid tool call markers.
///
/// Native tool calling delivers tool calls via Ollama's structured
/// `message.tool_calls` array, so this function is only defensive: a model can
/// still echo forged `<tool_call>` markers in its content stream, and we don't
/// want those rendered to the user as if they were real dispatch markers.
/// Valid, complete tool-call markers are preserved. Only applies to text shown
/// to the user — the dispatcher sees the structured array, not this scrubbed copy.
pub fn sanitize_output(text: &str) -> String {
    // Redact orphan openers (no matching close before next opener / end).
    // Each orphan's range runs from its own start to the next opener's start
    // (or end of text), so subsequent valid tool calls are preserved.
    let mut result = text.to_string();
    for (start, end) in find_orphan_ranges(text).into_iter().rev() {
        result.replace_range(start..end, "[invalid tool call]");
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

/// Returns true if `content` contains a meaningful natural-language answer —
/// i.e. prose text OUTSIDE of `<tool_call>` blocks, `<tool_result>` blocks,
/// `### FILE: …` file blocks, and fenced code blocks. Used by the executor's
/// final-answer fallback to decide whether a tool-using step ended with no
/// real answer (the model emitted only tool calls / file blocks / empty text
/// and "finished" without addressing the user).
///
/// Threshold: ≥3 surviving non-marker words counts as a real answer. Fewer
/// than that is treated as stray markers / whitespace, triggering the fallback.
pub fn has_final_answer(content: &str) -> bool {
    // Strip file blocks (### FILE: ... ``` ... ```) first, before generic fence
    // stripping, so the FILE marker and its fence are removed together.
    let file_re = regex::Regex::new(r"(?s)### FILE:.*?```").unwrap();
    let call_re = regex::Regex::new(r"(?s)<tool_call[^>]*>.*?</tool_call>").unwrap();
    let res_re = regex::Regex::new(r"(?s)<tool_result[^>]*>.*?</tool_result>").unwrap();
    // Also drop orphan / unclosed tool_call openers so a lone opener with no
    // prose doesn't count as an answer.
    let lone_call_re = regex::Regex::new(r"<tool_call[^>]*>").unwrap();
    let lone_res_re = regex::Regex::new(r"</tool_result>").unwrap();
    let fence_re = regex::Regex::new(r"(?s)```.*?```").unwrap();

    let s = file_re.replace_all(content, "");
    let s = call_re.replace_all(&s, "");
    let s = res_re.replace_all(&s, "");
    let s = lone_call_re.replace_all(&s, "");
    let s = lone_res_re.replace_all(&s, "");
    let s = fence_re.replace_all(&s, "");

    s.split_whitespace().count() >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Sanitizer (display-side) ---

    #[test]
    fn sanitize_trailing_orphan_redacted() {
        let text = r#"<tool_call name="web_reader" call_id="4">{"url": "https://example.com"}prose"#;
        let sanitized = sanitize_output(text);
        assert!(sanitized.contains("[invalid tool call]"));
        assert!(!sanitized.contains("<tool_call name=\"web_reader\""));
    }

    #[test]
    fn sanitize_orphan_then_valid_preserves_valid() {
        let text = r#"<tool_call name="web_reader" call_id="4">{"url": "https://example.com"}I'll guess.
<tool_call name="write_file" call_id="5">{"path": "out.md"}</tool_call>"#;
        let sanitized = sanitize_output(text);
        assert!(sanitized.contains("[invalid tool call]"));
        assert!(sanitized.contains("<tool_call name=\"write_file\""));
        assert!(sanitized.contains("</tool_call"));
        // The orphan's opening tag must not appear in the display.
        assert!(!sanitized.contains("<tool_call name=\"web_reader\""));
    }

    #[test]
    fn sanitize_no_orphans_preserves_all() {
        let text = r#"<tool_call name="read_file" call_id="a">{"path": "x"}</tool_call>"#;
        let sanitized = sanitize_output(text);
        assert_eq!(sanitized, text);
    }

    #[test]
    fn sanitize_lone_tag_still_redacted() {
        // Opener without name attribute.
        let text = r#"Some text <tool_call> oops"#;
        let sanitized = sanitize_output(text);
        assert!(sanitized.contains("[invalid tool call]"));
    }

    #[test]
    fn has_final_answer_empty_is_false() {
        assert!(!has_final_answer(""));
        assert!(!has_final_answer("   \n\t  "));
    }

    #[test]
    fn has_final_answer_tool_call_only_is_false() {
        let text = r#"<tool_call name="exec_shell" call_id="1">{"command": "ls"}</tool_call>"#;
        assert!(!has_final_answer(text));
    }

    #[test]
    fn has_final_answer_tool_result_only_is_false() {
        let text = r#"<tool_result tool="exec_shell" call_id="1">{"success": true, "output": "5"}</tool_result>"#;
        assert!(!has_final_answer(text));
    }

    #[test]
    fn has_final_answer_file_block_only_is_false() {
        let text = "### FILE: hello.txt\n### ACTION: create\n```\nhi\n```\n";
        assert!(!has_final_answer(text));
    }

    #[test]
    fn has_final_answer_prose_is_true() {
        assert!(has_final_answer("There are 5 files in the current directory."));
        assert!(has_final_answer("Done. I created the file and added the function."));
    }

    #[test]
    fn has_final_answer_prose_with_tool_call_is_true() {
        // Prose answer plus a trailing tool call — the prose survives.
        let text = "There are 5 files.\n<tool_call name=\"exec_shell\" call_id=\"2\">{\"command\": \"echo done\"}</tool_call>";
        assert!(has_final_answer(text));
    }

    #[test]
    fn has_final_answer_stray_markers_below_threshold_is_false() {
        // Fewer than 3 surviving words — just stray markers.
        assert!(!has_final_answer("ok"));
        assert!(!has_final_answer("### FILE:"));
    }
}
