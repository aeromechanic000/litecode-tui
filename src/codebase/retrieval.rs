use super::Template;
use crate::agent::prompts::{self, TEMPLATE_SELECTION_SYSTEM};
use crate::codebase::CodeBase;
use crate::config::Config;
use crate::ollama::chat::ChatMessage;
use crate::ollama::model::estimate_context_window;
use crate::ollama::OllamaClient;
use crate::util::text::{estimate_tokens, truncate_to_tokens};
use anyhow::Result;

/// Templates loaded and formatted for inclusion in agent prompts.
#[allow(dead_code)]
pub struct LoadedTemplates {
    /// Formatted template strings ready for prompt injection.
    pub refs: Vec<String>,
    /// Total estimated tokens across all loaded templates.
    pub total_tokens: usize,
    /// How many templates the model selected (before budget trimming).
    pub selected_count: usize,
}

/// Build a compact pipe-delimited catalog from all templates.
/// Format: `index|name|lang|tags|description` (one line per template).
#[allow(dead_code)]
pub fn build_catalog(templates: &[Template]) -> String {
    let mut lines = Vec::with_capacity(templates.len());
    for (i, t) in templates.iter().enumerate() {
        let lang = t.path.split('/').next().unwrap_or("unknown");
        let tags = t.tags.join(",");
        lines.push(format!(
            "{}|{}|{}|{}|{}",
            i + 1,
            t.name,
            lang,
            tags,
            t.description
        ));
    }
    lines.join("\n")
}

/// Parse the LLM's selection response into a list of valid template indices.
/// Extracts all numbers from the response and filters to valid range.
#[allow(dead_code)]
fn parse_selection(response: &str, total_templates: usize, max_select: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = response
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| {
            if s.is_empty() {
                return None;
            }
            // Parse as 1-based index, convert to 0-based
            s.parse::<usize>().ok().and_then(|n| {
                if n >= 1 && n <= total_templates {
                    Some(n - 1)
                } else {
                    None
                }
            })
        })
        .collect();

    indices.dedup();
    indices.truncate(max_select);
    indices
}

/// Use the core model to select relevant template indices from the catalog.
#[allow(dead_code)]
async fn select(
    client: &OllamaClient,
    core_model: &str,
    catalog: &str,
    user_request: &str,
    project_context: &str,
    max_select: usize,
) -> Result<Vec<usize>> {
    let truncated_context = truncate_project_context(project_context, 15);
    let prompt = format!(
        "USER REQUEST:\n{}\n\nPROJECT:\n{}\n\nCATALOG:\n{}\n\n\
         Select up to {} relevant template indices. Output ONLY comma-separated numbers.",
        user_request, truncated_context, catalog, max_select,
    );

    let messages = vec![
        ChatMessage::system(TEMPLATE_SELECTION_SYSTEM),
        ChatMessage::user(prompt),
    ];

    let response = client.chat(core_model, messages, true).await?;
    let total = catalog.lines().count();
    Ok(parse_selection(&response.content, total, max_select))
}

/// Load selected template content, respecting the token budget.
/// Returns formatted ref strings in selection order.
#[allow(dead_code)]
pub fn load_within_budget(
    templates: &[Template],
    indices: &[usize],
    max_tokens: usize,
) -> (Vec<String>, usize) {
    let mut refs = Vec::new();
    let mut used = 0;

    for &idx in indices {
        if let Some(t) = templates.get(idx) {
            let t_tokens = estimate_tokens(&t.content);
            if used + t_tokens <= max_tokens {
                refs.push(format!(
                    "### TEMPLATE: {} ###\n{}\n### END ###",
                    t.name, t.content
                ));
                used += t_tokens;
            } else {
                let remaining = max_tokens.saturating_sub(used);
                if remaining > 100 {
                    let truncated = truncate_to_tokens(&t.content, remaining);
                    refs.push(format!(
                        "### TEMPLATE: {} (truncated) ###\n{}\n### END ###",
                        t.name, truncated
                    ));
                    used += remaining;
                }
                break;
            }
        }
    }

    (refs, used)
}

/// Truncate project context to a limited number of lines for the selection call.
#[allow(dead_code)]
fn truncate_project_context(ctx: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = ctx.lines().take(max_lines).collect();
    if ctx.lines().count() > max_lines {
        format!("{}\n...", lines.join("\n"))
    } else {
        lines.join("\n")
    }
}

/// Full two-pass retrieval: select via LLM, then load content within budget.
#[allow(dead_code)]
pub async fn retrieve(
    client: &OllamaClient,
    config: &Config,
    codebase: &CodeBase,
    user_request: &str,
    project_context: &str,
) -> LoadedTemplates {
    let templates = codebase.templates();

    if templates.is_empty() || config.exec_model.is_empty() {
        return LoadedTemplates {
            refs: Vec::new(),
            total_tokens: 0,
            selected_count: 0,
        };
    }

    let context_window = estimate_context_window(&config.exec_model) as usize;

    // Calculate template budget: context window minus fixed overhead
    let request_tokens = estimate_tokens(user_request);
    let context_tokens = estimate_tokens(project_context);
    let fixed_overhead = 2500; // system prompts + formatting + response reserve
    let budget = context_window
        .saturating_sub(fixed_overhead)
        .saturating_sub(request_tokens)
        .saturating_sub(context_tokens);
    let budget = budget.min(config.max_template_context_tokens);

    if budget < 200 {
        return LoadedTemplates {
            refs: Vec::new(),
            total_tokens: 0,
            selected_count: 0,
        };
    }

    // Pass 1: Select templates via LLM
    let catalog = build_catalog(templates);
    let indices = match select(
        client,
        &config.exec_model,
        &catalog,
        user_request,
        project_context,
        config.template_max_select,
    )
    .await
    {
        Ok(indices) => indices,
        Err(e) => {
            eprintln!(
                "Template selection failed: {}, proceeding without templates",
                e
            );
            return LoadedTemplates {
                refs: Vec::new(),
                total_tokens: 0,
                selected_count: 0,
            };
        }
    };

    let selected_count = indices.len();

    if indices.is_empty() {
        return LoadedTemplates {
            refs: Vec::new(),
            total_tokens: 0,
            selected_count: 0,
        };
    }

    // Pass 2: Load content within budget
    let (refs, total_tokens) = load_within_budget(templates, &indices, budget);

    LoadedTemplates {
        refs,
        total_tokens,
        selected_count,
    }
}

/// Two-stage retrieval with semantic reranking for higher precision.
/// Stage 1: broad candidate selection via LLM (top `broad_select`).
/// Stage 2: exec_model reranks candidates by code-aware semantic relevance.
/// Falls back to `retrieve()` if reranking fails.
#[allow(dead_code)]
pub async fn retrieve_with_reranking(
    client: &OllamaClient,
    config: &Config,
    codebase: &CodeBase,
    user_request: &str,
    project_context: &str,
) -> LoadedTemplates {
    let templates = codebase.templates();

    if templates.is_empty() || config.exec_model.is_empty() {
        return LoadedTemplates {
            refs: Vec::new(),
            total_tokens: 0,
            selected_count: 0,
        };
    }

    let context_window = estimate_context_window(&config.exec_model) as usize;
    let request_tokens = estimate_tokens(user_request);
    let context_tokens = estimate_tokens(project_context);
    let fixed_overhead = 2500;
    let budget = context_window
        .saturating_sub(fixed_overhead)
        .saturating_sub(request_tokens)
        .saturating_sub(context_tokens);
    let budget = budget.min(config.max_template_context_tokens);

    if budget < 200 {
        return LoadedTemplates {
            refs: Vec::new(),
            total_tokens: 0,
            selected_count: 0,
        };
    }

    // Stage 1: Broad selection — request more candidates than needed
    let broad_select = config.template_max_select.max(10).min(templates.len());
    let catalog = build_catalog(templates);
    let broad_indices = match select(
        client,
        &config.exec_model,
        &catalog,
        user_request,
        project_context,
        broad_select,
    )
    .await
    {
        Ok(indices) => indices,
        Err(e) => {
            tracing::warn!("Reranking stage 1 failed: {}, falling back", e);
            return retrieve(client, config, codebase, user_request, project_context).await;
        }
    };

    if broad_indices.is_empty() {
        return LoadedTemplates {
            refs: Vec::new(),
            total_tokens: 0,
            selected_count: 0,
        };
    }

    // If few candidates, skip reranking (not enough to meaningfully reorder)
    if broad_indices.len() <= 3 {
        let selected_count = broad_indices.len();
        let (refs, total_tokens) = load_within_budget(templates, &broad_indices, budget);
        return LoadedTemplates {
            refs,
            total_tokens,
            selected_count,
        };
    }

    // Stage 2: Rerank with exec_model
    let reranked = match rerank(
        client,
        config,
        templates,
        &broad_indices,
        user_request,
        project_context,
    )
    .await
    {
        Ok(indices) => indices,
        Err(e) => {
            tracing::warn!("Reranking stage 2 failed: {}, using original order", e);
            broad_indices
        }
    };

    let selected_count = reranked.len();
    let (refs, total_tokens) = load_within_budget(templates, &reranked, budget);

    LoadedTemplates {
        refs,
        total_tokens,
        selected_count,
    }
}

/// Rerank candidate template indices using exec_model.
/// Sends code snippets (first 500 chars each) and asks model to order by relevance.
async fn rerank(
    client: &OllamaClient,
    config: &Config,
    templates: &[Template],
    indices: &[usize],
    user_request: &str,
    project_context: &str,
) -> Result<Vec<usize>> {
    let exec_model = &config.exec_model;
    if exec_model.is_empty() {
        return Ok(indices.to_vec());
    }

    let max_snippet_chars = 500;
    let mut candidate_lines = Vec::with_capacity(indices.len());
    for (rank, &idx) in indices.iter().enumerate() {
        if let Some(t) = templates.get(idx) {
            let snippet: String = t.content.chars().take(max_snippet_chars).collect();
            candidate_lines.push(format!(
                "{}. [{}] {} (lang: {}) — {}\n{}",
                rank + 1,
                t.name,
                t.description,
                t.path.split('/').next().unwrap_or("?"),
                t.tags.join(","),
                snippet
            ));
        }
    }

    let truncated_context = truncate_project_context(project_context, 10);
    let prompt = format!(
        "TASK:\n{}\n\nPROJECT:\n{}\n\nCANDIDATES:\n{}\n\n\
         Rank these candidates by relevance to the task. Output ONLY comma-separated indices in order of relevance (best first).",
        user_request,
        truncated_context,
        candidate_lines.join("\n\n")
    );

    let messages = vec![
        ChatMessage::system(prompts::RERANK_SYSTEM),
        ChatMessage::user(prompt),
    ];

    let response = client.chat(exec_model, messages, true).await?;
    let reranked = parse_rerank_response(&response.content, indices.len());

    if reranked.is_empty() {
        return Ok(indices.to_vec());
    }

    // Map reranked positions back to template indices
    let result: Vec<usize> = reranked
        .into_iter()
        .filter_map(|pos| {
            if pos >= 1 && pos <= indices.len() {
                indices.get(pos - 1).copied()
            } else {
                None
            }
        })
        .collect();

    if result.is_empty() {
        Ok(indices.to_vec())
    } else {
        Ok(result)
    }
}

/// Parse rerank response into 1-based position indices.
/// Extracts numbers and interprets them as candidate rank positions.
fn parse_rerank_response(text: &str, max_count: usize) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    let positions: Vec<usize> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| {
            if s.is_empty() {
                return None;
            }
            s.parse::<usize>().ok().and_then(|n| {
                if n >= 1 && n <= max_count && seen.insert(n) {
                    Some(n)
                } else {
                    None
                }
            })
        })
        .collect();

    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_template(
        name: &str,
        path: &str,
        desc: &str,
        tags: Vec<&str>,
        content: impl Into<String>,
    ) -> Template {
        let content = content.into();
        Template {
            name: name.to_string(),
            path: path.to_string(),
            description: desc.to_string(),
            scene: "development".to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            content: content.to_string(),
        }
    }

    #[test]
    fn catalog_format() {
        let templates = vec![
            make_template(
                "flask_api",
                "python/flask_api.py",
                "Flask REST API",
                vec!["python", "flask"],
                "code",
            ),
            make_template(
                "cli_app",
                "rust/cli_app.rs",
                "CLI with clap",
                vec!["rust", "cli"],
                "code",
            ),
        ];
        let catalog = build_catalog(&templates);
        assert!(catalog.contains("1|flask_api|python|python,flask|Flask REST API"));
        assert!(catalog.contains("2|cli_app|rust|rust,cli|CLI with clap"));
    }

    #[test]
    fn parse_selection_valid() {
        let indices = parse_selection("1,4,7", 10, 5);
        assert_eq!(indices, vec![0, 3, 6]);
    }

    #[test]
    fn parse_selection_with_text() {
        let indices = parse_selection("I recommend templates 2, 5, and 8 for this task.", 10, 5);
        assert_eq!(indices, vec![1, 4, 7]);
    }

    #[test]
    fn parse_selection_respects_max() {
        let indices = parse_selection("1,2,3,4,5,6,7", 10, 3);
        assert_eq!(indices.len(), 3);
    }

    #[test]
    fn parse_selection_filters_out_of_range() {
        let indices = parse_selection("0,1,11,99", 10, 10);
        assert_eq!(indices, vec![0]);
    }

    #[test]
    fn parse_selection_empty_response() {
        let indices = parse_selection("none", 10, 5);
        assert!(indices.is_empty());
    }

    #[test]
    fn load_within_budget_fits_all() {
        let templates = vec![
            make_template("a", "py/a.py", "desc", vec![], "short code"),
            make_template("b", "rs/b.rs", "desc", vec![], "more code"),
        ];
        let (refs, tokens) = load_within_budget(&templates, &[0, 1], 10000);
        assert_eq!(refs.len(), 2);
        assert!(tokens > 0);
    }

    #[test]
    fn load_within_budget_stops_at_limit() {
        let templates = vec![
            make_template("a", "py/a.py", "desc", vec![], "a".repeat(400)),
            make_template("b", "rs/b.rs", "desc", vec![], "b".repeat(400)),
            make_template("c", "go/c.go", "desc", vec![], "c".repeat(400)),
        ];
        let (refs, _) = load_within_budget(&templates, &[0, 1, 2], 150);
        assert!(refs.len() < 3);
        assert!(!refs.is_empty());
    }

    #[test]
    fn load_within_budget_truncates_last() {
        let long_content = "x".repeat(8000);
        let templates = vec![
            make_template("a", "py/a.py", "desc", vec![], "short"),
            make_template("b", "rs/b.rs", "desc", vec![], long_content),
        ];
        let (refs, tokens) = load_within_budget(&templates, &[0, 1], 200);
        assert_eq!(refs.len(), 2);
        assert!(refs[1].contains("truncated"));
        assert!(tokens <= 200);
    }

    #[test]
    fn truncate_context_short() {
        let ctx = "line1\nline2\nline3";
        assert_eq!(truncate_project_context(ctx, 5), ctx);
    }

    #[test]
    fn truncate_context_long() {
        let ctx = (1..=20)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate_project_context(&ctx, 10);
        assert!(result.ends_with("..."));
        assert!(result.lines().count() == 11); // 10 lines + "..."
    }

    #[test]
    fn parse_rerank_response_valid() {
        let positions = parse_rerank_response("3,1,5", 5);
        assert_eq!(positions, vec![3, 1, 5]);
    }

    #[test]
    fn parse_rerank_response_with_text() {
        let positions = parse_rerank_response("I recommend 2, then 4, then 1.", 5);
        assert_eq!(positions, vec![2, 4, 1]);
    }

    #[test]
    fn parse_rerank_response_filters_out_of_range() {
        let positions = parse_rerank_response("1,6,99", 5);
        assert_eq!(positions, vec![1]);
    }

    #[test]
    fn parse_rerank_response_empty() {
        let positions = parse_rerank_response("none", 5);
        assert!(positions.is_empty());
    }

    #[test]
    fn parse_rerank_response_dedup() {
        let positions = parse_rerank_response("1,2,1,3,2", 5);
        assert_eq!(positions, vec![1, 2, 3]);
    }
}
