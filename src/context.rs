use crate::agent::summarizer::{
    message_priority, needs_summarization, summarize, MessagePriority, SummarizerConfig,
};
use crate::app::ConversationMessage;
use crate::ollama::model::estimate_context_window;
use crate::util::text::estimate_tokens;

/// Sum of all message tokens in the history.
pub fn estimate_history_tokens(history: &[ConversationMessage]) -> usize {
    history.iter().map(|m| m.tokens).sum()
}

/// Check if history exceeds 90% of the context window and compact if needed.
/// Simple truncation: keeps the most recent messages that fit within 60% of
/// the window, discarding older ones.
pub fn maybe_compact(history: &mut Vec<ConversationMessage>, core_model: &str) {
    let context_window = estimate_context_window(core_model) as usize;
    let threshold = (context_window as f64 * 0.90) as usize;
    let total = estimate_history_tokens(history);

    if total <= threshold || history.is_empty() {
        return;
    }

    // Keep recent messages that fit within 60% of context window
    let keep_budget = (context_window as f64 * 0.60) as usize;
    let mut kept_tokens = 0;
    let split_point = history
        .iter()
        .rev()
        .position(|m| {
            if kept_tokens + m.tokens > keep_budget {
                return true;
            }
            kept_tokens += m.tokens;
            false
        })
        .map(|p| history.len().saturating_sub(p))
        .unwrap_or(0);

    if split_point > 0 {
        history.drain(0..split_point);
    }
}

/// Compact history using LLM-powered summarization instead of simple truncation.
#[allow(dead_code)]
///
/// Splits history into pinned (always kept), recent N (kept verbatim), and older (summarized).
/// Replaces the older messages with a single system message containing the summary.
/// Re-injects project instructions and current goal after summarization to prevent drift.
/// Falls back to `maybe_compact` if summarization fails.
pub async fn compact_with_summary(
    history: &mut Vec<ConversationMessage>,
    core_model: &str,
    client: &crate::ollama::OllamaClient,
    exec_model: &str,
    config: &SummarizerConfig,
    project_instructions: Option<&str>,
    current_goal: Option<&str>,
) -> bool {
    let context_window = estimate_context_window(core_model) as usize;

    if !needs_summarization(history, context_window, config) {
        return false;
    }

    match summarize(client, exec_model, history, config).await {
        Ok(result) => {
            if result.summarized_count == 0 {
                return false;
            }

            // Rebuild history: pinned messages + summary system message + recent messages
            let mut pinned_msgs: Vec<ConversationMessage> = Vec::new();
            let mut recent_msgs: Vec<ConversationMessage> = Vec::new();

            let keep_count = config.keep_recent_count.saturating_sub(
                history
                    .iter()
                    .filter(|m| message_priority(m) == MessagePriority::Pinned)
                    .count(),
            );

            let recent_start = history.len().saturating_sub(keep_count);
            for (i, msg) in history.drain(..).enumerate() {
                if message_priority(&msg) == MessagePriority::Pinned {
                    pinned_msgs.push(msg);
                } else if i >= recent_start {
                    recent_msgs.push(msg);
                }
                // else: this message was summarized, discard
            }

            // Build the new history
            let summary_msg = ConversationMessage {
                role: "system".to_string(),
                content: format!(
                    "[Conversation Summary]\n{}\n\nThis summarizes earlier conversation context.",
                    result.summary
                ),
                tokens: estimate_tokens(&result.summary),
            };

            history.clear();
            history.push(summary_msg);

            // Re-inject project instructions and current goal after summarization
            // This prevents instruction drift in small models
            let mut reinject_parts = Vec::new();
            if let Some(instructions) = project_instructions {
                if !instructions.is_empty() {
                    reinject_parts.push(format!("[Project Guidelines]\n{}", instructions));
                }
            }
            if let Some(goal) = current_goal {
                if !goal.is_empty() {
                    reinject_parts.push(format!("[Current Objective]\n{}", goal));
                }
            }
            if !reinject_parts.is_empty() {
                let reinject_content = reinject_parts.join("\n\n");
                history.push(ConversationMessage {
                    role: "system".to_string(),
                    content: reinject_content.clone(),
                    tokens: estimate_tokens(&reinject_content),
                });
            }

            history.extend(pinned_msgs);
            history.extend(recent_msgs);

            tracing::info!(
                "Compacted history: {} messages → {} (summarized {})",
                result.summarized_count + result.kept_count + 1,
                history.len(),
                result.summarized_count,
            );
            true
        }
        Err(e) => {
            tracing::warn!("Summarization failed, falling back to truncation: {}", e);
            maybe_compact(history, core_model);
            false
        }
    }
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
    fn maybe_compact_does_nothing_when_under_threshold() {
        let mut history = vec![msg("user", "hello")];
        maybe_compact(&mut history, "qwen3:8b");
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn maybe_compact_truncates_when_over_threshold() {
        // qwen3:2b → 4096 context. Fill with ~6000 tokens to exceed 90% threshold (3686).
        let mut history = Vec::new();
        for i in 0..400 {
            history.push(msg(
                "user",
                &format!(
                    "message number {:03} with enough text to use tokens and fill context",
                    i
                ),
            ));
        }
        // Each ~16 tokens, 400 * 16 = 6400 tokens > 3686
        let before = history.len();
        maybe_compact(&mut history, "qwen3:2b");
        assert!(
            history.len() < before,
            "history was not compacted: {} vs {}",
            history.len(),
            before
        );
        assert!(history.last().unwrap().content.contains("399"));
    }

    #[test]
    fn estimate_history_tokens_works() {
        let history = vec![
            msg("user", "hello world"),   // ~3 tokens
            msg("assistant", "hi there"), // ~2 tokens
        ];
        let total = estimate_history_tokens(&history);
        assert!(total > 0);
    }

    #[test]
    fn compact_with_summary_preserves_pinned() {
        // Create history with enough tokens to exceed a small threshold
        let mut history = Vec::new();
        for i in 0..100 {
            history.push(msg("user", &format!("Message {}: {}", i, "x".repeat(50))));
        }
        // This should be pinned
        history.push(msg(
            "assistant",
            "### FILE: src/main.rs\n### ACTION: create\n```\nfn main(){}\n```",
        ));
        history.push(msg("user", "next request"));

        let config = SummarizerConfig {
            keep_recent_count: 4,
            trigger_threshold_percent: 50,
            min_messages_for_summary: 5,
        };
        let context_window = 512; // Small to force summarization

        assert!(needs_summarization(&history, context_window, &config));
    }

    #[test]
    fn compact_with_summary_under_threshold_not_needed() {
        let history = vec![msg("user", "hello"), msg("assistant", "hi")];
        let config = SummarizerConfig::default();
        assert!(!needs_summarization(&history, 4096, &config));
    }
}
