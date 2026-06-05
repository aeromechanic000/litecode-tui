pub mod chat;
pub mod model;

use crate::config::Config;
use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;

/// Manages the KV cache context handle for `/api/generate`-based streaming.
///
/// Ollama returns a `context` array (numeric handle) with each `/api/generate`
/// response. Passing it back on the next request enables prefix-matched KV
/// cache reuse, avoiding redundant computation.
pub struct ContextManager {
    context_handle: Option<Vec<i64>>,
    total_prompt_tokens: usize,
    last_prompt_eval_count: Option<usize>,
    last_eval_count: Option<usize>,
    last_model: Option<String>,
    static_prefix_hash: Option<u64>,
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            context_handle: None,
            total_prompt_tokens: 0,
            last_prompt_eval_count: None,
            last_eval_count: None,
            last_model: None,
            static_prefix_hash: None,
        }
    }

    /// Get the context handle only if it was produced by the given model.
    /// Returns None if the handle is from a different model (incompatible cache).
    pub fn context_handle_for_model(&self, model: &str) -> Option<&Vec<i64>> {
        if self.last_model.as_deref() == Some(model) {
            self.context_handle.as_ref()
        } else {
            None
        }
    }

    /// Update state from a completed `/api/generate` response.
    /// Clears the handle if the model changed (different model = incompatible cache).
    pub fn update_from_response(
        &mut self,
        context: Vec<i64>,
        prompt_eval_count: Option<usize>,
        eval_count: Option<usize>,
        total_prompt_tokens: usize,
        model: &str,
    ) {
        if self.last_model.as_deref() != Some(model) {
            self.context_handle = None;
        }
        self.context_handle = Some(context);
        self.last_prompt_eval_count = prompt_eval_count;
        self.last_eval_count = eval_count;
        self.total_prompt_tokens = total_prompt_tokens;
        self.last_model = Some(model.to_string());
    }

    /// Clear all context state (equivalent to starting a new session).
    pub fn clear(&mut self) {
        self.context_handle = None;
        self.total_prompt_tokens = 0;
        self.last_prompt_eval_count = None;
        self.last_eval_count = None;
        self.last_model = None;
        self.static_prefix_hash = None;
    }

    /// Record the expected static prefix hash for KV cache validation.
    pub fn set_static_prefix_hash(&mut self, hash: u64) {
        if let Some(prev) = self.static_prefix_hash {
            if prev != hash {
                tracing::warn!(
                    "KV cache: static prefix hash changed ({:016x} → {:016x}), expect cache miss",
                    prev,
                    hash
                );
            }
        }
        self.static_prefix_hash = Some(hash);
    }

    #[allow(dead_code)]
    pub fn static_prefix_hash(&self) -> Option<u64> {
        self.static_prefix_hash
    }

    /// Estimated KV cache hit rate as a percentage (0.0 – 100.0).
    /// Returns None if no eval stats are available yet.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let eval = self.last_prompt_eval_count?;
        let total = self.total_prompt_tokens;
        if total == 0 {
            return Some(0.0);
        }
        let hit_tokens = total.saturating_sub(eval);
        Some(hit_tokens as f64 / total as f64 * 100.0)
    }

    /// Whether the context is at or beyond the model's context window.
    #[allow(dead_code)]
    pub fn is_context_full(&self, context_window: u64) -> bool {
        self.total_prompt_tokens >= context_window as usize
    }

    /// Context usage as a percentage of the context window.
    pub fn context_usage_percent(&self, context_window: u64) -> f64 {
        if context_window == 0 {
            return 0.0;
        }
        (self.total_prompt_tokens as f64 / context_window as f64 * 100.0).min(100.0)
    }

    #[allow(dead_code)]
    pub fn total_prompt_tokens(&self) -> usize {
        self.total_prompt_tokens
    }

    #[allow(dead_code)]
    pub fn last_prompt_eval_count(&self) -> Option<usize> {
        self.last_prompt_eval_count
    }

    #[allow(dead_code)]
    pub fn last_eval_count(&self) -> Option<usize> {
        self.last_eval_count
    }

    /// Set total prompt tokens without a context handle (for tracking when no /api/generate response)
    pub fn set_total_prompt_tokens(&mut self, tokens: usize) {
        self.total_prompt_tokens = tokens;
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct OllamaClient {
    endpoint: String,
    #[allow(dead_code)]
    timeout: Duration,
    http: Client,
    num_ctx: u64,
}

impl OllamaClient {
    pub fn new(config: &Config) -> Result<Self> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(config.connect_timeout))
            .timeout(Duration::from_secs(600))
            .build()
            .context("Creating HTTP client")?;
        Ok(Self {
            endpoint: config.ollama_endpoint.trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(config.connect_timeout),
            http,
            num_ctx: config.context_window_limit,
        })
    }

    #[allow(dead_code)]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Build an HTTP client suitable for streaming (no overall deadline, only connect timeout).
    pub fn streaming_http_client(connect_timeout: Duration) -> Result<Client> {
        Client::builder()
            .connect_timeout(connect_timeout)
            // No .timeout() — streams can run indefinitely; per-chunk read timeout
            // is handled by the OS TCP stack (typically ~60s between chunks).
            .build()
            .context("Creating streaming HTTP client")
    }

    pub async fn ping(&self) -> Result<()> {
        let url = format!("{}/api/tags", self.endpoint);
        let resp = self
            .http
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .with_context(|| format!("Connecting to Ollama at {}", self.endpoint))?;
        if !resp.status().is_success() {
            anyhow::bail!("Ollama returned status {}", resp.status());
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn list_models(&self) -> Result<Vec<model::ModelInfo>> {
        let url = format!("{}/api/tags", self.endpoint);
        let resp = self
            .http
            .get(&url)
            .timeout(self.timeout)
            .send()
            .await
            .context("Fetching model list from Ollama")?;

        if !resp.status().is_success() {
            anyhow::bail!("Ollama returned status {}", resp.status());
        }

        let body: serde_json::Value = resp.json().await?;
        let models = body
            .get("models")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();

        let mut result = Vec::new();
        for m in models {
            let name = m
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let size = m.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            let quantization_level = m
                .get("details")
                .and_then(|d| d.get("quantization_level"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let family = m
                .get("details")
                .and_then(|d| d.get("family"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let parameter_count = model::estimate_parameters(&name);
            let size_class = model::classify_model(parameter_count);
            let context_window = model::estimate_context_window(&name);

            result.push(model::ModelInfo {
                name,
                size,
                parameter_count,
                quantization_level,
                family,
                size_class,
                context_window,
            });
        }

        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    /// Count tokens using Ollama's `/api/tokenize` endpoint.
    /// Returns the actual token count from the model's tokenizer.
    #[allow(dead_code)]
    pub async fn tokenize(&self, model: &str, text: &str) -> Result<usize> {
        let url = format!("{}/api/tokenize", self.endpoint);
        let body = serde_json::json!({
            "model": model,
            "prompt": text
        });
        let resp = self
            .http
            .post(&url)
            .timeout(std::time::Duration::from_secs(30))
            .json(&body)
            .send()
            .await
            .context("Calling Ollama /api/tokenize")?;

        if !resp.status().is_success() {
            anyhow::bail!("Tokenize request failed with status {}", resp.status());
        }

        let json: serde_json::Value = resp.json().await?;
        let count = json
            .get("tokens")
            .and_then(|t| t.as_array())
            .map(|arr| arr.len())
            .or_else(|| json.get("count").and_then(|c| c.as_u64()).map(|n| n as usize))
            .ok_or_else(|| anyhow::anyhow!("Invalid tokenize response: no tokens array"))?;

        Ok(count)
    }

    /// Count tokens exactly via `/api/tokenize`, falling back to heuristic estimation.
    #[allow(dead_code)]
    pub async fn count_tokens(&self, model: &str, text: &str) -> usize {
        match self.tokenize(model, text).await {
            Ok(count) => count,
            Err(e) => {
                tracing::debug!("tokenize failed, using estimation: {}", e);
                crate::util::text::estimate_tokens(text)
            }
        }
    }

    /// Warm up and pin a model in Ollama's VRAM/RAM using `keep_alive=-1`.
    /// Sends a minimal generate request that causes Ollama to load the model
    /// and keep it resident indefinitely (until explicitly unloaded or server restart).
    pub async fn warmup_model(&self, model: &str) -> Result<()> {
        let url = format!("{}/api/generate", self.endpoint);
        let body = serde_json::json!({
            "model": model,
            "prompt": "warmup init",
            "keep_alive": -1,
            "stream": false
        });
        let resp = self
            .http
            .post(&url)
            .timeout(Duration::from_secs(120))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Warming up model '{}' at {}", model, self.endpoint))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Warmup failed for model '{}': {} — {}",
                model,
                status,
                text
            );
        }

        tracing::info!("model '{}' warmed up and pinned (keep_alive=-1)", model);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            ollama_endpoint: "http://localhost:11434".into(),
            connect_timeout: 5,
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn ping_connection_refused() {
        let config = Config {
            ollama_endpoint: "http://localhost:19999".into(),
            ..test_config()
        };
        let client = OllamaClient::new(&config).unwrap();
        let result = client.ping().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_models_connection_refused() {
        let config = Config {
            ollama_endpoint: "http://localhost:19999".into(),
            ..test_config()
        };
        let client = OllamaClient::new(&config).unwrap();
        let result = client.list_models().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn client_constructs_with_valid_config() {
        let config = test_config();
        let client = OllamaClient::new(&config);
        assert!(client.is_ok());
        assert_eq!(client.unwrap().endpoint(), "http://localhost:11434");
    }

    #[test]
    fn context_manager_new_has_no_handle() {
        let cm = ContextManager::new();
        assert!(cm.context_handle_for_model("any").is_none());
        assert!(cm.cache_hit_rate().is_none());
        assert_eq!(cm.context_usage_percent(4096), 0.0);
    }

    #[test]
    fn context_manager_update_and_hit_rate() {
        let mut cm = ContextManager::new();
        cm.update_from_response(vec![1, 2, 3], Some(64), Some(128), 1024, "test:7b");
        assert!(cm.context_handle_for_model("test:7b").is_some());
        assert!(cm.context_handle_for_model("other:4b").is_none()); // model mismatch
        let rate = cm.cache_hit_rate().unwrap();
        assert!((rate - 93.75).abs() < 0.01); // (1024-64)/1024 * 100
    }

    #[test]
    fn context_manager_clear_resets() {
        let mut cm = ContextManager::new();
        cm.update_from_response(vec![1, 2, 3], Some(10), Some(50), 500, "test:7b");
        cm.clear();
        assert!(cm.context_handle_for_model("test:7b").is_none());
        assert!(cm.cache_hit_rate().is_none());
    }

    #[test]
    fn context_manager_usage_percent() {
        let mut cm = ContextManager::new();
        cm.update_from_response(vec![1], None, None, 2048, "test:7b");
        let pct = cm.context_usage_percent(4096);
        assert!((pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn context_manager_full() {
        let mut cm = ContextManager::new();
        cm.update_from_response(vec![1], None, None, 4096, "test:7b");
        assert!(cm.is_context_full(4096));
        assert!(!cm.is_context_full(8192));
    }
}
