use crate::tools::{Tool, ToolDef, ToolResult};
use anyhow::Result;
use std::time::Duration;

const DEFAULT_MAX_LENGTH: usize = 8000;
const SUMMARY_LENGTH: usize = 300;
const REQUEST_TIMEOUT_SECS: u64 = 30;

pub struct WebReader {
    enabled: bool,
}

impl WebReader {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            enabled: config.enable_free_web_search,
        }
    }
}

impl Tool for WebReader {
    fn execute(&self, params: serde_json::Value, call_id: String) -> Result<ToolResult> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter"))?;

        if !self.enabled {
            return Ok(ToolResult::err(
                "web_reader",
                call_id,
                "Web access is disabled. Enable it in config with enable_free_web_search = true",
            ));
        }

        let max_length = params
            .get("max_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_MAX_LENGTH as u64) as usize;

        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(fetch_page(url, max_length)) {
            Ok(output) => Ok(ToolResult::ok("web_reader", call_id, output)),
            Err(e) => Ok(ToolResult::err(
                "web_reader",
                call_id,
                format!("Failed to fetch {}: {}", url, format_error_chain(&e)),
            )),
        }
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "web_reader".into(),
            description: "Fetch and read the content of a webpage from a URL. Returns the page title, URL, summary, and full text content. Use this tool whenever the user provides a URL or asks to read a webpage.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL to fetch (e.g. 'https://example.com')" },
                    "max_length": { "type": "integer", "description": "Maximum content length in characters (default 8000)" }
                },
                "required": ["url"]
            }),
        }
    }
}

/// Format an anyhow error with its full causal chain, joined by ` → ` and
/// with exact duplicates removed. reqwest's top-level error message ("error
/// sending request for url") is unhelpful on its own — the actionable cause
/// (DNS lookup failure, TCP connect refused, TLS handshake error, HTTP/2
/// protocol error) is buried one or two levels down. Surfacing the chain lets
/// the model pick a sensible fallback (retry vs switch host vs switch tool).
fn format_error_chain(e: &anyhow::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for cause in e.chain() {
        let s = cause.to_string();
        if !parts.contains(&s) {
            parts.push(s);
        }
    }
    parts.join(" → ")
}

async fn fetch_page(url: &str, max_length: usize) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()?;

    let resp = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.5")
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }

    let html = resp.text().await?;
    Ok(extract_content(&html, url, max_length))
}

fn extract_content(html: &str, url: &str, max_length: usize) -> String {
    let title = extract_title(html);
    let text = strip_html(html);
    let text = normalize_whitespace(&text);

    let summary = if text.len() > SUMMARY_LENGTH {
        &text[..SUMMARY_LENGTH]
    } else {
        &text
    };

    let content = if text.len() > max_length {
        &text[..max_length]
    } else {
        &text
    };

    format!(
        "Title: {}\nURL: {}\nSummary: {}\n---\n{}",
        title, url, summary, content
    )
}

fn extract_title(html: &str) -> String {
    let re = regex::Regex::new(r"(?i)<title[^>]*>(.*?)</title>").unwrap();
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_else(|| "(no title)".into())
}

/// Strip HTML tags, decode common entities, collapse whitespace.
fn strip_html(html: &str) -> String {
    // Remove script and style blocks entirely
    let re_script = regex::Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let cleaned = re_script.replace_all(html, " ");
    let re_style = regex::Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap();
    let cleaned = re_style.replace_all(&cleaned, " ");
    let re_noscript = regex::Regex::new(r"(?is)<noscript[^>]*>.*?</noscript>").unwrap();
    let cleaned = re_noscript.replace_all(&cleaned, " ");

    // Remove HTML comments
    let re_comment = regex::Regex::new(r"<!--.*?-->").unwrap();
    let cleaned = re_comment.replace_all(&cleaned, " ");

    // Remove all remaining tags
    let re_tag = regex::Regex::new(r"<[^>]*>").unwrap();
    let text = re_tag.replace_all(&cleaned, " ");

    // Decode common HTML entities
    let text = text.replace("&amp;", "&");
    let text = text.replace("&lt;", "<");
    let text = text.replace("&gt;", ">");
    let text = text.replace("&quot;", "\"");
    let text = text.replace("&#39;", "'");
    let text = text.replace("&nbsp;", " ");

    text
}

fn normalize_whitespace(s: &str) -> String {
    let re = regex::Regex::new(r"\s+").unwrap();
    re.replace_all(s.trim(), " ").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_fields() {
        let config = crate::config::Config::default();
        let tool = WebReader::from_config(&config);
        let def = tool.definition();
        assert_eq!(def.name, "web_reader");
        assert!(!def.description.is_empty());
    }

    #[test]
    fn format_error_chain_walks_sources() {
        // Simulate reqwest's chain shape: top-level wraps a middle layer wraps
        // the root cause. Without chain-walking, only the top message would be
        // surfaced, hiding the actionable detail.
        let root = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "Connection refused (os error 61)");
        let mid = anyhow::Error::new(root).context("tcp connect error");
        let top = mid.context("error sending request for url (https://example.com)");
        let formatted = format_error_chain(&top);
        assert!(
            formatted.contains("error sending request"),
            "expected top-level message in chain, got: {}",
            formatted
        );
        assert!(
            formatted.contains("Connection refused"),
            "expected root cause in chain, got: {}",
            formatted
        );
        assert!(
            formatted.contains("tcp connect"),
            "expected intermediate cause in chain, got: {}",
            formatted
        );
        assert!(
            formatted.contains(" → "),
            "expected arrow separator in chain, got: {}",
            formatted
        );
    }

    #[test]
    fn format_error_chain_dedupes_repeats() {
        // When two layers produce identical messages, only show one — keeps
        // the tool_result compact for the model.
        let root = std::io::Error::new(std::io::ErrorKind::Other, "dns error");
        let mid = anyhow::Error::new(root).context("dns error");
        let top = mid.context("error sending request");
        let formatted = format_error_chain(&top);
        assert_eq!(
            formatted.matches("dns error").count(),
            1,
            "expected dedup of repeated cause, got: {}",
            formatted
        );
    }

    #[test]
    fn disabled_returns_error() {
        let config = crate::config::Config {
            enable_free_web_search: false,
            ..crate::config::Config::default()
        };
        let tool = WebReader::from_config(&config);
        let result = tool
            .execute(serde_json::json!({"url": "https://example.com"}), "c1".into())
            .unwrap();
        assert!(!result.success);
        assert!(result.output.contains("disabled"));
    }

    #[test]
    fn extract_title_basic() {
        let html = "<html><head><title>Hello World</title></head><body>content</body></html>";
        assert_eq!(extract_title(html), "Hello World");
    }

    #[test]
    fn extract_title_missing() {
        let html = "<html><body>no title</body></html>";
        assert_eq!(extract_title(html), "(no title)");
    }

    #[test]
    fn extract_title_case_insensitive() {
        let html = "<HTML><HEAD><TITLE>Test</TITLE></HEAD></HTML>";
        assert_eq!(extract_title(html), "Test");
    }

    #[test]
    fn strip_html_removes_tags() {
        let html = "<p>Hello <b>world</b></p><div>more</div>";
        let text = strip_html(html);
        assert!(!text.contains('<'));
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(text.contains("more"));
    }

    #[test]
    fn strip_html_removes_scripts() {
        let html = "<html><script>var x = 1;</script><p>content</p></html>";
        let text = strip_html(html);
        assert!(!text.contains("var x"));
        assert!(text.contains("content"));
    }

    #[test]
    fn strip_html_decodes_entities() {
        let html = "<p>a &amp; b &lt; c</p>";
        let text = strip_html(html);
        assert!(text.contains("a & b < c"));
    }

    #[test]
    fn normalize_whitespace_collapses() {
        let result = normalize_whitespace("  hello   \n  world  \n\n  ");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn extract_content_truncates() {
        let html = format!(
            "<html><head><title>Test</title></head><body>{}</body></html>",
            "x".repeat(1000)
        );
        let result = extract_content(&html, "http://test.com", 100);
        assert!(result.contains("Title: Test"));
        assert!(result.contains("URL: http://test.com"));
        assert!(result.contains("Summary:"));
        assert!(result.contains("---"));
        let content_part = result.split("---\n").nth(1).unwrap();
        assert!(content_part.len() <= 100);
    }

    #[test]
    fn missing_url_param_returns_error() {
        let config = crate::config::Config::default();
        let tool = WebReader::from_config(&config);
        let result = tool.execute(serde_json::json!({}), "c1".into());
        assert!(result.is_err());
    }
}
