use crate::config::Config;
use crate::tools::{Tool, ToolDef, ToolResult};
use anyhow::Result;

pub struct WebSearch {
    enabled: bool,
    cache_valid_days: u64,
}

impl WebSearch {
    pub fn from_config(config: &Config) -> Self {
        Self {
            enabled: config.enable_free_web_search,
            cache_valid_days: config.search_cache_valid_days,
        }
    }
}

impl Tool for WebSearch {
    fn execute(&self, params: serde_json::Value, call_id: String) -> Result<ToolResult> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;

        if !self.enabled {
            return Ok(ToolResult::err(
                "web_search",
                call_id,
                "Web search is disabled. Enable it in config with enable_free_web_search = true",
            ));
        }

        let rt = tokio::runtime::Runtime::new()?;
        let config = Config {
            enable_free_web_search: self.enabled,
            search_cache_valid_days: self.cache_valid_days,
            ..Config::default()
        };
        let engine = crate::search::SearchEngine::new(&config);

        match rt.block_on(engine.search(query, 2000)) {
            Ok(results) => {
                if results.is_empty() {
                    return Ok(ToolResult::ok("web_search", call_id, "No results found."));
                }
                let output = crate::search::format_search_context(&results);
                Ok(ToolResult::ok("web_search", call_id, output))
            }
            Err(e) => Ok(ToolResult::err(
                "web_search",
                call_id,
                format!(
                    "Search failed: {}. This usually means a regional network \
                     block (e.g. mainland China blocks DuckDuckGo, and a \
                     self-hosted SearXNG may be unreachable too). Set \
                     web_search_backend to a reachable backend (bing or baidu) \
                     or configure searxng_url.",
                    e
                ),
            )),
        }
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "web_search".into(),
            description: "Search the web using DuckDuckGo. Returns formatted results with titles, URLs, and snippets.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query (e.g. 'rust tokio tutorial')" }
                },
                "required": ["query"]
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_fields() {
        let config = Config::default();
        let tool = WebSearch::from_config(&config);
        let def = tool.definition();
        assert_eq!(def.name, "web_search");
        assert!(!def.description.is_empty());
    }

    #[test]
    fn search_disabled_returns_error() {
        let config = Config {
            enable_free_web_search: false,
            ..Config::default()
        };
        let tool = WebSearch::from_config(&config);
        let result = tool
            .execute(serde_json::json!({"query": "test"}), "c1".into())
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("disabled"));
    }
}
