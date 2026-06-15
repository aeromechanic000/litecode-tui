use crate::ollama::OllamaClient;
use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    think: bool,
    options: ChatOptions,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
struct ChatOptions {
    /// Context window size in tokens
    num_ctx: u64,
    /// -1 = unlimited output tokens
    num_predict: i32,
}

/// A tool call Ollama emitted in the native `message.tool_calls` array.
///
/// When `/api/chat` is given a `tools` field, models that support native function
/// calling populate `message.tool_calls` instead of writing tool calls into
/// `message.content`. Without reading this, those calls are silently dropped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NativeToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    /// Tool calls Ollama returned via `message.tool_calls`. Empty when the model
    /// emitted text-format tool calls inside `content` instead.
    pub tool_calls: Vec<NativeToolCall>,
    #[allow(dead_code)]
    pub model: String,
}

/// Extract native tool calls from an Ollama `message` JSON object.
///
/// Each entry of `message.tool_calls` has the shape `{"function": {"name", "arguments"}}`.
/// In streaming, `arguments` may arrive as a partial JSON string across chunks; we
/// take the latest non-empty value per index, attempting to parse partials.
fn extract_tool_calls_from_message(msg: &serde_json::Value) -> Vec<NativeToolCall> {
    let mut accum: Vec<NativeToolCall> = Vec::new();
    let arr = match msg.get("tool_calls").and_then(|t| t.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    for (idx, tc) in arr.iter().enumerate() {
        let func = match tc.get("function") {
            Some(f) => f,
            None => continue,
        };
        let name = func
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let arguments = match func.get("arguments") {
            Some(serde_json::Value::String(s)) => {
                serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
            }
            Some(other) => other.clone(),
            None => serde_json::Value::Object(serde_json::Map::new()),
        };
        while accum.len() <= idx {
            accum.push(NativeToolCall {
                name: String::new(),
                arguments: serde_json::Value::Object(serde_json::Map::new()),
            });
        }
        if !name.is_empty() {
            accum[idx].name = name;
        }
        if !arguments.is_null() {
            accum[idx].arguments = arguments;
        }
    }
    accum.into_iter().filter(|tc| !tc.name.is_empty()).collect()
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    #[allow(dead_code)]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

impl OllamaClient {
    pub async fn chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        _think: bool,
    ) -> Result<ChatResponse> {
        self.chat_with_tools(model, messages, &[]).await
    }

    /// Chat with optional tool definitions for agent loop (non-streaming).
    #[allow(dead_code)]
    pub async fn chat_with_tools(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: &[serde_json::Value],
    ) -> Result<ChatResponse> {
        let url = format!("{}/api/chat", self.endpoint);
        let _think = messages.iter().any(|m| m.role == "system");
        let body = ChatRequest {
            model: model.to_string(),
            messages,
            stream: false,
            think: false,
            options: ChatOptions {
                num_ctx: self.num_ctx,
                num_predict: -1,
            },
            tools: tools.to_vec(),
        };

        let resp = self
            .http
            .post(&url)
            .timeout(std::time::Duration::from_secs(300))
            .json(&body)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Ollama at {} did not respond (is it running?) for model '{}'",
                    url, model
                )
            })?;

        if resp.status() == StatusCode::NOT_FOUND {
            anyhow::bail!(
                "Ollama returned 404 for model '{}' — it may have been removed, try re-pulling",
                model
            );
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Ollama returned error {} for model '{}': {}",
                status,
                model,
                text
            );
        }

        let raw: serde_json::Value = resp.json().await?;
        let msg = raw.get("message").cloned().unwrap_or(serde_json::Value::Null);
        let content = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let tool_calls = extract_tool_calls_from_message(&msg);
        let resp_model = raw
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(model)
            .to_string();

        Ok(ChatResponse {
            content,
            tool_calls,
            model: resp_model,
        })
    }

    /// Streaming variant of `chat_with_tools` that emits text chunks as they arrive.
    ///
    /// Calls `on_chunk` for each text fragment received from the streaming `/api/chat`
    /// endpoint. Returns the full assembled response once the stream completes.
    pub async fn chat_with_tools_streaming(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: &[serde_json::Value],
        mut on_chunk: impl FnMut(&str),
    ) -> Result<ChatResponse> {
        let url = format!("{}/api/chat", self.endpoint);
        let body = ChatRequest {
            model: model.to_string(),
            messages,
            stream: true,
            think: false,
            options: ChatOptions {
                num_ctx: self.num_ctx,
                num_predict: -1,
            },
            tools: tools.to_vec(),
        };

        // Use a client without overall timeout for streaming
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .context("Creating streaming HTTP client for chat")?;

        let resp = http
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Ollama at {} did not respond (is it running?) for model '{}'",
                    url, model
                )
            })?;

        if resp.status() == StatusCode::NOT_FOUND {
            anyhow::bail!(
                "Ollama returned 404 for model '{}' — it may have been removed, try re-pulling",
                model
            );
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Ollama returned error {} for model '{}': {}",
                status,
                model,
                text
            );
        }

        // Process the NDJSON stream
        let mut stream = resp.bytes_stream();
        let mut full_content = String::new();
        let mut tool_calls: Vec<NativeToolCall> = Vec::new();
        let mut buffer = String::new();
        let mut resp_model = model.to_string();
        let read_timeout = std::time::Duration::from_secs(300);

        loop {
            let chunk_opt = match tokio::time::timeout(read_timeout, stream.next()).await {
                Ok(opt) => opt,
                Err(_) => {
                    anyhow::bail!("Chat stream timed out (no data for 300s)");
                }
            };

            let bytes = match chunk_opt {
                Some(Ok(b)) => b,
                Some(Err(e)) => {
                    tracing::warn!("chat stream read error: {}", e);
                    break;
                }
                None => break,
            };

            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(val) => {
                        if let Some(m) = val.get("model").and_then(|m| m.as_str()) {
                            resp_model = m.to_string();
                        }

                        let message = val.get("message");

                        let chunk_text = message
                            .and_then(|m| m.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("");

                        if !chunk_text.is_empty() {
                            on_chunk(chunk_text);
                            full_content.push_str(chunk_text);
                        }

                        // Merge native tool_calls across chunks (index-based).
                        if let Some(msg) = message {
                            let chunk_tcs = extract_tool_calls_from_message(msg);
                            // Reconcile by index: latest non-empty wins per slot.
                            for (idx, tc) in chunk_tcs.into_iter().enumerate() {
                                while tool_calls.len() <= idx {
                                    tool_calls.push(NativeToolCall {
                                        name: String::new(),
                                        arguments: serde_json::Value::Object(
                                            serde_json::Map::new(),
                                        ),
                                    });
                                }
                                tool_calls[idx] = tc;
                            }
                        }

                        let done = val.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
                        if done {
                            let tool_calls = tool_calls
                                .into_iter()
                                .filter(|tc| !tc.name.is_empty())
                                .collect();
                            return Ok(ChatResponse {
                                content: full_content,
                                tool_calls,
                                model: resp_model,
                            });
                        }
                    }
                    Err(e) => {
                        tracing::debug!("JSON parse error in chat stream: {} in line: {}", e, line);
                    }
                }
            }
        }

        let tool_calls = tool_calls
            .into_iter()
            .filter(|tc| !tc.name.is_empty())
            .collect();
        Ok(ChatResponse {
            content: full_content,
            tool_calls,
            model: resp_model,
        })
    }
}

// ---------------------------------------------------------------------------
// /api/generate types — for KV cache context management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<Vec<i64>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    options: ChatOptions,
}

/// A streaming chunk from `/api/generate`.
#[derive(Debug, Clone)]
pub struct GenerateChunk {
    pub response: String,
    pub done: bool,
    pub done_reason: Option<String>,
    #[allow(dead_code)]
    pub model: String,
    /// Only present on the final chunk (done=true)
    pub context: Option<Vec<i64>>,
    /// Tokens re-computed (cache miss) — only on final chunk
    pub prompt_eval_count: Option<usize>,
    /// Tokens generated — only on final chunk
    pub eval_count: Option<usize>,
}

/// A non-streaming response from `/api/generate`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GenerateResponse {
    pub response: String,
    pub model: String,
    pub context: Vec<i64>,
    pub prompt_eval_count: usize,
    pub eval_count: usize,
}

impl OllamaClient {
    /// Streaming generation via `/api/generate` with optional KV cache context handle.
    ///
    /// The `system_prompt` is passed in the `system` field and `prompt` contains the
    /// concatenated conversation text. When `context_handle` is `Some`, Ollama will
    /// attempt prefix matching against the cached KV tensors.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_stream(
        http: reqwest::Client,
        endpoint: String,
        model: String,
        system_prompt: Option<String>,
        prompt: String,
        context_handle: Option<Vec<i64>>,
        num_ctx: u64,
        cancel: watch::Receiver<bool>,
    ) -> impl futures::Stream<Item = Result<GenerateChunk>> {
        let url = format!("{}/api/generate", endpoint);
        let body = GenerateRequest {
            model: model.clone(),
            prompt,
            context: context_handle,
            stream: true,
            system: system_prompt,
            options: ChatOptions {
                num_ctx,
                num_predict: -1,
            },
        };

        async_stream::stream! {
            let resp = match http.post(&url).json(&body).send().await {
                Ok(r) => r,
                Err(e) => {
                    yield Err(anyhow::anyhow!("Generate stream connect failed: {}", e));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                yield Err(anyhow::anyhow!("Ollama generate stream error: {}", status));
                return;
            }

            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();
            let read_timeout = std::time::Duration::from_secs(300);

            let mut total_bytes: usize = 0;
            const MAX_CONTENT_BYTES: usize = 10 * 1024 * 1024;
            let start = std::time::Instant::now();
            const MAX_DURATION: std::time::Duration = std::time::Duration::from_secs(30 * 60);
            let mut error_count: usize = 0;
            const MAX_ERRORS: usize = 5;

            loop {
                let cancelled = *cancel.borrow();
                if cancelled {
                    yield Ok(GenerateChunk {
                        response: String::new(),
                        done: true,
                        done_reason: Some("cancel".into()),
                        model: model.clone(),
                        context: None,
                        prompt_eval_count: None,
                        eval_count: None,
                    });
                    return;
                }

                if start.elapsed() > MAX_DURATION {
                    yield Err(anyhow::anyhow!(
                        "Generate stream exceeded maximum duration (30 min)"
                    ));
                    return;
                }

                let chunk_result = match tokio::time::timeout(read_timeout, stream.next()).await {
                    Ok(Some(result)) => result,
                    Ok(None) => {
                        yield Ok(GenerateChunk {
                            response: String::new(),
                            done: true,
                            done_reason: Some("stream_end".into()),
                            model: model.clone(),
                            context: None,
                            prompt_eval_count: None,
                            eval_count: None,
                        });
                        return;
                    }
                    Err(_) => {
                        yield Err(anyhow::anyhow!("Generate stream timed out (no data for 300s)"));
                        return;
                    }
                };

                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(_) => {
                        error_count += 1;
                        if error_count >= MAX_ERRORS {
                            yield Err(anyhow::anyhow!(
                                "Too many generate stream errors ({})",
                                error_count
                            ));
                            return;
                        }
                        continue;
                    }
                };

                total_bytes += bytes.len();
                if total_bytes > MAX_CONTENT_BYTES {
                    yield Err(anyhow::anyhow!(
                        "Generate stream exceeded maximum content size (10 MB)"
                    ));
                    return;
                }

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    buffer = buffer[pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(val) => {
                            let response = val.get("response")
                                .and_then(|r| r.as_str())
                                .unwrap_or("")
                                .to_string();
                            let done = val.get("done")
                                .and_then(|d| d.as_bool())
                                .unwrap_or(false);
                            let done_reason = val.get("done_reason")
                                .and_then(|d| d.as_str())
                                .map(|s| s.to_string());
                            let resp_model = val.get("model")
                                .and_then(|m| m.as_str())
                                .unwrap_or(&model)
                                .to_string();

                            // Extract eval stats (present on final chunk)
                            let context = if done {
                                val.get("context")
                                    .and_then(|c| c.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_i64())
                                            .collect()
                                    })
                            } else {
                                None
                            };
                            let prompt_eval_count = if done {
                                val.get("prompt_eval_count").and_then(|v| v.as_u64()).map(|v| v as usize)
                            } else {
                                None
                            };
                            let eval_count = if done {
                                val.get("eval_count").and_then(|v| v.as_u64()).map(|v| v as usize)
                            } else {
                                None
                            };

                            yield Ok(GenerateChunk {
                                response,
                                done,
                                done_reason,
                                model: resp_model,
                                context,
                                prompt_eval_count,
                                eval_count,
                            });

                            if done {
                                return;
                            }
                        }
                        Err(e) => {
                            error_count += 1;
                            if error_count >= MAX_ERRORS {
                                yield Err(anyhow::anyhow!(
                                    "JSON parse error in generate: {} in line: {}",
                                    e, line
                                ));
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_message_constructors() {
        let sys = ChatMessage::system("You are a helpful assistant.");
        assert_eq!(sys.role, "system");
        let user = ChatMessage::user("Write a hello world.");
        assert_eq!(user.role, "user");
        let asst = ChatMessage::assistant("Here is the code:");
        assert_eq!(asst.role, "assistant");
    }

    #[test]
    fn chat_request_serialization() {
        let req = ChatRequest {
            model: "qwen3:4b".to_string(),
            messages: vec![ChatMessage::user("test")],
            stream: true,
            think: true,
            options: ChatOptions {
                num_ctx: 262144,
                num_predict: -1,
            },
            tools: vec![],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"stream\":true"));
        assert!(json.contains("\"model\":\"qwen3:4b\""));
        assert!(json.contains("\"num_predict\":-1"));
    }

    #[tokio::test]
    async fn chat_model_not_found() {
        let config = crate::config::Config {
            ollama_endpoint: "http://localhost:19999".into(),
            ..crate::config::Config::default()
        };
        let client = OllamaClient::new(&config).unwrap();
        let result = client
            .chat("nonexistent", vec![ChatMessage::user("hi")], true)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn extract_tool_calls_complete_object() {
        let msg: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "function": {
                        "name": "web_reader",
                        "arguments": {"url": "https://example.com", "max_length": 1000}
                    }
                }
            ]
        });
        let calls = extract_tool_calls_from_message(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web_reader");
        assert_eq!(
            calls[0].arguments["url"].as_str().unwrap(),
            "https://example.com"
        );
        assert_eq!(calls[0].arguments["max_length"].as_i64().unwrap(), 1000);
    }

    #[test]
    fn extract_tool_calls_empty_when_absent() {
        let msg: serde_json::Value =
            serde_json::json!({"role": "assistant", "content": "hello"});
        assert!(extract_tool_calls_from_message(&msg).is_empty());
    }

    #[test]
    fn extract_tool_calls_partial_string_arguments() {
        // Some stream chunks deliver arguments as a JSON string fragment.
        let msg: serde_json::Value = serde_json::json!({
            "tool_calls": [
                {"function": {"name": "write_file", "arguments": "{\"path\":\"a.txt\"}"}}
            ]
        });
        let calls = extract_tool_calls_from_message(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert_eq!(calls[0].arguments["path"].as_str().unwrap(), "a.txt");
    }

    #[test]
    fn extract_tool_calls_multiple() {
        let msg: serde_json::Value = serde_json::json!({
            "tool_calls": [
                {"function": {"name": "read_file", "arguments": {"path": "a"}}},
                {"function": {"name": "read_file", "arguments": {"path": "b"}}}
            ]
        });
        let calls = extract_tool_calls_from_message(&msg);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["path"].as_str().unwrap(), "a");
        assert_eq!(calls[1].arguments["path"].as_str().unwrap(), "b");
    }

    #[test]
    fn extract_tool_calls_skips_unnamed() {
        let msg: serde_json::Value = serde_json::json!({
            "tool_calls": [
                {"function": {"name": "", "arguments": {}}},
                {"function": {"name": "list_dir", "arguments": {"path": "."}}}
            ]
        });
        let calls = extract_tool_calls_from_message(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "list_dir");
    }

    #[test]
    fn native_tool_call_to_tool_call_round_trip() {
        let tc = NativeToolCall {
            name: "exec_shell".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        let tool_call = crate::agent::tools_parser::ToolCall {
            name: tc.name.clone(),
            call_id: "native-0".into(),
            parameters: tc.arguments.clone(),
        };
        assert_eq!(tool_call.name, "exec_shell");
        assert_eq!(tool_call.call_id, "native-0");
        assert_eq!(tool_call.parameters["command"].as_str().unwrap(), "ls");
    }
}
