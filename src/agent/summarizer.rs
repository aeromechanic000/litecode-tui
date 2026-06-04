use crate::app::ConversationMessage;
use crate::ollama::chat::ChatMessage;
use crate::ollama::OllamaClient;
use crate::util::text::estimate_tokens;
use anyhow::Result;

/// Priority for messages — pinned messages survive compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePriority {
    Pinned,
    Normal,
}

/// Determine priority based on message content.
pub fn message_priority(msg: &ConversationMessage) -> MessagePriority {
    if msg.role == "system" && msg.content.contains("Error:") {
        return MessagePriority::Pinned;
    }
    if msg.role == "user" && (msg.content.contains("/yes") || msg.content.contains("/confirm")) {
        return MessagePriority::Pinned;
    }
    if msg.role == "assistant" && msg.content.contains("### FILE:") {
        return MessagePriority::Pinned;
    }
    MessagePriority::Normal
}

/// Configuration for summarization behavior.
#[derive(Debug, Clone)]
pub struct SummarizerConfig {
    pub keep_recent_count: usize,
    pub trigger_threshold_percent: usize,
    pub min_messages_for_summary: usize,
}

impl Default for SummarizerConfig {
    fn default() -> Self {
        Self {
            keep_recent_count: 6,
            trigger_threshold_percent: 80,
            min_messages_for_summary: 12,
        }
    }
}

/// Result of a summarization operation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SummaryResult {
    pub summary: String,
    pub summarized_count: usize,
    pub kept_count: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

const SUMMARIZE_SYSTEM: &str = r#"You are a conversation summarizer. Condense the given conversation into key points:
- Decisions made (with reasoning)
- Files created/modified (with paths)
- Important context and constraints
- Errors encountered and how they were resolved

Output: Bulleted list, 200 words max. Be specific: use actual file paths and technical details."#;

/// Check if summarization is needed based on token usage.
pub fn needs_summarization(
    history: &[ConversationMessage],
    context_window: usize,
    config: &SummarizerConfig,
) -> bool {
    if history.len() < config.min_messages_for_summary {
        return false;
    }
    let total_tokens: usize = history.iter().map(|m| m.tokens).sum();
    let threshold = (context_window * config.trigger_threshold_percent) / 100;
    total_tokens > threshold
}

/// Perform summarization using the exec model.
///
/// Splits history into: pinned (always kept) + recent N (verbatim) + older (summarized).
/// Returns the summary string and statistics.
pub async fn summarize(
    client: &OllamaClient,
    exec_model: &str,
    history: &[ConversationMessage],
    config: &SummarizerConfig,
) -> Result<SummaryResult> {
    let tokens_before: usize = history.iter().map(|m| m.tokens).sum();

    let mut pinned = Vec::new();
    let mut normal = Vec::new();
    for msg in history {
        match message_priority(msg) {
            MessagePriority::Pinned => pinned.push(msg),
            MessagePriority::Normal => normal.push(msg),
        }
    }

    let keep_count = config.keep_recent_count.saturating_sub(pinned.len());
    let split_idx = normal.len().saturating_sub(keep_count);
    let (to_summarize, recent) = normal.split_at(split_idx);

    if to_summarize.is_empty() {
        return Ok(SummaryResult {
            summary: String::new(),
            summarized_count: 0,
            kept_count: pinned.len() + recent.len(),
            tokens_before,
            tokens_after: tokens_before,
        });
    }

    let summarize_prompt = build_summarize_prompt(to_summarize);
    let messages = vec![
        ChatMessage::system(SUMMARIZE_SYSTEM),
        ChatMessage::user(&summarize_prompt),
    ];

    let response = client.chat(exec_model, messages, false).await?;
    let summary = response.content;

    let summary_tokens = estimate_tokens(&summary);
    let kept_tokens: usize = pinned.iter().chain(recent.iter()).map(|m| m.tokens).sum();
    let tokens_after = summary_tokens + kept_tokens;

    Ok(SummaryResult {
        summary,
        summarized_count: to_summarize.len(),
        kept_count: pinned.len() + recent.len(),
        tokens_before,
        tokens_after,
    })
}

fn build_summarize_prompt(messages: &[&ConversationMessage]) -> String {
    let mut prompt = String::from("Summarize this conversation:\n\n");
    for msg in messages {
        let preview: String = msg.content.chars().take(500).collect();
        prompt.push_str(&format!("{}: {}\n", msg.role, preview));
    }
    prompt.push_str("\nFocus on: key decisions, files created/modified, important context, errors and solutions.\nKeep under 200 words.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            role: role.to_string(),
            content: content.to_string(),
            tokens: estimate_tokens(content),
        }
    }

    #[test]
    fn pins_error_messages() {
        let m = msg("system", "Error: Failed to connect");
        assert_eq!(message_priority(&m), MessagePriority::Pinned);
    }

    #[test]
    fn pins_confirmations() {
        let m = msg("user", "/yes proceed with changes");
        assert_eq!(message_priority(&m), MessagePriority::Pinned);
    }

    #[test]
    fn pins_file_changes() {
        let m = msg(
            "assistant",
            "### FILE: src/main.rs\n### ACTION: create\n```\nfn main(){}\n```",
        );
        assert_eq!(message_priority(&m), MessagePriority::Pinned);
    }

    #[test]
    fn normal_for_regular_messages() {
        assert_eq!(
            message_priority(&msg("user", "hello")),
            MessagePriority::Normal
        );
        assert_eq!(
            message_priority(&msg("assistant", "here is the answer")),
            MessagePriority::Normal
        );
    }

    #[test]
    fn needs_summarization_under_threshold() {
        let config = SummarizerConfig {
            min_messages_for_summary: 5,
            ..Default::default()
        };
        let history: Vec<ConversationMessage> = (0..10)
            .map(|i| msg("user", &format!("Message {}", i)))
            .collect();
        // Each ~2 tokens, 10 messages = ~20 tokens < 80% of 4096
        assert!(!needs_summarization(&history, 4096, &config));
    }

    #[test]
    fn needs_summarization_over_threshold() {
        let config = SummarizerConfig {
            trigger_threshold_percent: 50,
            min_messages_for_summary: 5,
            ..Default::default()
        };
        let history: Vec<ConversationMessage> = (0..150)
            .map(|i| msg("user", &format!("Message {}: {}", i, "x".repeat(100))))
            .collect();
        assert!(needs_summarization(&history, 4096, &config));
    }

    #[test]
    fn needs_summarization_too_few_messages() {
        let config = SummarizerConfig {
            min_messages_for_summary: 100,
            ..Default::default()
        };
        let history: Vec<ConversationMessage> = (0..10)
            .map(|i| msg("user", &format!("{}", "x".repeat(1000))))
            .collect();
        assert!(!needs_summarization(&history, 4096, &config));
    }

    #[test]
    fn summarize_prompt_built_correctly() {
        let messages = vec![msg("user", "create a file"), msg("assistant", "done")];
        let refs: Vec<&ConversationMessage> = messages.iter().collect();
        let prompt = build_summarize_prompt(&refs);
        assert!(prompt.contains("Summarize this conversation"));
        assert!(prompt.contains("user: create a file"));
        assert!(prompt.contains("assistant: done"));
    }
}
