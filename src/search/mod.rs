pub mod cache;

use crate::config::Config;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct SearchEngine {
    http: reqwest::Client,
    cache: cache::SearchCache,
    enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub body: String,
}

impl SearchEngine {
    pub fn new(config: &Config) -> Self {
        let cache_dir = Config::cache_dir()
            .map(|d| d.join("web_search"))
            .unwrap_or_else(|_| PathBuf::from("/tmp/litepilot_search_cache"));

        Self {
            http: reqwest::Client::new(),
            cache: cache::SearchCache::new(cache_dir, config.search_cache_valid_days),
            enabled: config.enable_free_web_search,
        }
    }

    #[allow(dead_code)]
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn search(&self, query: &str, max_tokens: usize) -> Result<Vec<SearchResult>> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        // Check cache first
        if let Some(cached) = self.cache.get(query) {
            return Ok(cached);
        }

        // Try multiple free search sources
        let results = self.fetch_search_results(query).await?;

        // Truncate results to fit token budget
        let truncated = self.truncate_results(&results, max_tokens);

        // Cache the results
        self.cache.set(query, &truncated);

        Ok(truncated)
    }

    async fn fetch_search_results(&self, query: &str) -> Result<Vec<SearchResult>> {
        // Use DuckDuckGo HTML search as a free source (no API key needed)
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );

        let resp = self
            .http
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
            .send()
            .await
            .context("Search request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!("Search returned status {}", resp.status());
        }

        let html = resp.text().await?;
        Ok(self.parse_ddg_results(&html))
    }

    fn parse_ddg_results(&self, html: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        // Simple regex-based extraction from DDG HTML
        let link_re = regex::Regex::new(r#"class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
            .unwrap_or(regex::Regex::new(r".").unwrap());
        let snippet_re = regex::Regex::new(r#"class="result__snippet"[^>]*>(.*?)</[at]"#)
            .unwrap_or(regex::Regex::new(r".").unwrap());

        let links: Vec<_> = link_re
            .captures_iter(html)
            .filter_map(|c| {
                let url = c.get(1)?.as_str().to_string();
                let title = c.get(2)?.as_str().to_string();
                // Strip HTML tags
                let title = regex::Regex::new(r"<[^>]*>")
                    .ok()?
                    .replace_all(&title, "")
                    .to_string();
                Some((url, title))
            })
            .take(5)
            .collect();

        let tag_re = regex::Regex::new(r"<[^>]*>").unwrap();
        for (i, (url, title)) in links.iter().enumerate() {
            let snippet = snippet_re
                .captures_iter(html)
                .nth(i)
                .and_then(|c| c.get(1))
                .map(|m| tag_re.replace_all(m.as_str(), "").to_string())
                .unwrap_or_default();

            results.push(SearchResult {
                title: title.clone(),
                url: url.clone(),
                snippet: snippet.clone(),
                body: snippet,
            });
        }

        results
    }

    fn truncate_results(&self, results: &[SearchResult], max_tokens: usize) -> Vec<SearchResult> {
        let mut output = Vec::new();
        let mut token_count = 0;

        for result in results {
            let result_tokens = crate::util::text::estimate_tokens(&result.body);
            if token_count + result_tokens > max_tokens {
                break;
            }
            token_count += result_tokens;
            output.push(result.clone());
        }

        output
    }
}

/// Format search results as context text for LLM consumption.
pub fn format_search_context(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut ctx = String::from("Web search results:\n\n");
    for (i, r) in results.iter().enumerate() {
        ctx.push_str(&format!(
            "[{}] {} — {}\n{}\n\n",
            i + 1,
            r.title,
            r.url,
            r.snippet
        ));
    }
    ctx.push_str("Use the above search results to inform your response.\n\n");
    ctx
}

// Simple URL encoding (avoid adding another dependency)
mod urlencoding {
    pub fn encode(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                ' ' => "+".to_string(),
                _ => format!("%{:02X}", c as u8),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encoding() {
        assert_eq!(urlencoding::encode("hello world"), "hello+world");
        assert_eq!(urlencoding::encode("rust & code"), "rust+%26+code");
    }

    #[test]
    fn truncate_results() {
        let engine = SearchEngine::new(&Config::default());
        let results = vec![
            SearchResult {
                title: "test1".into(),
                url: "http://a".into(),
                snippet: "a".repeat(1000),
                body: "a".repeat(1000),
            },
            SearchResult {
                title: "test2".into(),
                url: "http://b".into(),
                snippet: "b".repeat(1000),
                body: "b".repeat(1000),
            },
        ];
        let truncated = engine.truncate_results(&results, 100);
        assert!(truncated.len() <= 2);
    }

    #[test]
    fn search_disabled_returns_empty() {
        let mut engine = SearchEngine::new(&Config::default());
        engine.set_enabled(false);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(engine.search("test", 100)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn enabled_toggle() {
        let mut engine = SearchEngine::new(&Config::default());
        assert!(engine.is_enabled());
        engine.set_enabled(false);
        assert!(!engine.is_enabled());
    }
}
