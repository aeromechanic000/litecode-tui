use crate::app::ConversationMessage;
use crate::config::Config;
use crate::ollama::chat::ChatMessage;
use crate::ollama::OllamaClient;

const RECAP_PROMPT: &str = r#"You are a concise assistant. Given a conversation between
a user and a coding agent, summarize what was accomplished in ONE short sentence.
Focus on: files created/modified, errors fixed, features implemented.
Output ONLY the summary sentence, nothing else."#;

/// Generate a one-line recap of recent conversation messages using exec_model.
pub async fn generate_recap(
    client: &OllamaClient,
    messages: &[ConversationMessage],
    config: &Config,
) -> anyhow::Result<String> {
    // Take last N messages to keep prompt small
    let recent: Vec<&ConversationMessage> = messages.iter().rev().take(20).collect();
    let recent_refs: Vec<&ConversationMessage> = recent.into_iter().rev().collect();

    if recent_refs.is_empty() {
        return Ok("No messages to summarize.".into());
    }

    let conversation: String = recent_refs
        .iter()
        .map(|m| {
            let preview: String = m.content.chars().take(500).collect();
            format!("{}: {}", m.role, preview)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let chat_messages = vec![
        ChatMessage::system(RECAP_PROMPT),
        ChatMessage::user(format!(
            "Conversation:\n{}\n\nSummarize what was accomplished in one sentence.",
            conversation
        )),
    ];

    let model = &config.exec_model;
    let response = client.chat(model, chat_messages, false).await?;
    Ok(response.content.trim().to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn recap_prompt_is_nonempty() {
        assert!(!super::RECAP_PROMPT.is_empty());
        assert!(super::RECAP_PROMPT.contains("ONE short sentence"));
    }
}
