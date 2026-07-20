use serde::{Deserialize, Serialize};

/// A tool call parsed from an LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub call_id: String,
    pub parameters: serde_json::Value,
    /// Set when the JSON params failed to parse (parameters fell back to `{}`).
    /// Dispatch should treat this as a malformed call and retry for correction
    /// rather than executing with empty params — local models frequently corrupt
    /// large JSON content blobs, and the generic "missing params" validation
    /// error gives the model no actionable signal about the real syntax mistake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
}

/// Diagnostic info about why tool call parsing failed.
#[derive(Debug, Clone)]
pub struct ParseDiagnostics {
    /// Tags found in text that suggest a tool call was attempted.
    pub hints_found: Vec<String>,
    /// Description of what was tried and why it failed.
    pub failure_reasons: Vec<String>,
    /// Number of `<tool_call name="...">` openers with no matching `</tool_call>`
    /// before the next opener or end of text. Non-zero means the response is
    /// malformed in a way the parser silently dropped — callers should retry.
    pub orphan_opens: usize,
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

    /// True if the response contained `<tool_call>` openers with no matching
    /// close before the next opener. Independent of `is_failed_attempt`: even
    /// if some calls parsed, orphan openers signal the model's output was
    /// malformed and the round should be retried.
    pub fn has_orphan_opens(&self) -> bool {
        self.diagnostics.orphan_opens > 0
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

    // Detect orphan openers before any other failure-path reason. We want this
    // signal to reach callers even when some valid calls also parsed — without
    // it, a single bad opener followed by a valid call would be silently
    // dropped and the pipeline would never trigger a correction retry.
    let orphan_ranges = find_orphan_ranges(text);
    if !orphan_ranges.is_empty() {
        reasons.push(format!(
            "Found {} <tool_call> opening tag(s) without a matching </tool_call> close \
             before the next opener or end of response. Every opener must be followed by \
             its JSON params and a </tool_call> close before any prose or next opener.",
            orphan_ranges.len()
        ));
    }

    // Try JSON-format tool calls first
    let json_calls = parse_json_calls_inner(text, &mut reasons);

    let calls = if !json_calls.is_empty() {
        json_calls
    } else {
        // If JSON found tags but no valid calls, record why. Skip when orphan
        // detection already explained the failure to avoid duplicate reasons.
        if text.contains("<tool_call") && orphan_ranges.is_empty() {
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
            orphan_opens: orphan_ranges.len(),
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
        let (parameters, parse_error) = match serde_json::from_str::<serde_json::Value>(params_str)
        {
            Ok(v) if v.is_object() => (v, None),
            Ok(_) => {
                let msg = format!("Parameters for '{}' is not a JSON object: {}", name, params_str);
                reasons.push(msg.clone());
                (serde_json::json!({}), Some(msg))
            }
            Err(e) => {
                let msg = format!(
                    "Invalid JSON in parameters for '{}': {} — raw: {}",
                    name, e, params_str
                );
                reasons.push(msg.clone());
                (serde_json::json!({}), Some(msg))
            }
        };

        // Small models often emit {"param": {...}} or {"parameters": {...}}
        // instead of putting the keys at the top level. Unwrap the common
        // envelope keys so dispatch can proceed; the prompts also warn against
        // this, but the fallback keeps a single mistake from failing the step.
        let parameters = unwrap_param_envelope(parameters);

        if !name.is_empty() {
            calls.push(ToolCall {
                name,
                call_id,
                parameters,
                parse_error,
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
        parse_error: None,
    })
}

fn uuid_short() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

/// Hoist `{"param": {...}}` / `{"parameters": {...}}` / `{"args": {...}}` /
/// `{"input": {...}}` to the inner object. Only fires when the envelope is the
/// sole key and the inner value is itself an object — leaves legitimate
/// single-key params (e.g. `{"path": "x"}`) untouched.
fn unwrap_param_envelope(v: serde_json::Value) -> serde_json::Value {
    let obj = match v.as_object() {
        Some(o) if o.len() == 1 => o,
        _ => return v,
    };
    let (key, inner) = obj.iter().next().expect("len == 1 guaranteed");
    let is_envelope = matches!(
        key.as_str(),
        "param" | "parameters" | "args" | "input"
    );
    if is_envelope && inner.is_object() {
        inner.clone()
    } else {
        v
    }
}

/// Find every `<tool_call name="...">` opener that lacks a matching
/// `</tool_call>` close before the next opener (or end of text). Returns the
/// `(start, end)` byte ranges an orphan occupies: `start` is the opener's
/// start, `end` is the next opener's start or `text.len()` for trailing
/// orphans. Used both by the sanitizer (to redact) and the parser (to count).
///
/// Tool calls never nest, so this pairs each opener with the next available
/// close that sits before the following opener. An opener with no close in
/// that window is malformed — the previous implementation only checked for
/// any `</tool_call>` later in the text, which fooled it when a *different*
/// call's close tag existed downstream (e.g. an unclosed `web_reader`
/// immediately followed by a valid `write_file`).
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
/// Valid, complete tool calls are preserved. Only applies to text shown to the user
/// — the parser still sees the raw input for tool dispatch.
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

// ---------------------------------------------------------------------------
// Inline `<think>…</think>` reasoning-block stripping
// ---------------------------------------------------------------------------

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// Length of the longest suffix of `s` that is also a (proper) prefix of `tag`.
///
/// Used to decide how much trailing text to hold back when a `<think>` or
/// `</think>` tag might be split across two stream chunks. `tag` is ASCII, so
/// byte-length slicing is char-boundary safe.
fn trailing_tag_prefix_len(s: &str, tag: &str) -> usize {
    if s.is_empty() || tag.len() <= 1 {
        return 0;
    }
    let max = std::cmp::min(s.len(), tag.len() - 1);
    let mut best = 0;
    for len in 1..=max {
        if let Some(suffix) = s.get(s.len() - len..) {
            if tag.starts_with(suffix) {
                best = len;
            }
        }
    }
    best
}

/// Stateful streaming stripper for inline `<think>…</think>` reasoning blocks.
///
/// Models that lack a proper chat template (notably on `/api/generate`) often
/// emit reasoning inline as `<think>…</think>` *inside* the response text rather
/// than via Ollama's separate `thinking` field. This removes those blocks from
/// the streamed response so the tool parser and display never see raw reasoning.
///
/// Robustness rule (by design): if a `<think>` opener is never closed by the
/// time the stream ends, the opener tag is dropped but its buffered content is
/// *kept as response* (`finish()` flushes it) — silently discarding it would
/// lose potentially-valid output. In that case the thinking text leaks into the
/// response, which is the intended fallback.
///
/// Tag boundaries may fall between chunks; a possible partial-tag suffix is held
/// back until the next chunk resolves it, so a `<think>` split as `<thi` + `nk>`
/// is still recognized.
pub struct ThinkStripper {
    pending: String,
    in_think: bool,
}

impl Default for ThinkStripper {
    fn default() -> Self {
        Self::new()
    }
}

impl ThinkStripper {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
            in_think: false,
        }
    }

    /// Feed one chunk of response text. Returns the portion that is safe to emit
    /// as clean response right now. Text that might be part of a tag split across
    /// the chunk boundary is held back until the next call.
    pub fn feed(&mut self, chunk: &str) -> String {
        self.pending.push_str(chunk);
        self.drain(false)
    }

    /// Call when the stream has ended. Returns any remaining clean text. Per the
    /// unclosed-think rule, buffered thinking content is flushed as response.
    pub fn finish(&mut self) -> String {
        let mut out = self.drain(true);
        if self.in_think {
            // Unclosed `<think>`: keep buffered content as response.
            out.push_str(&self.pending);
        }
        self.pending.clear();
        self.in_think = false;
        out
    }

    /// Process `pending` as far as can be decided. When `final_pass` is false,
    /// a possible partial-tag suffix is held back for the next chunk.
    fn drain(&mut self, final_pass: bool) -> String {
        let mut out = String::new();
        loop {
            let open_idx = self.pending.find(THINK_OPEN);
            let close_idx = self.pending.find(THINK_CLOSE);
            if self.in_think {
                // Inside a think block: only a closer ends it. Until one arrives,
                // hold ALL buffered content — if the stream ends with no closer,
                // finish() must still be able to flush it as response.
                match close_idx {
                    Some(c) => {
                        self.pending.drain(..c + THINK_CLOSE.len());
                        self.in_think = false;
                        continue;
                    }
                    None => return out,
                }
            } else {
                match (open_idx, close_idx) {
                    // Stray closer with no preceding opener (or after an opener
                    // we already closed): strip just the tag, keep surrounding text.
                    (Some(o), Some(c)) if c < o => {
                        out.push_str(&self.pending[..c]);
                        self.pending.drain(..c + THINK_CLOSE.len());
                        continue;
                    }
                    (None, Some(c)) => {
                        out.push_str(&self.pending[..c]);
                        self.pending.drain(..c + THINK_CLOSE.len());
                        continue;
                    }
                    // Opener (first, or only): emit preceding text, enter think.
                    (Some(o), _) => {
                        out.push_str(&self.pending[..o]);
                        self.pending.drain(..o + THINK_OPEN.len());
                        self.in_think = true;
                        continue;
                    }
                    // No tag at all: emit (or hold back a partial-tag suffix).
                    (None, None) => {
                        if final_pass {
                            out.push_str(&self.pending);
                            self.pending.clear();
                        } else {
                            let hold = trailing_tag_prefix_len(&self.pending, THINK_OPEN)
                                .max(trailing_tag_prefix_len(&self.pending, THINK_CLOSE));
                            let keep = self.pending.len() - hold;
                            out.push_str(&self.pending[..keep]);
                            self.pending.drain(..keep);
                        }
                        return out;
                    }
                }
            }
        }
    }
}

/// One-shot convenience: strip complete `<think>…</think>` blocks from `text`,
/// keeping content from any unclosed `<think>` as response. Useful for non-streamed
/// responses (e.g. `/api/chat`); the streaming path uses `ThinkStripper` directly.
#[allow(dead_code)]
pub fn strip_think_blocks(text: &str) -> String {
    let mut s = ThinkStripper::new();
    let mut out = s.feed(text);
    out.push_str(&s.finish());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_strip_complete_block() {
        let text = "<think>let me reason</think>Here is the answer.";
        assert_eq!(strip_think_blocks(text), "Here is the answer.");
    }

    #[test]
    fn think_strip_multiple_blocks() {
        let text = "<think>a</think>part1<think>b</think>part2";
        assert_eq!(strip_think_blocks(text), "part1part2");
    }

    #[test]
    fn think_strip_unclosed_keeps_content() {
        // No </think>: opener removed, content kept as response (robustness rule).
        let text = "<think>some reasoning that never closes\nactual-ish content";
        assert_eq!(
            strip_think_blocks(text),
            "some reasoning that never closes\nactual-ish content"
        );
    }

    #[test]
    fn think_strip_no_tags_unchanged() {
        let text = "just a normal response with <tool_call> adjacent";
        assert_eq!(strip_think_blocks(text), text);
    }

    #[test]
    fn think_strip_stray_close_removed() {
        // </think> without an opener: tag removed, text kept.
        let text = "response</think>tail";
        assert_eq!(strip_think_blocks(text), "responsetail");
    }

    #[test]
    fn think_strip_does_not_touch_tool_call() {
        let text = "<think>plan</think><tool_call name=\"web_reader\" call_id=\"1\">{\"url\":\"x\"}</tool_call>";
        let stripped = strip_think_blocks(text);
        assert!(stripped.contains("<tool_call name=\"web_reader\""));
        assert!(stripped.contains("</tool_call>"));
        assert!(!stripped.contains("<think>"));
    }

    #[test]
    fn think_strip_streaming_split_tag() {
        // `<think>` split across chunks: `<thi` + `nk>`.
        let mut s = ThinkStripper::new();
        let a = s.feed("before<thi");
        let b = s.feed("nk>reason");
        let c = s.feed("</think>after");
        let d = s.finish();
        let combined = format!("{}{}{}{}", a, b, c, d);
        assert_eq!(combined, "beforeafter");
    }

    #[test]
    fn think_strip_streaming_split_close() {
        // `</think>` split across chunks, with content after.
        let mut s = ThinkStripper::new();
        let a = s.feed("<think>reasoning</thin");
        let b = s.feed("k>response");
        let c = s.finish();
        assert_eq!(format!("{}{}{}", a, b, c), "response");
    }

    #[test]
    fn think_strip_streaming_unclosed_flushes_at_finish() {
        let mut s = ThinkStripper::new();
        let a = s.feed("hello <think>still thinking");
        let b = s.finish();
        // "hello " emitted normally; the unclosed thinking flushed as response.
        assert_eq!(format!("{}{}", a, b), "hello still thinking");
    }

    #[test]
    fn think_strip_streaming_partial_prefix_resolved() {
        // Trailing "<t" is a potential tag prefix; held back then resolved as
        // normal text when it doesn't continue into "<think>".
        let mut s = ThinkStripper::new();
        let a = s.feed("text <t");
        let b = s.feed("able> not a think tag");
        let c = s.finish();
        assert_eq!(format!("{}{}{}", a, b, c), "text <table> not a think tag");
    }

    #[test]
    fn trailing_prefix_basic() {
        assert_eq!(trailing_tag_prefix_len("abc<thi", THINK_OPEN), 4); // "<thi"
        assert_eq!(trailing_tag_prefix_len("xyz", THINK_OPEN), 0);
        assert_eq!(trailing_tag_prefix_len("</thin", THINK_CLOSE), 6); // "</thin"
    }

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

    #[test]
    fn parses_param_envelope_unwrap() {
        // Model emitted {"param": {...}} — the bug that motivated this fix.
        let text = r#"<tool_call name="web_reader" call_id="x">{"param": {"url": "https://example.com"}}</tool_call>"#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].parameters["url"], "https://example.com");
        assert!(
            calls[0].parameters.get("param").is_none(),
            "param envelope should be unwrapped"
        );
    }

    #[test]
    fn parses_parameters_envelope_unwrap() {
        // Same shape with the full word "parameters" as the wrapper.
        let text = r#"<tool_call name="exec_shell" call_id="y">{"parameters": {"command": "cargo test"}}</tool_call>"#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].parameters["command"], "cargo test");
    }

    #[test]
    fn does_not_unwrap_non_envelope() {
        // Legitimate single-key params must NOT be hoisted — "path" isn't an envelope key.
        let text = r#"<tool_call name="read_file" call_id="z">{"path": "src/main.rs"}</tool_call>"#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].parameters["path"], "src/main.rs");
    }

    #[test]
    fn does_not_unwrap_envelope_with_primitive_value() {
        // {"param": "url=..."} — value is a string, not an object. Leave alone; the
        // validate_params error will surface the problem to the model.
        let text = r#"<tool_call name="web_reader" call_id="w">{"param": "url=https://x.com"}</tool_call>"#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].parameters["param"], "url=https://x.com");
    }

    // --- Malformed-JSON surface (parse_error) ---

    #[test]
    fn malformed_json_params_records_parse_error() {
        // The bug report's shape: write_file with a huge inline content blob plus
        // a bogus unquoted `"action": create"` field. serde rejects it; the parser
        // must surface parse_error so dispatch retries for correction instead of
        // executing with empty params.
        let text = r##"<tool_call name="write_file" call_id="2">{"path": "out.md", "content": "# hi", "action": create"}</tool_call>"##;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(
            calls[0].parameters.as_object().map_or(true, |o| o.is_empty()),
            "malformed params should fall back to empty object"
        );
        let err = calls[0]
            .parse_error
            .as_ref()
            .expect("parse_error should be set on malformed JSON");
        assert!(err.contains("Invalid JSON"), "unexpected error text: {}", err);
    }

    #[test]
    fn valid_json_params_have_no_parse_error() {
        let text = r#"<tool_call name="write_file" call_id="2">{"path": "out.md", "content": "hi"}</tool_call>"#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].parse_error.is_none());
        assert_eq!(calls[0].parameters["path"], "out.md");
    }

    #[test]
    fn mixed_valid_and_malformed_calls_flag_only_bad_one() {
        // web_reader valid, write_file malformed in the same response.
        let text = r##"<tool_call name="web_reader" call_id="1">{"url": "https://example.com"}</tool_call><tool_call name="write_file" call_id="2">{"path": "x", "action": create"}</tool_call>"##;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert!(calls[0].parse_error.is_none());
        assert!(calls[1].parse_error.is_some());
    }

    // --- Orphan opener detection (parser-side) ---

    #[test]
    fn trailing_orphan_opener_is_flagged() {
        // Opener with no close at all — orphan.
        let text = r#"<tool_call name="web_reader" call_id="4">{"url": "https://example.com"}"#;
        let result = parse_tool_calls_with_diagnostics(text);
        assert_eq!(result.diagnostics.orphan_opens, 1);
        assert!(result.has_orphan_opens());
        assert!(result.calls.is_empty());
    }

    #[test]
    fn orphan_opener_followed_by_valid_call_still_flagged() {
        // The exact shape from the bug report: unclosed web_reader opener
        // followed by prose, then a properly-closed write_file. Previously the
        // parser silently dropped the orphan and only saw write_file.
        let text = r#"<tool_call name="web_reader" call_id="4">{"url": "https://example.com"}I'll guess.
<tool_call name="write_file" call_id="5">{"path": "out.md"}</tool_call>"#;
        let result = parse_tool_calls_with_diagnostics(text);
        assert_eq!(result.diagnostics.orphan_opens, 1);
        // write_file still parses — orphan detection is additive, not a replacement.
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.calls[0].name, "write_file");
    }

    #[test]
    fn no_orphans_when_all_calls_close() {
        let text = r#"<tool_call name="read_file" call_id="a">{"path": "x"}</tool_call>
<tool_call name="list_dir" call_id="b">{"path": "y"}</tool_call>"#;
        let result = parse_tool_calls_with_diagnostics(text);
        assert_eq!(result.diagnostics.orphan_opens, 0);
        assert!(!result.has_orphan_opens());
        assert_eq!(result.calls.len(), 2);
    }

    #[test]
    fn trailing_orphan_after_valid_call() {
        // Valid call followed by an opener that never closes — orphan.
        let text = r#"<tool_call name="read_file" call_id="a">{"path": "x"}</tool_call>
<tool_call name="web_reader" call_id="b">{"url": "https://x.com"}"#;
        let result = parse_tool_calls_with_diagnostics(text);
        assert_eq!(result.diagnostics.orphan_opens, 1);
        assert_eq!(result.calls.len(), 1);
    }

    #[test]
    fn multiple_orphans_all_counted() {
        // Two orphans, one valid in between.
        let text = r#"<tool_call name="a">{"x": 1}
<tool_call name="b">{"y": 2}</tool_call>
<tool_call name="c">{"z": 3}"#;
        let result = parse_tool_calls_with_diagnostics(text);
        assert_eq!(result.diagnostics.orphan_opens, 2);
    }

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
        // The bug case: unclosed web_reader followed by a valid write_file.
        // The orphan's prose should be redacted but write_file must survive.
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
        // Opener without name attribute — pre-existing behavior.
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
