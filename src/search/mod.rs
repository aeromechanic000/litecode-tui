pub mod cache;

use crate::config::Config;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;

/// Overall timeout for a single search-backend HTTP request.
///
/// Kept short on purpose: a regionally-blocked backend (e.g. DuckDuckGo in
/// mainland China) can otherwise stall for 60s+ and hang the whole turn. A
/// short timeout lets a blocked backend fail fast so the fallback chain can
/// move on.
const BACKEND_TIMEOUT_SECS: u64 = 15;
/// Timeout for the TCP connect phase alone (separate from the overall timeout).
const BACKEND_CONNECT_TIMEOUT_SECS: u64 = 8;
/// Max results parsed from a single backend's first page.
const MAX_RESULTS_PER_BACKEND: usize = 5;

/// A browser-like User-Agent; without it search engines often return an
/// anti-bot / empty page.
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub struct SearchEngine {
    http: reqwest::Client,
    cache: cache::SearchCache,
    enabled: bool,
    backend: Backend,
    auto_region: bool,
    searxng_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Bing,
    Baidu,
    DuckDuckGo,
    Searxng,
}

impl Backend {
    /// Parse a config string into a backend. Unknown values log a warning and
    /// fall back to Bing rather than failing the config load.
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "bing" => Backend::Bing,
            "baidu" => Backend::Baidu,
            "duckduckgo" | "ddg" => Backend::DuckDuckGo,
            "searxng" => Backend::Searxng,
            other => {
                tracing::warn!("unknown web_search_backend {:?}, defaulting to bing", other);
                Backend::Bing
            }
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Backend::Bing => "bing",
            Backend::Baidu => "baidu",
            Backend::DuckDuckGo => "duckduckgo",
            Backend::Searxng => "searxng",
        }
    }

    /// SearXNG is only usable when a self-hosted URL is configured; the others
    /// are always attempted (they simply error if regionally blocked).
    fn is_available(self, searxng_configured: bool) -> bool {
        !matches!(self, Backend::Searxng) || searxng_configured
    }
}

/// Returned by [`SearchEngine::search`] when every backend in the chain was
/// unreachable (network timeout / connection refused / non-2xx). This is
/// distinct from a successful zero-result query: a regional network block
/// (e.g. mainland China blocking DuckDuckGo) surfaces here, so callers can tell
/// "blocked" from "nothing matched" and hint the user toward a reachable backend.
#[derive(Debug)]
pub struct AllBackendsUnreachable {
    /// Backend identifiers tried, in order.
    pub tried: Vec<&'static str>,
}

impl std::fmt::Display for AllBackendsUnreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "all search backends unreachable (tried: {})",
            self.tried.join(", ")
        )
    }
}

impl std::error::Error for AllBackendsUnreachable {}

impl SearchEngine {
    pub fn new(config: &Config) -> Self {
        let cache_dir = Config::cache_dir()
            .map(|d| d.join("web_search"))
            .unwrap_or_else(|_| PathBuf::from("/tmp/litepilot_search_cache"));
        // A bounded client is essential: without per-request timeouts a blocked
        // backend hangs the turn (observed 60s+ stalls). Fall back to a plain
        // client only if the builder itself errors.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(BACKEND_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(BACKEND_CONNECT_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            http,
            cache: cache::SearchCache::new(cache_dir, config.search_cache_valid_days),
            enabled: config.enable_free_web_search,
            backend: Backend::parse(&config.web_search_backend),
            auto_region: config.auto_switch_network_region,
            searxng_url: normalize_searxng_url(&config.searxng_url),
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

    /// Ordered backends to try for a query.
    ///
    /// The configured backend is always first (strict preference — see
    /// `web_search_backend`). When `auto_switch_network_region` is on,
    /// region-reachable fallbacks follow so search still works behind a regional
    /// block; SearXNG appears only when a URL is configured. With
    /// `auto_switch_network_region` off, only the configured backend is used.
    fn chain(&self) -> Vec<Backend> {
        let searxng_on = self.searxng_url.is_some();
        let mut chain: Vec<Backend> = Vec::new();
        if self.backend.is_available(searxng_on) {
            chain.push(self.backend);
        }
        if self.auto_region {
            // Order: SearXNG (self-hosted → bypasses all blocks) → Bing
            // (reachable in CN, broad coverage) → Baidu (CN-local) → DuckDuckGo
            // (blocked in CN, fine elsewhere).
            for b in [
                Backend::Searxng,
                Backend::Bing,
                Backend::Baidu,
                Backend::DuckDuckGo,
            ] {
                if b.is_available(searxng_on) && !chain.contains(&b) {
                    chain.push(b);
                }
            }
        }
        chain
    }

    pub async fn search(
        &self,
        query: &str,
        max_tokens: usize,
    ) -> std::result::Result<Vec<SearchResult>, AllBackendsUnreachable> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        // Check cache first.
        if let Some(cached) = self.cache.get(query) {
            return Ok(cached);
        }

        let chain = self.chain();
        if chain.is_empty() {
            // No usable backend (e.g. backend=searxng with no URL and
            // auto_switch_network_region off). Nothing to try.
            return Ok(Vec::new());
        }

        let mut tried: Vec<&'static str> = Vec::new();
        let mut reachable_but_empty = false;
        for backend in chain {
            tried.push(backend.as_str());
            match self.fetch_backend(backend, query).await {
                Ok(results) if !results.is_empty() => {
                    let truncated = self.truncate_results(&results, max_tokens);
                    // Only cache genuine non-empty hits. A blocked/empty
                    // response is never cached — otherwise a regional block
                    // would poison the cache for `search_cache_valid_days`.
                    self.cache.set(query, &truncated);
                    return Ok(truncated);
                }
                Ok(_) => {
                    // Reachable, but zero parsed results — genuine no-match so
                    // far. Remember and try the next backend for coverage.
                    reachable_but_empty = true;
                }
                Err(_) => {
                    // Unreachable: timeout / connection / non-2xx. Move on.
                }
            }
        }

        if reachable_but_empty {
            Ok(Vec::new())
        } else {
            Err(AllBackendsUnreachable { tried })
        }
    }

    async fn fetch_backend(&self, backend: Backend, query: &str) -> Result<Vec<SearchResult>> {
        match backend {
            Backend::Bing => self.fetch_bing(query).await,
            Backend::Baidu => self.fetch_baidu(query).await,
            Backend::DuckDuckGo => self.fetch_duckduckgo(query).await,
            Backend::Searxng => self.fetch_searxng(query).await,
        }
    }

    // ---- per-backend fetchers ----

    async fn fetch_bing(&self, query: &str) -> Result<Vec<SearchResult>> {
        // Route by query script: Latin queries hit the en-US market index (best
        // for international / current events), CJK queries hit zh-CN (best for
        // China-local content). Aligning query language, cc, and Accept-Language
        // yields the best recall on Bing.
        let latin = is_latin_query(query);
        let (cc, setlang, accept_lang) = if latin {
            ("en", "en-US", "en-US,en;q=0.9")
        } else {
            ("CN", "zh-CN", "zh-CN,zh;q=0.9,en;q=0.8")
        };
        let url = format!(
            "https://www.bing.com/search?q={}&cc={}&setlang={}",
            urlencode::encode(query),
            cc,
            setlang
        );
        let resp = self
            .http
            .get(&url)
            .header("User-Agent", BROWSER_UA)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", accept_lang)
            .send()
            .await
            .context("Bing request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("Bing returned status {}", resp.status());
        }
        let html = resp.text().await?;
        Ok(parse_bing_results(&html))
    }

    async fn fetch_baidu(&self, query: &str) -> Result<Vec<SearchResult>> {
        let url = format!("https://www.baidu.com/s?wd={}", urlencode::encode(query));
        let resp = self
            .http
            .get(&url)
            .header("User-Agent", BROWSER_UA)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await
            .context("Baidu request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("Baidu returned status {}", resp.status());
        }
        let html = resp.text().await?;
        Ok(parse_baidu_results(&html))
    }

    async fn fetch_duckduckgo(&self, query: &str) -> Result<Vec<SearchResult>> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencode::encode(query)
        );
        let resp = self
            .http
            .get(&url)
            .header("User-Agent", BROWSER_UA)
            .send()
            .await
            .context("DuckDuckGo request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("DuckDuckGo returned status {}", resp.status());
        }
        let html = resp.text().await?;
        Ok(parse_ddg_results(&html))
    }

    async fn fetch_searxng(&self, query: &str) -> Result<Vec<SearchResult>> {
        let base = match &self.searxng_url {
            Some(u) => u.clone(),
            None => anyhow::bail!("SearXNG URL not configured"),
        };
        // Prefer the JSON endpoint; many instances support format=json. Fall
        // back to HTML parsing for instances that disable it.
        let resp = self
            .http
            .get(&base)
            .query(&[("q", query), ("format", "json"), ("pageno", "1")])
            .header("User-Agent", BROWSER_UA)
            .send()
            .await
            .context("SearXNG request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("SearXNG returned status {}", resp.status());
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let body = resp.text().await?;
        if ctype.contains("json") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                return Ok(parse_searxng_json(&json));
            }
            // claimed JSON but failed to parse → fall through to HTML
        }
        Ok(parse_searxng_html(&body))
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub body: String,
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

// ------------------------------------------------------------
// Query classification
// ------------------------------------------------------------

/// True when the query is mostly Latin/ASCII letters (English etc.) — Latin
/// letters make up ≥60% of non-whitespace characters.
///
/// Used to route Bing to the en-US market index for international/current-events
/// queries (the zh-CN index covers those poorly), while CJK queries stay on
/// zh-CN where China-local content is best.
fn is_latin_query(q: &str) -> bool {
    let mut letters = 0usize;
    let mut nonspace = 0usize;
    for c in q.chars() {
        if c.is_whitespace() {
            continue;
        }
        nonspace += 1;
        if c.is_ascii_alphabetic() {
            letters += 1;
        }
    }
    nonspace > 0 && letters * 10 >= nonspace * 6
}

// ------------------------------------------------------------
// Result-page parsers (HTML / JSON → SearchResult)
// ------------------------------------------------------------

/// `<li class="b_algo">` blocks: `<h2><a href>` title + `<p class="b_lineclamp">` snippet.
fn parse_bing_results(html: &str) -> Vec<SearchResult> {
    let tag_re = regex::Regex::new(r"<[^>]+>").unwrap();
    let ws_re = regex::Regex::new(r"\s+").unwrap();
    let block_re = regex::Regex::new(r#"<li\b[^>]*class="[^"]*\bb_algo\b[^"]*""#).unwrap();
    let link_re =
        regex::Regex::new(r#"(?is)<h2[^>]*>.*?<a[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let snip_re =
        regex::Regex::new(r#"(?is)<p\b[^>]*class="[^"]*b_lineclamp[^"]*"[^>]*>(.*?)</p>"#).unwrap();

    let strip = |s: &str| -> String {
        let s = tag_re.replace_all(s, " ");
        let s = unescape_entities(&s);
        ws_re.replace_all(s.trim(), " ").to_string()
    };

    let mut out = Vec::new();
    for block in block_re.split(html).skip(1) {
        let cap = match link_re.captures(block) {
            Some(c) => c,
            None => continue,
        };
        let link = decode_bing_url(cap.get(1).unwrap().as_str());
        if link.is_empty() {
            continue;
        }
        let title = strip(cap.get(2).unwrap().as_str());
        let snippet = snip_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| strip(m.as_str()))
            .unwrap_or_default();
        if title.is_empty() && snippet.is_empty() {
            continue;
        }
        out.push(SearchResult {
            title: if title.is_empty() { link.clone() } else { title },
            url: link,
            snippet: snippet.clone(),
            body: snippet,
        });
        if out.len() >= MAX_RESULTS_PER_BACKEND {
            break;
        }
    }
    out
}

/// Bing wraps some organic results in a `bing.com/ck/a?...&u=...` redirect with
/// the real URL percent-encoded in `u=`. Most organic hrefs are already real
/// URLs; this only decodes the `ck/a` form. Returns "" for non-HTTP hrefs.
fn decode_bing_url(href: &str) -> String {
    let href = href.trim();
    if href.contains("bing.com/ck/a") {
        if let Ok(re) = regex::Regex::new(r#"[?&]u=([^&]+)"#) {
            if let Some(m) = re.captures(href) {
                let cand = percent_decode(m.get(1).unwrap().as_str());
                if cand.starts_with("http") {
                    return cand;
                }
            }
        }
    }
    if regex::Regex::new(r"(?i)^https?://")
        .unwrap()
        .is_match(href)
    {
        href.to_string()
    } else {
        String::new()
    }
}

/// `<div class="...c-container...">` blocks: `<h3><a href>` title + the
/// container's `mu="..."` real URL (else Baidu's redirect link) + snippet.
fn parse_baidu_results(html: &str) -> Vec<SearchResult> {
    let tag_re = regex::Regex::new(r"<[^>]+>").unwrap();
    let ws_re = regex::Regex::new(r"\s+").unwrap();
    let block_re = regex::Regex::new(r#"<div\b[^>]*class="[^"]*\bc-container\b[^"]*""#).unwrap();
    let link_re =
        regex::Regex::new(r#"(?is)<h3[^>]*>.*?<a[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let mu_re = regex::Regex::new(r#"\bmu="([^"]+)""#).unwrap();
    let snip_re = regex::Regex::new(
        r#"(?is)<span\b[^>]*class="[^"]*content-right[^"]*"[^>]*>(.*?)</span>"#,
    )
    .unwrap();
    let snip2_re =
        regex::Regex::new(r#"(?is)<div\b[^>]*class="[^"]*c-abstract[^"]*"[^>]*>(.*?)</div>"#)
            .unwrap();

    let strip = |s: &str| -> String {
        let s = tag_re.replace_all(s, " ");
        let s = unescape_entities(&s);
        ws_re.replace_all(s.trim(), " ").to_string()
    };

    let mut out = Vec::new();
    for block in block_re.split(html).skip(1) {
        let cap = match link_re.captures(block) {
            Some(c) => c,
            None => continue,
        };
        let title = strip(cap.get(2).unwrap().as_str());
        // Prefer the container's mu="..." real URL; else keep Baidu's redirect
        // link (still clickable, resolves to the real site via Baidu).
        let real = mu_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .filter(|u| u.starts_with("http"))
            .unwrap_or_else(|| cap.get(1).unwrap().as_str().to_string());
        let snippet = snip_re
            .captures(block)
            .or_else(|| snip2_re.captures(block))
            .and_then(|c| c.get(1))
            .map(|m| strip(m.as_str()))
            .unwrap_or_default();
        if title.is_empty() && snippet.is_empty() {
            continue;
        }
        out.push(SearchResult {
            title: if title.is_empty() { real.clone() } else { title },
            url: real,
            snippet: snippet.clone(),
            body: snippet,
        });
        if out.len() >= MAX_RESULTS_PER_BACKEND {
            break;
        }
    }
    out
}

/// DuckDuckGo HTML results page: `class="result__a"` links + `result__snippet`.
fn parse_ddg_results(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let link_re = regex::Regex::new(r#"(?is)class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
        .unwrap_or_else(|_| regex::Regex::new(r".").unwrap());
    let snippet_re = regex::Regex::new(r#"(?is)class="result__snippet"[^>]*>(.*?)</[at]"#)
        .unwrap_or_else(|_| regex::Regex::new(r".").unwrap());
    let tag_re = regex::Regex::new(r"<[^>]*>").unwrap();

    let links: Vec<_> = link_re
        .captures_iter(html)
        .filter_map(|c| {
            let url = c.get(1)?.as_str().to_string();
            let title = c.get(2)?.as_str().to_string();
            let title = regex::Regex::new(r"<[^>]*>")
                .ok()?
                .replace_all(&title, "")
                .to_string();
            Some((url, title))
        })
        .take(MAX_RESULTS_PER_BACKEND)
        .collect();

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

/// SearXNG JSON `results[]`: each item's url/title/content.
fn parse_searxng_json(json: &serde_json::Value) -> Vec<SearchResult> {
    let mut out = Vec::new();
    let results = match json.get("results").and_then(|v| v.as_array()) {
        Some(r) => r,
        None => return out,
    };
    for x in results {
        let url = x.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if url.is_empty() {
            continue;
        }
        let title = x
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(url)
            .to_string();
        let snippet = x.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
        out.push(SearchResult {
            title,
            url: url.to_string(),
            snippet: snippet.clone(),
            body: snippet,
        });
        if out.len() >= MAX_RESULTS_PER_BACKEND {
            break;
        }
    }
    out
}

/// SearXNG HTML results page: `<article class="result">` blocks.
fn parse_searxng_html(html: &str) -> Vec<SearchResult> {
    let tag_re = regex::Regex::new(r"<[^>]+>").unwrap();
    let ws_re = regex::Regex::new(r"\s+").unwrap();
    let block_re = regex::Regex::new(r#"<article\b[^>]*class="[^"]*\bresult\b[^"]*""#).unwrap();
    let link_re =
        regex::Regex::new(r#"(?is)<h3[^>]*>.*?<a[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let snip_re =
        regex::Regex::new(r#"(?is)<p\b[^>]*class="[^"]*\bcontent\b[^"]*"[^>]*>(.*?)</p>"#).unwrap();

    let strip = |s: &str| -> String {
        let s = tag_re.replace_all(s, " ");
        let s = unescape_entities(&s);
        ws_re.replace_all(s.trim(), " ").to_string()
    };

    let mut out = Vec::new();
    for block in block_re.split(html).skip(1) {
        let cap = match link_re.captures(block) {
            Some(c) => c,
            None => continue,
        };
        let link = cap.get(1).unwrap().as_str().to_string();
        if !link.starts_with("http") {
            continue;
        }
        let title = strip(cap.get(2).unwrap().as_str());
        let snippet = snip_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| strip(m.as_str()))
            .unwrap_or_default();
        if title.is_empty() && snippet.is_empty() {
            continue;
        }
        out.push(SearchResult {
            title: if title.is_empty() { link.clone() } else { title },
            url: link,
            snippet: snippet.clone(),
            body: snippet,
        });
        if out.len() >= MAX_RESULTS_PER_BACKEND {
            break;
        }
    }
    out
}

// ------------------------------------------------------------
// Small helpers
// ------------------------------------------------------------

fn unescape_entities(s: &str) -> String {
    // Decode `&amp;` LAST: replacing it first would turn `&amp;lt;` into `&lt;`
    // and then a later pass would decode that into `<` (double-decode). Doing the
    // named entities first and `&amp;` last leaves a literal `&amp;lt;` as `&lt;`.
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Percent-decode `%XX` escapes into raw bytes, then interpret as UTF-8 (lossy).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Normalize a user-supplied SearXNG URL into a `…/search` endpoint with a
/// scheme. Empty / whitespace-only → None.
fn normalize_searxng_url(raw: &Option<String>) -> Option<String> {
    let s = raw.as_ref()?.trim();
    if s.is_empty() {
        return None;
    }
    let with_scheme = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("http://{}", s)
    };
    let trimmed = with_scheme.trim_end_matches('/');
    if trimmed.ends_with("/search") {
        Some(trimmed.to_string())
    } else {
        Some(format!("{}/search", trimmed))
    }
}

// Simple URL encoding (avoid adding another dependency).
mod urlencode {
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

    fn engine_with(backend: &str, auto_region: bool, searxng: Option<&str>) -> SearchEngine {
        let config = Config {
            web_search_backend: backend.to_string(),
            auto_switch_network_region: auto_region,
            searxng_url: searxng.map(|s| s.to_string()),
            ..Config::default()
        };
        SearchEngine::new(&config)
    }

    #[test]
    fn url_encoding() {
        assert_eq!(urlencode::encode("hello world"), "hello+world");
        assert_eq!(urlencode::encode("rust & code"), "rust+%26+code");
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

    // ---- Backend selection / chain ----

    #[test]
    fn backend_parse_known_and_unknown() {
        assert_eq!(Backend::parse("Bing"), Backend::Bing);
        assert_eq!(Backend::parse(" baidu "), Backend::Baidu);
        assert_eq!(Backend::parse("ddg"), Backend::DuckDuckGo);
        assert_eq!(Backend::parse("searxng"), Backend::Searxng);
        // Unknown → graceful default to Bing.
        assert_eq!(Backend::parse("google"), Backend::Bing);
    }

    #[test]
    fn chain_strict_when_auto_region_off() {
        // Only the configured backend; no fallbacks.
        let engine = engine_with("bing", false, None);
        assert_eq!(engine.chain(), vec![Backend::Bing]);

        let engine = engine_with("duckduckgo", false, None);
        assert_eq!(engine.chain(), vec![Backend::DuckDuckGo]);
    }

    #[test]
    fn chain_configured_first_then_region_fallbacks() {
        // Configured Bing first, then Baidu, then DuckDuckGo (SearXNG skipped:
        // no URL configured). DuckDuckGo stays last because it's blocked in CN.
        let engine = engine_with("bing", true, None);
        assert_eq!(
            engine.chain(),
            vec![Backend::Bing, Backend::Baidu, Backend::DuckDuckGo]
        );
    }

    #[test]
    fn chain_ddg_configured_falls_through_to_bing() {
        // The CN failure scenario: DDG configured but blocked → Bing/Baidu must
        // follow so search still works out of the box.
        let engine = engine_with("duckduckgo", true, None);
        assert_eq!(
            engine.chain(),
            vec![Backend::DuckDuckGo, Backend::Bing, Backend::Baidu]
        );
    }

    #[test]
    fn chain_includes_searxng_only_when_url_set() {
        // SearXNG appears (right after the configured backend) only with a URL.
        let engine = engine_with("bing", true, Some("localhost:8080"));
        assert_eq!(
            engine.chain(),
            vec![Backend::Bing, Backend::Searxng, Backend::Baidu, Backend::DuckDuckGo]
        );
    }

    #[test]
    fn chain_empty_when_searxng_only_but_no_url_and_no_auto() {
        // backend=searxng, no URL, auto off → nothing usable.
        let engine = engine_with("searxng", false, None);
        assert!(engine.chain().is_empty());
    }

    // ---- URL / query helpers ----

    #[test]
    fn normalize_searxng_url_variants() {
        assert_eq!(
            normalize_searxng_url(&Some("localhost:8080".into())),
            Some("http://localhost:8080/search".into())
        );
        assert_eq!(
            normalize_searxng_url(&Some("https://s.example/".into())),
            Some("https://s.example/search".into())
        );
        assert_eq!(
            normalize_searxng_url(&Some("http://h/search".into())),
            Some("http://h/search".into())
        );
        assert_eq!(normalize_searxng_url(&None), None);
        assert_eq!(normalize_searxng_url(&Some("   ".into())), None);
    }

    #[test]
    fn is_latin_query_classification() {
        assert!(is_latin_query("rust tokio tutorial"));
        assert!(is_latin_query("2026 World Cup final"));
        assert!(!is_latin_query("周末采购生鲜食材"));
        assert!(!is_latin_query("2026世界杯决赛"));
        assert!(!is_latin_query("")); // no non-space chars → false
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a%26b"), "a&b");
        assert_eq!(percent_decode("no-escapes"), "no-escapes");
    }

    // ---- parsers (offline, fixture HTML) ----

    #[test]
    fn parse_bing_results_from_fixture() {
        let html = r#"
        <ol>
          <li class="b_algo">
            <h2><a href="https://example.com/rust">Rust Programming Language</a></h2>
            <p class="b_lineclamp4">A systems language focusing on memory safety.</p>
          </li>
          <li class="b_algo">
            <h2><a href="https://www.bing.com/ck/a?lt=0&u=https%3A%2F%2Fplain.example%2Fx">Via Redirect</a></h2>
            <p class="b_lineclamp1">Snippet two</p>
          </li>
          <li class="b_algo">
            <h2><a href="javascript:void(0)">Bad</a></h2>
            <p class="b_lineclamp1">No usable link</p>
          </li>
        </ol>"#;
        let results = parse_bing_results(html);
        // First: direct link, title + snippet.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://example.com/rust");
        assert_eq!(results[0].title, "Rust Programming Language");
        assert!(results[0].snippet.contains("memory safety"));
        // Second: ck/a redirect with a plain percent-encoded u= decoded to the
        // real URL. (Bing's common base64 u= form is left as the redirect link
        // — neither this code nor the reference decodes base64.)
        assert_eq!(results[1].url, "https://plain.example/x");
        // The javascript: href (non-http) result is dropped.
    }

    #[test]
    fn parse_searxng_json_from_fixture() {
        let json = serde_json::json!({
            "results": [
                {"url": "https://a.example", "title": "Alpha", "content": "first"},
                {"url": "", "title": "Dropped", "content": "no url"},
                {"url": "https://b.example", "title": "Beta", "content": "second"}
            ]
        });
        let results = parse_searxng_json(&json);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://a.example");
        assert_eq!(results[1].title, "Beta");
    }

    #[test]
    fn parse_baidu_results_from_fixture() {
        let html = r#"
        <div class="result c-container new-pmd" mu="https://real.cn/page">
          <h3><a href="https://www.baidu.com/link?url=xyz">真实标题</a></h3>
          <div class="c-abstract">摘要内容 here</div>
        </div>"#;
        let results = parse_baidu_results(html);
        assert_eq!(results.len(), 1);
        // mu="..." real URL preferred over the baidu redirect link.
        assert_eq!(results[0].url, "https://real.cn/page");
        assert_eq!(results[0].title, "真实标题");
        assert!(results[0].snippet.contains("摘要内容"));
    }

    #[test]
    fn unescape_entities_order() {
        // &amp; decodes first so a literal "&amp;lt;" stays "&lt;", not "<".
        assert_eq!(unescape_entities("&amp;lt;"), "&lt;");
        assert_eq!(unescape_entities("a &amp; b &lt; c"), "a & b < c");
    }
}
