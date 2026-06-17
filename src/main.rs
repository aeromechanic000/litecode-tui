mod agent;
mod app;
mod approval;
mod codebase;
mod config;
mod context;
mod hooks;
mod logger;
mod lsp;
mod ollama;
mod project;
mod prompt;
mod recap;
mod router;
mod sandbox;
mod search;
mod session;
mod skills;
mod snapshot;
mod tools;
mod ui;
mod util;
mod wizard;
mod working_set;

use anyhow::Result;
use app::{AppMode, AppState, ConversationMessage};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use ui::OutputLine;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "litepilot",
    version,
    about = "Terminal AI coding assistant powered by local Ollama models"
)]
struct Args {
    /// Working directory (defaults to current directory)
    #[arg(short, long)]
    dir: Option<String>,
    /// Resume a previous session (latest, or by ID)
    #[arg(long)]
    resume: Option<Option<String>>,
    /// List saved sessions
    #[arg(long)]
    sessions: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Install crash dump handler early
    install_crash_handler();

    // Determine workspace
    let workspace = match args.dir {
        Some(ref d) => PathBuf::from(d),
        None => std::env::current_dir()?,
    };

    // Handle --sessions: list and exit
    if args.sessions {
        match session::persistence::list_sessions() {
            Ok(sessions) if sessions.is_empty() => {
                println!("No saved sessions.");
            }
            Ok(sessions) => {
                println!("Saved sessions:\n");
                for s in &sessions {
                    println!(
                        "  {}  ({} messages)  {}",
                        &s.id[..8],
                        s.message_count,
                        s.preview
                    );
                    println!("    created: {}  updated: {}", s.created_at, s.updated_at);
                }
            }
            Err(e) => eprintln!("Error listing sessions: {}", e),
        }
        return Ok(());
    }

    // Setup config — use project-local .litepilot if present, else global ~/.litepilot
    let litepilot_dir = config::Config::ensure_dirs_for(&workspace)?;
    let config = config::Config::load_for_workspace(&workspace).unwrap_or_else(|_| {
        let default = config::Config::default();
        let _ = default.save(&litepilot_dir.join("config.toml"));
        default
    });

    // Initialize file logging (guard must live until app exits)
    let _log_guard = logger::init(&litepilot_dir);

    tracing::info!(
        "LitePilot v{} starting | workspace={} | endpoint={}",
        env!("CARGO_PKG_VERSION"),
        workspace.display(),
        config.ollama_endpoint
    );
    tracing::info!(
        "models: exec={} eval={}",
        config.exec_model,
        config.effective_eval_model()
    );
    tracing::info!("mode: {}", config.default_mode);

    // Setup terminal — fullscreen for wizard
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run setup wizard — user confirms or changes Ollama URL and model selection
    let config = wizard::run(&mut terminal, config, &workspace)?;

    // Switch from fullscreen wizard to inline terminal for main app
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    drop(terminal);
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::with_options(backend, TerminalOptions {
        viewport: Viewport::Inline(3),
    })?;

    // Load session if --resume was specified
    let resume_session = match args.resume {
        Some(Some(id)) => {
            // Resume specific session by ID (prefix match)
            match session::persistence::load_session(&id) {
                Ok(s) => {
                    tracing::info!(
                        "resuming session {} ({} messages)",
                        &s.id[..8],
                        s.messages.len()
                    );
                    Some(s)
                }
                Err(e) => {
                    tracing::warn!("failed to load session {}: {}", id, e);
                    None
                }
            }
        }
        Some(None) => {
            // --resume without ID: load latest session
            match session::persistence::list_sessions() {
                Ok(sessions) if !sessions.is_empty() => {
                    let latest = &sessions[0]; // already sorted by updated_at desc
                    match session::persistence::load_session(&latest.id) {
                        Ok(s) => {
                            tracing::info!(
                                "resuming latest session {} ({} messages)",
                                &s.id[..8],
                                s.messages.len()
                            );
                            Some(s)
                        }
                        Err(_) => None,
                    }
                }
                _ => None,
            }
        }
        None => None,
    };

    // Run app with panic recovery to ensure terminal is restored
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_app(&mut terminal, config, workspace, resume_session)
    }));

    tracing::info!("session ended");

    // Restore terminal (even if panic occurred)
    disable_raw_mode()?;
    terminal.show_cursor()?;

    match result {
        Ok(inner) => inner,
        Err(panic_payload) => {
            let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!("fatal panic: {}", msg);
            eprintln!("Fatal error: {}", msg);
            std::process::exit(1);
        }
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: config::Config,
    workspace: PathBuf,
    resume_session: Option<session::Session>,
) -> Result<()> {
    let mut ui_state = ui::UiState::from_config(&config).with_workspace(workspace.clone());
    let mut app_state = AppState::new(config, workspace);

    // Resume session if provided
    if let Some(session) = resume_session {
        for msg in &session.messages {
            match msg.role.as_str() {
                "user" => ui_state.add_output(OutputLine::User(msg.content.clone())),
                "assistant" => ui_state.add_output(OutputLine::Assistant(msg.content.clone())),
                "system" => ui_state.add_output(OutputLine::System(msg.content.clone())),
                _ => {}
            }
        }
        app_state.current_session = session;
        ui_state.add_output(OutputLine::System(format!(
            "Resumed session {} ({} messages)",
            &app_state.current_session.id[..8],
            app_state.current_session.messages.len()
        )));
    }

    // Welcome banner
    let version = env!("CARGO_PKG_VERSION");
    let banner = format!(
        "version: {v:<6} \n\
        ╔═════════════════════════════════════════════════╗\n\
        ║      _      _ _       ____  _ _       _         ║\n\
        ║     | |    (_) |_ ___|  _ \\(_) | ___ | |_       ║\n\
        ║     | |    | | __/ _ \\ |_) | | |/ _ \\| __|      ║\n\
        ║     | |____| | ||  __/  __/| | | (_) | |_       ║\n\
        ║     |______|_|\\__\\___|_|   |_|_|\\___/ \\__|      ║\n\
        ║                                                 ║\n\
        ║     Ollama-powered AI assistant in Teminal      ║\n\
        ╚═════════════════════════════════════════════════╝",
        v = version
    );
    ui_state.add_output(OutputLine::System(banner));
    ui_state.add_output(OutputLine::System(
        "Shift+Tab: mode | Enter: send | Shift+Enter: newline | Ctrl+Tab: think | Ctrl+C: quit | /skills: list skills".into(),
    ));

    // Show loaded skills count
    let skill_count = app_state.skills.list().len();
    if skill_count > 0 {
        let names: Vec<&str> = app_state
            .skills
            .list()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        ui_state.add_output(OutputLine::System(format!(
            "Loaded {} skills: /{}",
            skill_count,
            names.join(", /")
        )));
    }

    let ollama_client = ollama::OllamaClient::new(&app_state.config)?;

    // Check Ollama connectivity in background
    let endpoint = app_state.config.ollama_endpoint.clone();
    let ping_result = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            let client = ollama::OllamaClient::new(&config::Config {
                ollama_endpoint: endpoint,
                ..config::Config::default()
            })?;
            client.ping().await
        })
    });

    if let Ok(Ok(_)) = ping_result.join() {
        tracing::info!("ollama ping OK: {}", app_state.config.ollama_endpoint);
        ui_state.add_output(OutputLine::System(format!(
            "Connected to Ollama at {}",
            app_state.config.ollama_endpoint
        )));
        ui_state.add_output(OutputLine::System(format!(
            "[Exec]:{} [Eval]:{}",
            app_state.config.exec_model,
            app_state.config.effective_eval_model()
        )));
    } else {
        tracing::error!("ollama ping FAILED: {}", app_state.config.ollama_endpoint);
        ui_state.add_output(OutputLine::Error(format!(
            "Cannot connect to Ollama at {}. Start Ollama first.",
            app_state.config.ollama_endpoint
        )));
    }

    // Channel for receiving LLM results from background threads
    let (result_tx, result_rx) = mpsc::channel::<agent::retry::PipelineResult>();

    // Warm up models for residency if configured
    if app_state.config.keep_exec_resident() || app_state.config.keep_eval_resident() {
        let warmup_config = app_state.config.clone();
        let warmup_tx = result_tx.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("warmup runtime error: {}", e);
                    return;
                }
            };
            let client = match ollama::OllamaClient::new(&warmup_config) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("warmup client error: {}", e);
                    return;
                }
            };

            let mut warmed = Vec::new();
            if warmup_config.keep_exec_resident() && !warmup_config.exec_model.is_empty() {
                let model = warmup_config.exec_model.clone();
                let _ = warmup_tx.send(agent::retry::PipelineResult::EvalReady {
                    evaluation: format!("[Residency] Warming up exec model: {}...", model),
                });
                match rt.block_on(client.warmup_model(&model)) {
                    Ok(()) => warmed.push(model.clone()),
                    Err(e) => {
                        let _ = warmup_tx.send(agent::retry::PipelineResult::EvalReady {
                            evaluation: format!("[Residency] Failed to warm up exec model '{}': {}", model, e),
                        });
                    }
                }
            }
            if warmup_config.keep_eval_resident() {
                let model = warmup_config.eval_model.clone();
                let _ = warmup_tx.send(agent::retry::PipelineResult::EvalReady {
                    evaluation: format!("[Residency] Warming up eval model: {}...", model),
                });
                match rt.block_on(client.warmup_model(&model)) {
                    Ok(()) => warmed.push(model.clone()),
                    Err(e) => {
                        let _ = warmup_tx.send(agent::retry::PipelineResult::EvalReady {
                            evaluation: format!("[Residency] Failed to warm up eval model '{}': {}", model, e),
                        });
                    }
                }
            }
            if !warmed.is_empty() {
                let _ = warmup_tx.send(agent::retry::PipelineResult::EvalReady {
                    evaluation: format!("[Residency] Models pinned: {}", warmed.join(", ")),
                });
            }
        });
    }

    loop {
        // Flush pending output above the viewport via insert_before()
        flush_pending_output(terminal, &mut ui_state)?;

        // Render the inline viewport (status + activity + input)
        terminal.draw(|f| ui::draw(f, &app_state, &mut ui_state))?;

        // Check for completed LLM responses (non-blocking)
        while let Ok(result) = result_rx.try_recv() {
            match result {
                agent::retry::PipelineResult::Retry(r) => {
                    app_state.is_processing = false;
                    ui_state.stop_thinking();
                    // Emit structured completion event
                    match &r {
                        agent::retry::RetryResult::Success { content, attempts } => {
                            app_state.event_sink.turn_complete(
                                &app_state.config.exec_model,
                                content.len(),
                                *attempts,
                                0,
                                0,
                            );
                        }
                        agent::retry::RetryResult::Exhausted {
                            content, attempts, ..
                        } => {
                            app_state.event_sink.turn_complete(
                                &app_state.config.exec_model,
                                content.len(),
                                *attempts,
                                0,
                                0,
                            );
                        }
                        agent::retry::RetryResult::Failed {
                            last_error,
                            attempts,
                        } => {
                            app_state
                                .event_sink
                                .emit(&hooks::HookEvent::error(last_error, "retry"));
                            let _ = attempts; // used above
                        }
                    }
                    match &r {
                        agent::retry::RetryResult::Success { content, attempts } => {
                            tracing::info!(
                                "llm response: {} bytes (attempts={})",
                                content.len(),
                                attempts
                            );
                        }
                        agent::retry::RetryResult::Exhausted {
                            content,
                            attempts,
                            corrections,
                        } => {
                            tracing::warn!(
                                "llm response exhausted retries: attempts={}, corrections={}",
                                attempts,
                                corrections.len()
                            );
                            tracing::info!("llm response content: {} bytes", content.len());
                        }
                        agent::retry::RetryResult::Failed {
                            last_error,
                            attempts,
                        } => {
                            tracing::error!(
                                "llm failed after {} attempts: {}",
                                attempts,
                                last_error
                            );
                        }
                    }
                    render_retry_result(&mut ui_state, r);
                    ui_state.add_output(OutputLine::Separator);
                }
                agent::retry::PipelineResult::AutoSuccess { changes, applied } => {
                    app_state.is_processing = false;
                    ui_state.stop_thinking();
                    tracing::info!("auto pipeline success: {} files applied", applied.len());
                    render_auto_result(&mut ui_state, &changes, &applied);
                    ui_state.add_output(OutputLine::Separator);
                }
                agent::retry::PipelineResult::AutoFailed { error } => {
                    app_state.is_processing = false;
                    ui_state.stop_thinking();
                    tracing::error!("auto pipeline failed: {}", error);
                    ui_state.add_output(OutputLine::Error(format!("Pipeline failed: {}", error)));
                    ui_state.add_output(OutputLine::Separator);
                }
                agent::retry::PipelineResult::SearchDone { count, .. } => {
                    // Intermediate status — LLM still running
                    tracing::info!("web search: {} results", count);
                    ui_state.add_output(OutputLine::System(format!(
                        "[search] Found {} result(s)",
                        count
                    )));
                    continue; // Don't drain queue yet
                }
                agent::retry::PipelineResult::StreamChunk { content } => {
                    // Remove thinking indicator on first chunk
                    ui_state.stop_thinking();
                    // Sanitize forged tool call markers before display
                    let sanitized = agent::tools_parser::sanitize_output(&content);
                    // Append chunk to streaming output — don't mark processing done
                    ui_state.append_stream_chunk(&sanitized);
                    continue;
                }
                agent::retry::PipelineResult::StreamDone { content } => {
                    app_state.is_processing = false;
                    ui_state.stop_thinking();
                    ui_state.finish_stream();
                    // Sanitize output for display
                    let content = agent::tools_parser::sanitize_output(&content);
                    tracing::info!("stream complete: {} bytes", content.len());
                    if content.len() < 100 {
                        tracing::warn!(
                            "suspiciously short response ({} bytes): {:?}",
                            content.len(),
                            content
                        );
                    } else {
                        tracing::debug!("stream content (first 500 chars): {:.500}", content);
                    }
                    // Record in conversation history
                    if !content.is_empty() {
                        let tokens = util::text::estimate_tokens(&content);
                        tracing::debug!(
                            "adding to conversation history: role=assistant, {} tokens",
                            tokens
                        );
                        app_state.conversation_history.push(ConversationMessage {
                            role: "assistant".into(),
                            content: content.clone(),
                            tokens,
                        });
                        // Auto-save session
                        app_state.current_session.add_message("assistant", &content);
                        auto_save_session(&app_state);
                        let history_before = app_state.conversation_history.len();
                        context::maybe_compact(
                            &mut app_state.conversation_history,
                            &app_state.config.exec_model,
                        );
                        if app_state.conversation_history.len() < history_before {
                            tracing::info!(
                                "conversation history compacted: {} -> {} messages",
                                history_before,
                                app_state.conversation_history.len()
                            );
                        }
                        // Check if background summarization is needed
                        if app_state.conversation_summary.is_none()
                            || app_state.conversation_history.len() > 20
                        {
                            maybe_trigger_summarization(&app_state, result_tx.clone());
                        }
                    }
                    let mode = app_state.mode;
                    // Execute bash blocks in Auto mode, show hint in Edit mode
                    if !content.is_empty() {
                        if mode == AppMode::Auto {
                            execute_bash_blocks(&app_state, &mut ui_state, &content);
                        } else if mode == AppMode::Edit {
                            let bash_blocks = parse_bash_blocks(&content);
                            if !bash_blocks.is_empty() {
                                let cmd_count: usize = bash_blocks
                                    .iter()
                                    .map(|b| b.lines().filter(|l| !l.trim().is_empty()).count())
                                    .sum();
                                ui_state.add_output(OutputLine::System(format!(
                                    "Detected {} command(s). Use /run <cmd> to execute.",
                                    cmd_count
                                )));
                            }
                        }
                    }
                    // Handle file changes per mode
                    if !content.is_empty() {
                        let changes = agent::AgentPipeline::parse_file_changes(&content);
                        if !changes.is_empty() {
                            for change in &changes {
                                app_state.working_set.touch(&change.path);
                            }
                            tracing::info!("detected {} file change(s) in response", changes.len());
                            for change in &changes {
                                tracing::info!(
                                    "  file change: {} {} ({} bytes content)",
                                    change.action,
                                    change.path.display(),
                                    change.content.len()
                                );
                            }
                            match mode {
                                AppMode::Auto => {
                                    auto_apply_changes(
                                        &mut app_state,
                                        &mut ui_state,
                                        &changes,
                                        &result_tx,
                                    );
                                    // Post-turn snapshot (non-fatal)
                                    if let Err(e) =
                                        app_state.snapshot_manager.post_turn("auto changes")
                                    {
                                        tracing::debug!("post-turn snapshot skipped: {}", e);
                                    }
                                    // End-of-turn recap for substantial auto changes
                                    if changes.len() > 2 && app_state.config.enable_recap {
                                        let history = app_state.conversation_history.clone();
                                        let config = app_state.config.clone();
                                        let tx = result_tx.clone();
                                        std::thread::spawn(move || {
                                            let rt = tokio::runtime::Runtime::new().unwrap();
                                            let result = rt.block_on(async {
                                                let client = ollama::OllamaClient::new(&config)?;
                                                recap::generate_recap(&client, &history, &config)
                                                    .await
                                            });
                                            if let Ok(summary) = result {
                                                let _ =
                                                    tx.send(agent::retry::PipelineResult::Retry(
                                                        agent::retry::RetryResult::Success {
                                                            content: format!(
                                                                "Turn recap: {}",
                                                                summary
                                                            ),
                                                            attempts: 0,
                                                        },
                                                    ));
                                            }
                                        });
                                    }
                                }
                                AppMode::Edit => {
                                    enter_file_confirmation(
                                        &mut app_state,
                                        &mut ui_state,
                                        &changes,
                                    );
                                }
                                AppMode::Plan => {
                                    ui_state.add_output(OutputLine::System(format!(
                                        "Detected {} file change(s). Switch to Edit/Auto to apply.",
                                        changes.len()
                                    )));
                                }
                            }
                        }
                    }
                    // Post-execution eval review using eval model
                    if mode != AppMode::Plan
                        && !content.is_empty()
                        && app_state.config.effective_eval_model() != app_state.config.exec_model
                    {
                        spawn_eval_review(
                            &app_state,
                            &ui_state.last_user_input,
                            &content,
                            result_tx.clone(),
                        );
                    }
                    ui_state.add_output(OutputLine::Separator);
                }
                agent::retry::PipelineResult::StreamMeta {
                    context_handle,
                    prompt_eval_count,
                    eval_count,
                    total_prompt_tokens,
                    model,
                } => {
                    // Update context manager with KV cache metadata
                    if let Some(ctx) = context_handle {
                        app_state.context_manager.update_from_response(
                            ctx,
                            prompt_eval_count,
                            eval_count,
                            total_prompt_tokens,
                            &model,
                        );
                    } else {
                        app_state.context_manager.set_total_prompt_tokens(total_prompt_tokens);
                    }

                    let context_window =
                        ollama::model::estimate_context_window(&model);

                    // Display KV cache hit rate
                    if let Some(rate) = app_state.context_manager.cache_hit_rate() {
                        let cached = total_prompt_tokens
                            .saturating_sub(prompt_eval_count.unwrap_or(0));
                        let recomputed = prompt_eval_count.unwrap_or(0);
                        let gen = eval_count.unwrap_or(0);
                        ui_state.add_output(OutputLine::System(format!(
                            "KV cache: {:.1}% hit ({} cached, {} recomputed, {} generated)",
                            rate, cached, recomputed, gen
                        )));
                    }

                    // Context overflow warnings
                    let usage_pct =
                        app_state.context_manager.context_usage_percent(context_window);
                    if usage_pct >= 100.0 {
                        ui_state.add_output(OutputLine::Error(format!(
                            "Context OVERFLOW! {:.0}% of window used ({}/{} tokens). Use /clear to reset.",
                            usage_pct,
                            total_prompt_tokens,
                            context_window
                        )));
                    } else if usage_pct >= 80.0 {
                        ui_state.add_output(OutputLine::System(format!(
                            "Context {:.0}% full ({}/{} tokens). Consider /clear to start fresh.",
                            usage_pct,
                            total_prompt_tokens,
                            context_window
                        )));
                    }
                }
                agent::retry::PipelineResult::PlanReady { plan } => {
                    tracing::info!("plan ready: {} bytes", plan.len());
                    tracing::debug!("plan content:\n{}", plan);
                    ui_state.stop_thinking();
                    if plan.starts_with("(plan unavailable") {
                        // Plan step failed — extract actual error and proceed without plan
                        let error_detail = plan
                            .strip_prefix("(plan unavailable: ")
                            .and_then(|s| s.strip_suffix(')'))
                            .unwrap_or("unknown error");
                        ui_state.add_output(OutputLine::System(
                            format!("Plan skipped ({}). Executing directly...", error_detail),
                        ));
                        ui_state.start_thinking();
                        spawn_execution_with_plan(
                            &app_state,
                            &ui_state.last_user_input,
                            "",
                            result_tx.clone(),
                        );
                    } else {
                        ui_state.add_output(OutputLine::Plan(plan.clone()));
                        if app_state.mode == AppMode::Auto || app_state.mode == AppMode::Plan {
                            // Auto/Plan mode: auto-execute
                            ui_state.start_thinking();
                            spawn_execution_with_plan(
                                &app_state,
                                &ui_state.last_user_input,
                                &plan,
                                result_tx.clone(),
                            );
                        } else {
                            // Edit mode: wait for approval
                            app_state.pending_plan = Some(plan);
                            app_state.is_processing = false;
                            ui_state.add_output(OutputLine::System(
                                "Execute? y/n/o (y=yes, n=no, o=other):".into(),
                            ));
                        }
                    }
                }
                agent::retry::PipelineResult::StepStart {
                    step,
                    total,
                    description,
                } => {
                    ui_state.stop_thinking();
                    tracing::info!("executing step {}/{}: {}", step, total, description);
                    ui_state.add_output(OutputLine::System(format!(
                        "Step {}/{}: {}",
                        step, total, description
                    )));
                    ui_state.start_thinking();
                }
                agent::retry::PipelineResult::SummaryReady {
                    summary,
                    summarized_count,
                } => {
                    tracing::info!(
                        "conversation summarized: {} messages compacted, {} byte summary",
                        summarized_count,
                        summary.len()
                    );
                    app_state.conversation_summary = Some(summary);
                    ui_state.add_output(OutputLine::System(format!(
                        "Context compacted ({} messages summarized)",
                        summarized_count
                    )));
                }
                agent::retry::PipelineResult::ToolStart { tool_name, call_id } => {
                    tracing::info!("tool started: {} ({})", tool_name, call_id);
                    ui_state.add_output(OutputLine::System(format!("  Running: {}...", tool_name)));
                }
                agent::retry::PipelineResult::ToolResultReady { result } => {
                    if result.success {
                        tracing::info!(
                            "tool completed: {} ({} bytes)",
                            result.tool_name,
                            result.output.len()
                        );
                        let preview: String = result.output.chars().take(200).collect();
                        ui_state.add_output(OutputLine::System(format!(
                            "  {} done: {}{}",
                            result.tool_name,
                            preview,
                            if result.output.len() > 200 { "..." } else { "" }
                        )));
                    } else {
                        tracing::warn!("tool failed: {} — {}", result.tool_name, result.output);
                        ui_state.add_output(OutputLine::Error(format!(
                            "  {} failed: {}",
                            result.tool_name, result.output
                        )));
                    }
                }
                agent::retry::PipelineResult::DiagnosticReady { result: diag } => {
                    if diag.has_errors() {
                        tracing::warn!(
                            "post-write diagnostics: {} errors in {} files",
                            diag.errors.len(),
                            diag.files_checked
                        );
                        for err in &diag.errors {
                            let loc = match err.line {
                                Some(l) => format!("{}:{}", err.file, l),
                                None => err.file.clone(),
                            };
                            ui_state.add_output(OutputLine::Error(format!(
                                "  diagnostic: {} — {}",
                                loc, err.message
                            )));
                        }
                    } else if diag.files_checked > 0 {
                        tracing::info!("diagnostics passed for {} files", diag.files_checked);
                    }
                }
                agent::retry::PipelineResult::EvalReady { evaluation } => {
                    tracing::info!("eval review: {} bytes", evaluation.len());
                    ui_state.add_output(OutputLine::System(evaluation));
                }
            }

            // Drain next queued message if any
            if !app_state.pending_queue.is_empty() {
                let next = app_state.pending_queue.remove(0);
                tracing::info!(
                    "draining queued message: {:?} ({} remaining in queue)",
                    next,
                    app_state.pending_queue.len()
                );
                ui_state.add_output(OutputLine::System(
                    "Processing queued message...".to_string(),
                ));
                ui_state.add_output(OutputLine::User(next.clone()));
                ui_state.start_thinking();
                spawn_request_for_mode(&mut app_state, &ollama_client, &next, result_tx.clone());
                app_state.is_processing = true;
            }
        }

        if event::poll(Duration::from_millis(100))? {
            let ev = event::read()?;
            match ev {
                Event::Paste(text) => {
                    ui_state.set_paste(text);
                }
                Event::Key(key) => {
                    // Handle edit confirmation keys (y/n/a) when awaiting
                    if app_state.awaiting_confirmation && key.modifiers == KeyModifiers::NONE {
                        match key.code {
                            KeyCode::Char('y') => {
                                // Destructive ops need double-key (YY)
                                if app_state.awaiting_destructive_confirm {
                                    app_state.awaiting_destructive_confirm = false;
                                    // Second Y confirmed — proceed
                                } else {
                                    // Check if current action is destructive
                                    if let Some(action) = app_state.pending_confirmations.last() {
                                        let risk = pending_action_risk(action);
                                        if risk == approval::RiskLevel::Destructive {
                                            ui_state.add_output(OutputLine::System(
                                                "Destructive! Press Y again to confirm.".into(),
                                            ));
                                            app_state.awaiting_destructive_confirm = true;
                                            continue;
                                        }
                                    }
                                }
                                // Apply the next pending file
                                if let Some(action) = app_state.pending_confirmations.pop() {
                                    tracing::info!(
                                        "user confirmed: applying file ({} remaining)",
                                        app_state.pending_confirmations.len()
                                    );
                                    let sandbox =
                                        sandbox::Sandbox::new(app_state.workspace.clone());
                                    apply_pending_action(
                                        &mut ui_state,
                                        &app_state,
                                        action,
                                        &sandbox,
                                    );
                                }
                                if app_state.pending_confirmations.is_empty() {
                                    app_state.awaiting_confirmation = false;
                                    ui_state.add_output(OutputLine::System(
                                        "All files reviewed.".into(),
                                    ));
                                } else {
                                    ui_state.add_output(OutputLine::System(format!(
                                        "Apply next? ({} remaining) y/n/a/o (y=yes, n=no, a=apply all, o=other):",
                                        app_state.pending_confirmations.len()
                                    )));
                                }
                                continue;
                            }
                            KeyCode::Char('n') => {
                                // Skip this file
                                if let Some(action) = app_state.pending_confirmations.pop() {
                                    tracing::info!(
                                        "user skipped file ({} remaining)",
                                        app_state.pending_confirmations.len()
                                    );
                                    let path = match &action {
                                        app::PendingAction::WriteFile { path, .. } => {
                                            path.display().to_string()
                                        }
                                        app::PendingAction::DeleteFile { path } => {
                                            path.display().to_string()
                                        }
                                        app::PendingAction::ExecuteCommand { cmd, .. } => {
                                            cmd.clone()
                                        }
                                    };
                                    ui_state.add_output(OutputLine::System(format!(
                                        "Skipped {}",
                                        path
                                    )));
                                }
                                if app_state.pending_confirmations.is_empty() {
                                    app_state.awaiting_confirmation = false;
                                    ui_state.add_output(OutputLine::System(
                                        "All files reviewed.".into(),
                                    ));
                                } else {
                                    ui_state.add_output(OutputLine::System(format!(
                                        "Apply next? ({} remaining) y/n/a/o (y=yes, n=no, a=apply all, o=other):",
                                        app_state.pending_confirmations.len()
                                    )));
                                }
                                continue;
                            }
                            KeyCode::Char('a') => {
                                // Apply all remaining
                                tracing::info!(
                                    "user chose apply-all for {} remaining file(s)",
                                    app_state.pending_confirmations.len()
                                );
                                let sandbox = sandbox::Sandbox::new(app_state.workspace.clone());
                                let remaining =
                                    std::mem::take(&mut app_state.pending_confirmations);
                                let total = remaining.len();
                                let mut applied = 0;
                                for action in remaining {
                                    apply_pending_action(
                                        &mut ui_state,
                                        &app_state,
                                        action,
                                        &sandbox,
                                    );
                                    applied += 1;
                                }
                                app_state.awaiting_confirmation = false;
                                ui_state.add_output(OutputLine::System(format!(
                                    "Applied {}/{} remaining file(s)",
                                    applied, total
                                )));
                                continue;
                            }
                            KeyCode::Char('o') => {
                                // Other: user provides custom feedback
                                tracing::info!(
                                    "user chose Other for file review ({} pending cancelled)",
                                    app_state.pending_confirmations.len()
                                );
                                app_state.clear_pending();
                                app_state.awaiting_confirmation = false;
                                app_state.awaiting_other_input = true;
                                ui_state.add_output(OutputLine::System(
                                    "Other: (type your response and press Enter)".into(),
                                ));
                                continue;
                            }
                            _ => {} // Fall through to normal key handling
                        }
                    }

                    // Plan approval: 'y' to execute, 'n' to cancel
                    if app_state.pending_plan.is_some()
                        && key.modifiers == KeyModifiers::NONE
                    {
                        match key.code {
                            KeyCode::Char('y') => {
                                if let Some(plan) = app_state.pending_plan.take() {
                                    ui_state.add_output(OutputLine::System("Executing plan...".into()));
                                    ui_state.start_thinking();
                                    app_state.is_processing = true;
                                    spawn_execution_with_plan(
                                        &app_state,
                                        &ui_state.last_user_input,
                                        &plan,
                                        result_tx.clone(),
                                    );
                                }
                                continue;
                            }
                            KeyCode::Char('n') => {
                                if app_state.pending_plan.take().is_some() {
                                    ui_state.add_output(OutputLine::System("Plan cancelled.".into()));
                                }
                                continue;
                            }
                            _ => {} // fall through to 'o' handler and normal input
                        }
                    }

                    // Plan approval: 'o' for Other feedback
                    if app_state.pending_plan.is_some()
                        && key.modifiers == KeyModifiers::NONE
                        && key.code == KeyCode::Char('o')
                    {
                        app_state.pending_plan = None;
                        app_state.awaiting_other_input = true;
                        ui_state.add_output(OutputLine::System(
                            "Other: (type your response and press Enter)".into(),
                        ));
                        continue;
                    }

                    match (key.modifiers, key.code) {
                        // Shift+Tab: switch mode (crossterm sends BackTab for Shift+Tab)
                        (_, KeyCode::BackTab) => {
                            let old_mode = app_state.mode;
                            let new_mode = app_state.switch_mode();
                            app_state.prompt_builder.set_mode(new_mode);
                            tracing::info!("mode switch: {} -> {}", old_mode, new_mode);
                            ui_state.add_output(OutputLine::System(format!(
                                "Switched to {} mode",
                                new_mode
                            )));
                        }
                        // Ctrl+C: quit
                        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                            tracing::info!("user requested quit (Ctrl+C)");
                            if app_state.mode == AppMode::Auto {
                                ui_state.add_output(OutputLine::System(
                                    "Press Ctrl+C again to confirm quit from AUTO mode".into(),
                                ));
                                // Simple double-press: check next event
                                if event::poll(Duration::from_secs(2))? {
                                    if let Event::Key(k2) = event::read()? {
                                        if k2.code == KeyCode::Char('c')
                                            && k2.modifiers.contains(KeyModifiers::CONTROL)
                                        {
                                            break;
                                        }
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                        // Ctrl+Tab: toggle model thinking
                        (KeyModifiers::CONTROL, KeyCode::Tab) => {
                            app_state.think_enabled = !app_state.think_enabled;
                            let state = if app_state.think_enabled {
                                "THINK"
                            } else {
                                "DIRECT"
                            };
                            tracing::info!("think mode: {}", state);
                            ui_state
                                .add_output(OutputLine::System(format!("Model mode: {}", state)));
                        }
                        // Shift+Enter: insert newline
                        (KeyModifiers::SHIFT, KeyCode::Enter) => {
                            ui_state.push_char('\n');
                        }
                        // Enter: submit input or approve pending plan
                        (KeyModifiers::NONE, KeyCode::Enter) => {
                            // Other input: submit user feedback
                            if app_state.awaiting_other_input {
                                let input = ui_state.take_input();
                                if !input.is_empty() {
                                    app_state.awaiting_other_input = false;
                                    tracing::info!("user Other input: {}", input.trim());
                                    ui_state.add_output(OutputLine::User(input.clone()));
                                    app_state.conversation_history.push(
                                        app::ConversationMessage {
                                            role: "user".into(),
                                            content: input.clone(),
                                            tokens: 0,
                                        },
                                    );
                                    ui_state.start_thinking();
                                    spawn_request_for_mode(
                                        &mut app_state,
                                        &ollama_client,
                                        &input,
                                        result_tx.clone(),
                                    );
                                    app_state.is_processing = true;
                                } else {
                                    ui_state.add_output(OutputLine::System(
                                        "Cancelled.".into(),
                                    ));
                                    app_state.awaiting_other_input = false;
                                }
                                continue;
                            }

                            let input = ui_state.take_input();
                            if !input.is_empty() {
                                app_state.record_input(input.clone());
                                tracing::info!("user input: {}", input.trim());
                                tracing::debug!(
                                    "conversation history: {} messages, queue: {}",
                                    app_state.conversation_history.len(),
                                    app_state.pending_queue.len()
                                );
                                ui_state.last_user_input = input.clone();

                                if input.trim() == "/quit" || input.trim() == "/exit" {
                                    tracing::info!(
                                        "session ending via /{}",
                                        input.trim().trim_start_matches('/')
                                    );
                                    break;
                                }

                                // Handle slash commands (instant, no LLM call)
                                let trimmed = input.trim();
                                if trimmed == "/clear" {
                                    app_state.context_manager.clear();
                                    app_state.conversation_history.clear();
                                    app_state.conversation_summary = None;
                                    ui_state.clear_output();
                                    ui_state.add_output(OutputLine::System(
                                        "Context cleared. Starting fresh session.".into(),
                                    ));
                                    ui_state.add_output(OutputLine::System(
                                        "Shift+Tab: mode | Enter: send | Shift+Enter: newline | Ctrl+Tab: think | Ctrl+C: quit | /skills: list skills".into(),
                                    ));
                                } else if trimmed == "/skills" {
                                    let skills = app_state.skills.list();
                                    if skills.is_empty() {
                                        ui_state.add_output(OutputLine::System(
                                            "No skills loaded.".into(),
                                        ));
                                    } else {
                                        let mut lines = vec!["Available skills:".to_string()];
                                        for s in skills {
                                            lines
                                                .push(format!("  /{} — {}", s.name, s.description));
                                        }
                                        ui_state.add_output(OutputLine::System(lines.join("\n")));
                                    }
                                } else if trimmed == "/setup" {
                                    tracing::info!("running setup wizard");
                                    match wizard::run(
                                        terminal,
                                        app_state.config.clone(),
                                        &app_state.workspace,
                                    ) {
                                        Ok(new_config) => {
                                            app_state.config = new_config;
                                            ui_state.add_output(OutputLine::System(
                                                "Setup complete. Configuration updated.".into(),
                                            ));
                                        }
                                        Err(e) => {
                                            ui_state.add_output(OutputLine::Error(format!(
                                                "Setup wizard failed: {}",
                                                e
                                            )));
                                        }
                                    }
                                    let skills = app_state.skills.list();
                                    if skills.is_empty() {
                                        ui_state.add_output(OutputLine::System(
                                            "No skills loaded.".into(),
                                        ));
                                    } else {
                                        let mut lines = vec!["Available skills:".to_string()];
                                        for s in skills {
                                            lines
                                                .push(format!("  /{} — {}", s.name, s.description));
                                        }
                                        ui_state.add_output(OutputLine::System(lines.join("\n")));
                                    }
                                } else if trimmed == "/recap" {
                                    // Generate a recap of the current session
                                    if !app_state.config.enable_recap {
                                        ui_state.add_output(OutputLine::System(
                                            "Recap is disabled in config.".into(),
                                        ));
                                    } else {
                                        let history = app_state.conversation_history.clone();
                                        let config = app_state.config.clone();
                                        let tx = result_tx.clone();
                                        ui_state.add_output(OutputLine::System(
                                            "Generating recap...".into(),
                                        ));
                                        std::thread::spawn(move || {
                                            let rt = tokio::runtime::Runtime::new().unwrap();
                                            let result = rt.block_on(async {
                                                let client = ollama::OllamaClient::new(&config)?;
                                                recap::generate_recap(&client, &history, &config)
                                                    .await
                                            });
                                            match result {
                                                Ok(summary) => {
                                                    let _ = tx.send(
                                                        agent::retry::PipelineResult::Retry(
                                                            agent::retry::RetryResult::Success {
                                                                content: format!(
                                                                    "Recap: {}",
                                                                    summary
                                                                ),
                                                                attempts: 0,
                                                            },
                                                        ),
                                                    );
                                                }
                                                Err(e) => {
                                                    let _ = tx.send(
                                                        agent::retry::PipelineResult::Retry(
                                                            agent::retry::RetryResult::Failed {
                                                                last_error: format!(
                                                                    "Recap failed: {}",
                                                                    e
                                                                ),
                                                                attempts: 0,
                                                            },
                                                        ),
                                                    );
                                                }
                                            }
                                        });
                                    } // end enable_recap guard
                                } else if trimmed == "/snapshots" || trimmed == "/snaps" {
                                    match app_state.snapshot_manager.list(10) {
                                        Ok(entries) => {
                                            if entries.is_empty() {
                                                ui_state.add_output(OutputLine::System(
                                                    "No snapshots yet.".into(),
                                                ));
                                            } else {
                                                let mut lines =
                                                    vec!["Recent snapshots:".to_string()];
                                                for (i, e) in entries.iter().enumerate() {
                                                    lines.push(format!(
                                                        "  {} {} {}",
                                                        &e.hash[..8],
                                                        e.date,
                                                        e.message
                                                    ));
                                                    if i >= 9 {
                                                        break;
                                                    }
                                                }
                                                lines.push(
                                                    "Use /undo to restore last, /restore <hash> for specific"
                                                        .into(),
                                                );
                                                ui_state.add_output(OutputLine::System(
                                                    lines.join("\n"),
                                                ));
                                            }
                                        }
                                        Err(e) => {
                                            ui_state.add_output(OutputLine::Error(format!(
                                                "Snapshot list failed: {}",
                                                e
                                            )));
                                        }
                                    }
                                } else if trimmed == "/undo" {
                                    match app_state.snapshot_manager.list(2) {
                                        Ok(entries) => {
                                            if entries.len() < 2 {
                                                ui_state.add_output(OutputLine::Error(
                                                    "No snapshot to undo to.".into(),
                                                ));
                                            } else {
                                                let target = &entries[1];
                                                match app_state
                                                    .snapshot_manager
                                                    .restore(&target.hash)
                                                {
                                                    Ok(()) => {
                                                        ui_state.add_output(OutputLine::System(
                                                            format!(
                                                                "Restored to {} ({})",
                                                                &target.hash[..8],
                                                                target.message
                                                            ),
                                                        ));
                                                    }
                                                    Err(e) => {
                                                        ui_state.add_output(OutputLine::Error(
                                                            format!("Restore failed: {}", e),
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            ui_state.add_output(OutputLine::Error(format!(
                                                "Snapshot access failed: {}",
                                                e
                                            )));
                                        }
                                    }
                                } else if trimmed.starts_with("/restore ") {
                                    let hash = trimmed.trim_start_matches("/restore ").trim();
                                    if hash.is_empty() {
                                        ui_state.add_output(OutputLine::Error(
                                            "Usage: /restore <commit-hash>".into(),
                                        ));
                                    } else {
                                        match app_state.snapshot_manager.restore(hash) {
                                            Ok(()) => {
                                                ui_state.add_output(OutputLine::System(format!(
                                                    "Restored to {}",
                                                    &hash[..hash.len().min(8)]
                                                )));
                                            }
                                            Err(e) => {
                                                ui_state.add_output(OutputLine::Error(format!(
                                                    "Restore failed: {}",
                                                    e
                                                )));
                                            }
                                        }
                                    }
                                } else if trimmed == "/apply" {
                                    // Apply file changes from the last assistant response
                                    tracing::info!(
                                        "applying file changes, mode={}",
                                        app_state.mode
                                    );
                                    tracing::debug!(
                                        "conversation history length: {} messages",
                                        app_state.conversation_history.len()
                                    );
                                    let last_content = ui_state.output_lines.iter().rev().find_map(
                                        |ol| match ol {
                                            OutputLine::Assistant(t) => Some(t.clone()),
                                            _ => None,
                                        },
                                    );
                                    if let Some(ref content) = last_content {
                                        let changes =
                                            agent::AgentPipeline::parse_file_changes(content);
                                        if changes.is_empty() {
                                            ui_state.add_output(OutputLine::Error(
                                                "No file changes found in the last response."
                                                    .into(),
                                            ));
                                        } else if app_state.mode == AppMode::Edit {
                                            // Edit mode: queue confirmations for y/n review
                                            let sandbox =
                                                sandbox::Sandbox::new(app_state.workspace.clone());
                                            let file_ops = project::file_ops::FileOps::new(
                                                &sandbox,
                                                app_state.mode,
                                            );
                                            app_state.clear_pending();
                                            let mut queued = 0;
                                            for change in &changes {
                                                let full_path =
                                                    app_state.workspace.join(&change.path);
                                                let fc = match if change.action == "delete" {
                                                    file_ops.prepare_delete(&full_path)
                                                } else {
                                                    file_ops
                                                        .prepare_write(&full_path, &change.content)
                                                } {
                                                    Ok(fc) => fc,
                                                    Err(e) => {
                                                        ui_state.add_output(OutputLine::Error(
                                                            format!(
                                                                "Blocked {}: {}",
                                                                change.path.display(),
                                                                e
                                                            ),
                                                        ));
                                                        continue;
                                                    }
                                                };

                                                // Show diff preview
                                                if !fc.diff_preview.is_empty() {
                                                    ui_state.add_output(OutputLine::System(
                                                        format!("--- {}", change.path.display()),
                                                    ));
                                                    for line in fc.diff_preview.lines() {
                                                        if line.starts_with('-')
                                                            && !line.starts_with("---")
                                                        {
                                                            ui_state.add_output(OutputLine::Diff {
                                                                added: vec![],
                                                                removed: vec![line.to_string()],
                                                            });
                                                        } else if line.starts_with('+')
                                                            && !line.starts_with("+++")
                                                        {
                                                            ui_state.add_output(OutputLine::Diff {
                                                                added: vec![line.to_string()],
                                                                removed: vec![],
                                                            });
                                                        }
                                                    }
                                                }

                                                let action = if change.action == "delete" {
                                                    app::PendingAction::DeleteFile {
                                                        path: full_path,
                                                    }
                                                } else {
                                                    app::PendingAction::WriteFile {
                                                        path: full_path,
                                                        content: change.content.clone(),
                                                        diff_preview: fc.diff_preview,
                                                    }
                                                };
                                                app_state.push_pending(action);
                                                queued += 1;
                                            }
                                            if queued > 0 {
                                                app_state.awaiting_confirmation = true;
                                                ui_state.add_output(OutputLine::System(format!(
                                                "Review {} file(s). y/n/a/o (y=yes, n=no, a=apply all, o=other):",
                                                queued
                                            )));
                                            }
                                        } else {
                                            // Auto mode: apply all immediately (Plan mode blocked by FileOps)
                                            let sandbox =
                                                sandbox::Sandbox::new(app_state.workspace.clone());
                                            let file_ops = project::file_ops::FileOps::new(
                                                &sandbox,
                                                app_state.mode,
                                            );
                                            let mut applied = 0;
                                            for change in &changes {
                                                let full_path =
                                                    app_state.workspace.join(&change.path);
                                                let fc = match if change.action == "delete" {
                                                    file_ops.prepare_delete(&full_path)
                                                } else {
                                                    file_ops
                                                        .prepare_write(&full_path, &change.content)
                                                } {
                                                    Ok(fc) => fc,
                                                    Err(e) => {
                                                        ui_state.add_output(OutputLine::Error(
                                                            format!(
                                                                "Blocked {}: {}",
                                                                change.path.display(),
                                                                e
                                                            ),
                                                        ));
                                                        continue;
                                                    }
                                                };
                                                match file_ops.apply_change(&fc) {
                                                    Ok(()) => {
                                                        ui_state.add_output(OutputLine::System(
                                                            format!(
                                                                "Wrote {}",
                                                                change.path.display()
                                                            ),
                                                        ));
                                                        applied += 1;
                                                        run_syntax_check(
                                                            &mut ui_state,
                                                            &app_state.workspace.join(&change.path),
                                                            &sandbox,
                                                        );
                                                    }
                                                    Err(e) => {
                                                        ui_state.add_output(OutputLine::Error(
                                                            format!(
                                                                "Failed to write {}: {}",
                                                                change.path.display(),
                                                                e
                                                            ),
                                                        ));
                                                    }
                                                }
                                            }
                                            ui_state.add_output(OutputLine::System(format!(
                                                "Applied {}/{} file(s)",
                                                applied,
                                                changes.len()
                                            )));
                                        }
                                    } else {
                                        ui_state.add_output(OutputLine::Error(
                                            "No assistant response to apply.".into(),
                                        ));
                                    }
                                } else if let Some(uv_args) = trimmed.strip_prefix("/uv ") {
                                    // /uv commands: init, venv, add <pkg>, run <script>
                                    handle_uv_command(
                                        &mut app_state,
                                        &mut ui_state,
                                        uv_args.trim(),
                                    );
                                } else if trimmed == "/uv" {
                                    ui_state.add_output(OutputLine::System(
                                    "Usage: /uv init | /uv venv | /uv add <package> | /uv run <script>".into(),
                                ));
                                } else if let Some(run_cmd) = trimmed.strip_prefix("/run ") {
                                    handle_run_command(
                                        &mut app_state,
                                        &mut ui_state,
                                        run_cmd.trim(),
                                    );
                                } else if let Some(cmd) = trimmed.strip_prefix('/') {
                                    let (skill_name, args) = match cmd.split_once(' ') {
                                        Some((name, rest)) => (name, rest.trim()),
                                        None => (cmd, ""),
                                    };

                                    if let Some(skill) = app_state.skills.get(skill_name).cloned() {
                                        tracing::info!("skill: /{} {:?}", skill_name, args);
                                        let full_input = if args.is_empty() {
                                            String::new()
                                        } else {
                                            args.to_string()
                                        };
                                        if app_state.is_processing {
                                            // Queue skill invocation
                                            let label = format!("/{} {}", skill_name, full_input)
                                                .trim_end()
                                                .to_string();
                                            app_state.pending_queue.push(label.clone());
                                            ui_state.add_output(OutputLine::Pending(label));
                                        } else {
                                            ui_state.add_output(OutputLine::User(input.clone()));
                                            ui_state.start_thinking();
                                            spawn_skill_request(
                                                &app_state,
                                                &ollama_client,
                                                &skill,
                                                &full_input,
                                                result_tx.clone(),
                                            );
                                            app_state.is_processing = true;
                                        }
                                    } else {
                                        ui_state.add_output(OutputLine::Error(format!(
                                        "Unknown skill: /{}. Type /skills to see available skills.",
                                        skill_name
                                    )));
                                    }
                                } else if app_state.is_processing {
                                    // Queue the message for later processing
                                    app_state.pending_queue.push(input.clone());
                                    ui_state.add_output(OutputLine::Pending(input));
                                } else {
                                    // Display user message immediately and spawn background request
                                    ui_state.add_output(OutputLine::User(input.clone()));
                                    // Record in conversation history
                                    app_state.conversation_history.push(ConversationMessage {
                                        role: "user".into(),
                                        content: input.clone(),
                                        tokens: util::text::estimate_tokens(&input),
                                    });
                                    // Record in session
                                    app_state.current_session.add_message("user", &input);
                                    ui_state.start_thinking();
                                    // Track KV cache prefix hash before spawning
                                    app_state.context_manager.set_static_prefix_hash(
                                        app_state.prompt_builder.static_prefix_hash()
                                    );
                                    spawn_request_for_mode(
                                        &mut app_state,
                                        &ollama_client,
                                        &input,
                                        result_tx.clone(),
                                    );
                                    app_state.is_processing = true;
                                }
                            }
                        }
                        // Backspace
                        (KeyModifiers::NONE, KeyCode::Backspace) => {
                            ui_state.backspace();
                        }
                        // Escape: cancel other input
                        (KeyModifiers::NONE, KeyCode::Esc) => {
                            if app_state.awaiting_other_input {
                                app_state.awaiting_other_input = false;
                                ui_state.input_text.clear();
                                ui_state.input_cursor = 0;
                                ui_state.add_output(OutputLine::System("Cancelled.".into()));
                            }
                        }
                        // Page Up/Down: no virtual scrolling (terminal native scrollback)
                        // Character input (allow NONE or SHIFT for uppercase)
                        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                            app_state.history_index = 0;
                            ui_state.push_char(c);
                        }
                        // Up arrow: navigate input history (older)
                        (KeyModifiers::NONE, KeyCode::Up) => {
                            if !app_state.input_history.is_empty()
                                && app_state.history_index < app_state.input_history.len()
                            {
                                app_state.history_index += 1;
                                let idx = app_state.input_history.len() - app_state.history_index;
                                ui_state.input_text = app_state.input_history[idx].clone();
                                ui_state.input_cursor = ui_state.input_text.chars().count();
                            }
                        }
                        // Down arrow: navigate input history (newer)
                        (KeyModifiers::NONE, KeyCode::Down) => {
                            if app_state.history_index > 0 {
                                app_state.history_index -= 1;
                                if app_state.history_index == 0 {
                                    ui_state.input_text.clear();
                                    ui_state.input_cursor = 0;
                                } else {
                                    let idx =
                                        app_state.input_history.len() - app_state.history_index;
                                    ui_state.input_text = app_state.input_history[idx].clone();
                                    ui_state.input_cursor = ui_state.input_text.chars().count();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Flush pending inline output above the viewport using `terminal.insert_before()`.
fn flush_pending_output(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ui_state: &mut ui::UiState,
) -> Result<()> {
    if ui_state.pending_inline.is_empty() {
        return Ok(());
    }
    let lines = std::mem::take(&mut ui_state.pending_inline);
    let theme = &ui_state.theme;
    let width = terminal.size()?.width;
    let line_count = ui::inline::estimate_line_count(&lines, width);

    terminal.insert_before(line_count, |buf| {
        ui::inline::render_output_lines(buf, &lines, &theme);
    })?;
    Ok(())
}

/// Spawn a streaming generate request on a background thread (direct, no plan step).
#[allow(dead_code)]
fn spawn_llm_request(
    app_state: &AppState,
    _client: &ollama::OllamaClient,
    input: &str,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    let model = match router::classify_request(input) {
        router::ModelTier::Exec => app_state.config.exec_model.clone(),
        router::ModelTier::Eval => app_state.config.effective_eval_model().to_string(),
    };
    if model.is_empty() {
        let _ = tx.send(agent::retry::PipelineResult::Retry(
            agent::retry::RetryResult::Failed {
                last_error: "No exec model configured. Run setup or edit ~/.litepilot/config.toml"
                    .into(),
                attempts: 0,
            },
        ));
        return;
    }

    let endpoint = app_state.config.ollama_endpoint.clone();
    let connect_timeout = app_state.config.connect_timeout;
    let context_window_limit = app_state.config.context_window_limit;
    let input = input.to_string();
    let web_search_enabled = app_state.web_search_enabled;
    let max_search_tokens = app_state.config.max_search_context_tokens;

    // Build system prompt via PromptBuilder (already updated before calling this function)
    let system_with_workspace = app_state.prompt_builder.build();

    // Build generate prompt from conversation history
    let (history_system, history_prompt) =
        build_generate_prompt(&app_state.conversation_history, &input);
    let combined_system = if history_system.is_some() {
        Some(format!(
            "{}\n\n{}",
            system_with_workspace,
            history_system.unwrap_or_default()
        ))
    } else {
        Some(system_with_workspace.clone())
    };
    let context_handle = app_state.context_manager.context_handle_for_model(&model).cloned();
    let total_prompt_tokens = estimate_prompt_tokens(combined_system.as_deref(), &history_prompt);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: format!("Runtime error: {}", e),
                        attempts: 0,
                    },
                ));
                return;
            }
        };

        // Run web search if enabled
        let final_prompt = if web_search_enabled {
            let search_ctx = rt.block_on(run_web_search(&input, max_search_tokens));
            if !search_ctx.is_empty() {
                let _ = tx.send(agent::retry::PipelineResult::SearchDone {
                    count: search_ctx.lines().filter(|l| l.starts_with('[')).count(),
                    context: search_ctx.clone(),
                });
            }
            if history_prompt.is_empty() {
                format!("{}\n{}", search_ctx, input)
            } else {
                format!("{}\nuser: {}\n{}", history_prompt, search_ctx, input)
            }
        } else {
            history_prompt
        };

        // Build streaming request — use a client without overall deadline
        let http =
            match ollama::OllamaClient::streaming_http_client(Duration::from_secs(connect_timeout))
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(agent::retry::PipelineResult::Retry(
                        agent::retry::RetryResult::Failed {
                            last_error: format!("Client error: {}", e),
                            attempts: 0,
                        },
                    ));
                    return;
                }
            };

        tracing::info!(
            "spawn_llm_generate: model={}, context_window={}, web_search={}, has_context_handle={}",
            model,
            context_window_limit,
            web_search_enabled,
            context_handle.is_some()
        );

        let step = stream_single_step_generate(
            &rt,
            http,
            &endpoint,
            &model,
            combined_system,
            final_prompt,
            context_handle,
            context_window_limit,
            &tx,
        );

        if step.content.starts_with("ERROR:") {
            let _ = tx.send(agent::retry::PipelineResult::Retry(
                agent::retry::RetryResult::Failed {
                    last_error: step.content,
                    attempts: 0,
                },
            ));
        } else {
            let _ = tx.send(agent::retry::PipelineResult::StreamDone {
                content: step.content,
            });
            let _ = tx.send(agent::retry::PipelineResult::StreamMeta {
                context_handle: step.context_handle,
                prompt_eval_count: step.prompt_eval_count,
                eval_count: step.eval_count,
                total_prompt_tokens,
                model,
            });
        }
    });
}

/// Plan-then-execute: runs a quick plan step on the exec model, sends PlanReady,
/// then the main loop waits for user approval before calling spawn_execution_with_plan.
fn spawn_plan_then_execute(
    app_state: &AppState,
    _client: &ollama::OllamaClient,
    input: &str,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    let exec_model = app_state.config.exec_model.clone();
    let endpoint = app_state.config.ollama_endpoint.clone();
    let connect_timeout = app_state.config.connect_timeout;
    let context_window_limit = app_state.config.context_window_limit;
    let max_file_lines = app_state.config.max_file_lines;
    let input = input.to_string();
    let history_ctx = app_state
        .conversation_history
        .iter()
        .rev()
        .take(6)
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    let plan_mode = app_state.mode == AppMode::Plan;
    let workspace = app_state.workspace.clone();

    // Build tool descriptions once on the main thread (cheap: just registers handlers).
    // The planner only sees prose names/descriptions — no native tool-calling. It emits
    // text steps that reference tools; the executor (spawn_execution_with_plan) does the
    // actual dispatch.
    let tools_description = tools::ToolRegistry::new(workspace.clone(), &app_state.config)
        .descriptions_text();

    tracing::info!(
        "spawn_plan_then_execute: model={}, history_msgs={}, workspace={}, tools_chars={}",
        exec_model,
        app_state.conversation_history.len(),
        workspace.display(),
        tools_description.chars().count()
    );

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: format!("Runtime error: {}", e),
                        attempts: 0,
                    },
                ));
                return;
            }
        };

        let bg_config = config::Config {
            ollama_endpoint: endpoint,
            connect_timeout,
            exec_model: exec_model.clone(),
            context_window_limit,
            ..config::Config::default()
        };
        let bg_client = match ollama::OllamaClient::new(&bg_config) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: format!("Client error: {}", e),
                        attempts: 0,
                    },
                ));
                return;
            }
        };

        // Quick plan step using exec model
        let project_listing = std::fs::read_dir(&workspace)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let user_msg = format!(
            "Working directory: {}\nFiles in directory: {}\nCurrent date: {}\n\n{}\nNew request: {}",
            workspace.display(),
            project_listing,
            current_datetime(),
            if history_ctx.is_empty() {
                String::new()
            } else {
                format!("Recent context:\n{}\n\n", history_ctx)
            },
            input
        );
        let messages = vec![
            ollama::chat::ChatMessage::system(apply_plan_prompt(
                agent::prompts::QUICK_PLAN_SYSTEM,
                max_file_lines,
                &tools_description,
            )),
            ollama::chat::ChatMessage::user(&user_msg),
        ];

        match rt.block_on(bg_client.chat(&exec_model, messages, false)) {
            Ok(resp) => {
                let plan = resp.content;
                let _ = tx.send(agent::retry::PipelineResult::PlanReady { plan: plan.clone() });
                // In Plan mode, auto-continue to execution (read-only analysis)
                if plan_mode {
                    // The main loop will handle this: Plan mode skips the approval gate
                }
            }
            Err(e) => {
                // Plan step failed — fall back to direct streaming without plan
                tracing::warn!("plan step failed for exec model '{}': {}", exec_model, e);
                let _ = tx.send(agent::retry::PipelineResult::PlanReady {
                    plan: format!("(plan unavailable: exec model '{}' — {})", exec_model, e),
                });
            }
        }
    });
}

/// Execute with a plan: step-by-step execution, one plan step per LLM call.
fn spawn_execution_with_plan(
    app_state: &AppState,
    input: &str,
    plan: &str,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    let model = app_state.config.exec_model.clone();
    if model.is_empty() {
        let _ = tx.send(agent::retry::PipelineResult::Retry(
            agent::retry::RetryResult::Failed {
                last_error: "No core model configured.".into(),
                attempts: 0,
            },
        ));
        return;
    }

    let endpoint = app_state.config.ollama_endpoint.clone();
    let connect_timeout = app_state.config.connect_timeout;
    let input = input.to_string();
    let plan = plan.to_string();
    let history = app_state.conversation_history.clone();
    let workspace = app_state.workspace.clone();
    let context_window_limit = app_state.config.context_window_limit;
    let now = current_datetime();
    let context_handle = app_state.context_manager.context_handle_for_model(&model).cloned();

    // Tool registry for per-step tool dispatch. Built once outside the step loop and
    // shared across all steps in this execution. The LLM emits text-format
    // `<tool_call>` blocks mid-step; `stream_step_with_tools` parses and dispatches.
    let config = app_state.config.clone();
    let tool_registry = tools::ToolRegistry::new(workspace.clone(), &config);

    // Build system prompt via PromptBuilder (already updated before calling this function)
    let coding_system = app_state.prompt_builder.build();

    tracing::info!(
        "spawn_execution_with_plan: model={}, plan={} bytes, history_msgs={}, context_window={}, has_context_handle={}",
        model,
        plan.len(),
        history.len(),
        context_window_limit,
        context_handle.is_some()
    );

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: format!("Runtime error: {}", e),
                        attempts: 0,
                    },
                ));
                return;
            }
        };

        let http = match ollama::OllamaClient::streaming_http_client(
            std::time::Duration::from_secs(connect_timeout),
        ) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: format!("Client error: {}", e),
                        attempts: 0,
                    },
                ));
                return;
            }
        };

        // Parse plan into individual steps
        let steps: Vec<String> = plan
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let rest = trimmed.trim_start_matches(|c: char| c.is_ascii_digit());
                if rest.len() == trimmed.len() {
                    return None;
                }
                rest.strip_prefix('.')
                    .or_else(|| rest.strip_prefix(')'))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .collect();

        // Build history prompt for /api/generate
        let history_prompt = history
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        if steps.is_empty() {
            tracing::info!(
                "no parseable steps in plan, executing as single request (plan: {} bytes)",
                plan.len()
            );
            let output_budget = context_window_limit / 4;
            let system_prompt = format!(
                "{}\n\nWorking directory: {}\nCurrent date: {}\nOutput limit: keep response under {} tokens ({} lines max). If the task is too large, output only the first part and note what remains.",
                coding_system,
                workspace.display(),
                now,
                output_budget,
                output_budget / 4,
            );
            let prompt = if history_prompt.is_empty() {
                format!("[Plan]\n{}\n\nuser: {}", plan, input)
            } else {
                format!("{}\n[Plan]\n{}\n\nuser: {}", history_prompt, plan, input)
            };
            let total_tokens = estimate_prompt_tokens(Some(&system_prompt), &prompt);

            let step = stream_step_with_tools(
                &rt,
                http,
                &endpoint,
                &model,
                Some(system_prompt),
                prompt,
                context_handle,
                context_window_limit,
                &tool_registry,
                &tx,
            );
            let _ = tx.send(agent::retry::PipelineResult::StreamDone {
                content: step.content,
            });
            let _ = tx.send(agent::retry::PipelineResult::StreamMeta {
                context_handle: step.context_handle,
                prompt_eval_count: step.prompt_eval_count,
                eval_count: step.eval_count,
                total_prompt_tokens: total_tokens,
                model,
            });
            return;
        }

        let total_steps = steps.len();
        tracing::info!(
            "executing {} steps from plan ({} bytes)",
            total_steps,
            plan.len()
        );
        let mut all_content = String::new();
        let mut step_results: Vec<String> = Vec::new();
        let per_step_budget = context_window_limit / (4 * total_steps.max(1) as u64);
        let system_prompt = format!(
            "{}\n\nWorking directory: {}\nCurrent date: {}\nOutput limit for this step: keep response under {} tokens ({} lines max). Output ONLY the current step — do not attempt other steps.",
            coding_system,
            workspace.display(),
            now,
            per_step_budget,
            per_step_budget / 4,
        );

        let mut current_context = context_handle;

        for (i, step_desc) in steps.iter().enumerate() {
            let step_num = i + 1;

            let _ = tx.send(agent::retry::PipelineResult::StepStart {
                step: step_num,
                total: total_steps,
                description: step_desc.clone(),
            });

            // Note: `[SEARCH]` plan steps were previously special-cased to invoke
            // run_web_search directly. That behavior is now subsumed by the executor's
            // tool loop: the LLM emits a `<tool_call name="web_search">` block mid-step
            // and `stream_step_with_tools` dispatches it via the ToolRegistry.

            let prev_summary = if step_results.is_empty() {
                String::new()
            } else {
                let mut s = "\n\n[Previous steps completed]\n".to_string();
                for (j, result) in step_results.iter().enumerate() {
                    // Extract file paths from previous results for quick reference
                    let file_paths: Vec<&str> = result
                        .lines()
                        .filter(|l| l.trim().starts_with("### FILE:"))
                        .map(|l| l.trim().trim_start_matches("### FILE:").trim())
                        .collect();
                    if !file_paths.is_empty() {
                        s.push_str(&format!(
                            "Step {}: {} → files: {}\n",
                            j + 1,
                            steps.get(j).map(|s| s.as_str()).unwrap_or("(completed)"),
                            file_paths.join(", ")
                        ));
                    } else {
                        // Non-file result (search, command output, etc.) — include short summary
                        let preview: String = result.chars().take(500).collect();
                        s.push_str(&format!(
                            "Step {}: {} → {}\n",
                            j + 1,
                            steps.get(j).map(|s| s.as_str()).unwrap_or("(completed)"),
                            preview
                        ));
                    }
                }
                s.push_str("\nIMPORTANT: Read any files from previous steps before proceeding. Files listed above contain the actual content from those steps.\n");
                s
            };

            let user_msg = format!(
                "Original request: {}\n\nPlan:\n{}\n{}Now execute ONLY step {} of {}: {}\n\nWorkflow for this step:\n1. If previous steps created files, READ them first to recall their content\n2. Generate the content needed for THIS step\n3. Write the result to a file immediately using the format below\n\nOutput file content using this format:\n### FILE: relative/path/to/file\n### ACTION: create|modify\n```\nfile content\n```",
                input, plan, prev_summary, step_num, total_steps, step_desc,
            );

            let prompt = if history_prompt.is_empty() {
                format!("user: {}", user_msg)
            } else {
                format!("{}\nuser: {}", history_prompt, user_msg)
            };

            let step_result = stream_step_with_tools(
                &rt,
                http.clone(),
                &endpoint,
                &model,
                Some(system_prompt.clone()),
                prompt,
                current_context.clone(),
                context_window_limit,
                &tool_registry,
                &tx,
            );

            tracing::info!(
                "step {}/{} complete: {} bytes, {} lines",
                step_num,
                total_steps,
                step_result.content.len(),
                step_result.content.lines().count()
            );

            if step_result.content.starts_with("ERROR:") {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: step_result.content,
                        attempts: step_num,
                    },
                ));
                return;
            }

            // Carry the context handle forward for cache reuse between steps
            if let Some(ctx) = &step_result.context_handle {
                current_context = Some(ctx.clone());
            }

            all_content.push_str(&step_result.content);
            all_content.push('\n');
            step_results.push(step_result.content);
        }

        // Send final StreamMeta with the last step's context handle
        // We estimate total tokens from the last step's prompt
        let last_prompt = if history_prompt.is_empty() {
            format!("user: {}", input)
        } else {
            format!("{}\nuser: {}", history_prompt, input)
        };
        let total_tokens = estimate_prompt_tokens(Some(&system_prompt), &last_prompt);

        tracing::info!(
            "all {} steps complete: total {} bytes, {} lines",
            total_steps,
            all_content.len(),
            all_content.lines().count()
        );
        let _ = tx.send(agent::retry::PipelineResult::StreamDone {
            content: all_content,
        });
        let _ = tx.send(agent::retry::PipelineResult::StreamMeta {
            context_handle: current_context,
            prompt_eval_count: None, // per-step, not easily aggregated
            eval_count: None,
            total_prompt_tokens: total_tokens,
            model,
        });
    });
}

/// Stream a single step's LLM call, returning the accumulated content.
/// On error, returns a string starting with "ERROR:".
/// Maximum number of auto-continuations when a response is truncated (done_reason: "length").
const MAX_TRUNCATION_CONTINUATIONS: usize = 3;

/// Result of a single streaming step, including KV cache metadata.
#[derive(Clone)]
struct StepResult {
    content: String,
    context_handle: Option<Vec<i64>>,
    prompt_eval_count: Option<usize>,
    eval_count: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
fn stream_single_step_generate(
    rt: &tokio::runtime::Runtime,
    http: reqwest::Client,
    endpoint: &str,
    model: &str,
    system_prompt: Option<String>,
    prompt: String,
    context_handle: Option<Vec<i64>>,
    num_ctx: u64,
    tx: &mpsc::Sender<agent::retry::PipelineResult>,
) -> StepResult {
    let tx = tx.clone();
    let mut full_content = String::new();
    let mut current_prompt = prompt;
    let mut current_context = context_handle;
    let mut final_context_handle: Option<Vec<i64>> = None;
    let mut final_prompt_eval_count: Option<usize> = None;
    let mut final_eval_count: Option<usize> = None;

    for attempt in 0..=MAX_TRUNCATION_CONTINUATIONS {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let stream = ollama::OllamaClient::generate_stream(
            http.clone(),
            endpoint.to_string(),
            model.to_string(),
            system_prompt.clone(),
            current_prompt.clone(),
            current_context.clone(),
            num_ctx,
            cancel_rx,
        );
        let mut pin = std::pin::pin!(stream);

        let chunk_result: Result<Option<String>, String> = rt.block_on(async {
            use futures::StreamExt;
            while let Some(chunk_result) = pin.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        if !chunk.response.is_empty() {
                            let _ = tx.send(agent::retry::PipelineResult::StreamChunk {
                                content: chunk.response.clone(),
                            });
                            full_content.push_str(&chunk.response);
                        }
                        if chunk.done {
                            // Capture metadata from final chunk
                            if let Some(ctx) = chunk.context {
                                final_context_handle = Some(ctx);
                            }
                            if chunk.prompt_eval_count.is_some() {
                                final_prompt_eval_count = chunk.prompt_eval_count;
                            }
                            if chunk.eval_count.is_some() {
                                final_eval_count = chunk.eval_count;
                            }
                            return Ok(chunk.done_reason);
                        }
                    }
                    Err(e) => {
                        return Err(format!("Stream error: {}", e));
                    }
                }
            }
            Ok(None)
        });

        drop(cancel_tx);

        match chunk_result {
            Ok(done_reason) => {
                let reason = done_reason.as_deref().unwrap_or("unknown");
                if reason == "length" && attempt < MAX_TRUNCATION_CONTINUATIONS {
                    tracing::warn!(
                        "response truncated (done_reason: length), continuing (attempt {}/{}, {} bytes so far)",
                        attempt + 1, MAX_TRUNCATION_CONTINUATIONS, full_content.len()
                    );
                    // Extend the prompt with the truncated response + continuation request
                    current_prompt = format!(
                        "{}\nassistant: {}\nuser: Your previous response was cut off due to length. Continue exactly from where you left off. Do not repeat what you already wrote.",
                        current_prompt, full_content
                    );
                    // Keep the context handle for cache reuse on continuation
                    current_context = final_context_handle.clone();
                    continue;
                }
                tracing::info!(
                    "stream done: reason={}, {} bytes",
                    reason,
                    full_content.len()
                );
                return StepResult {
                    content: full_content,
                    context_handle: final_context_handle,
                    prompt_eval_count: final_prompt_eval_count,
                    eval_count: final_eval_count,
                };
            }
            Err(e) => {
                return StepResult {
                    content: format!("ERROR: {}", e),
                    context_handle: None,
                    prompt_eval_count: None,
                    eval_count: None,
                };
            }
        }
    }

    tracing::warn!(
        "max continuations reached, returning {} bytes",
        full_content.len()
    );
    StepResult {
        content: full_content,
        context_handle: final_context_handle,
        prompt_eval_count: final_prompt_eval_count,
        eval_count: final_eval_count,
    }
}

/// Maximum number of tool-call rounds within a single plan step. Each round is one
/// `/api/generate` call (which itself may chain up to `MAX_TRUNCATION_CONTINUATIONS`
/// sub-calls for length-truncated responses). Five rounds covers a reasonable
/// fetch→inspect→decide sequence without risking runaway loops.
const MAX_TOOL_ROUNDS_PER_STEP: usize = 5;

/// Maximum retries when the LLM emits something that *looks* like a tool call but
/// fails to parse (e.g. missing closing tag, malformed JSON). Mirrors agent loop.
const MAX_TOOL_CORRECTION_RETRIES: usize = 2;

/// Execute one plan step with optional tool calls. Repeatedly calls
/// `stream_single_step_generate` (the `/api/generate` primitive that owns truncation
/// continuation and KV-cache context-handle threading). After each round, parses the
/// assistant output for text-format `<tool_call>` blocks. If found, dispatches them via
/// the `ToolRegistry`, appends `<tool_result>` blocks to the prompt, and re-calls with
/// the same context handle so Ollama prefix-matches the cached KV tensors and only
/// evaluates the delta. Terminates when the LLM emits no tool calls (and no failed
/// attempts), or when the round / retry / signature-repeat caps are hit.
///
/// The low-level primitive `stream_single_step_generate` stays unchanged — this
/// wrapper orchestrates tool round-trips on top of it.
#[allow(clippy::too_many_arguments)]
fn stream_step_with_tools(
    rt: &tokio::runtime::Runtime,
    http: reqwest::Client,
    endpoint: &str,
    model: &str,
    system_prompt: Option<String>,
    initial_prompt: String,
    context_handle: Option<Vec<i64>>,
    num_ctx: u64,
    tools: &tools::ToolRegistry,
    tx: &mpsc::Sender<agent::retry::PipelineResult>,
) -> StepResult {
    let mut current_prompt = initial_prompt;
    let mut current_context = context_handle;
    let mut last_step_result: Option<StepResult> = None;
    let mut correction_retries = 0usize;
    let mut prev_signature = String::new();

    for round in 0..MAX_TOOL_ROUNDS_PER_STEP {
        let step_result = stream_single_step_generate(
            rt,
            http.clone(),
            endpoint,
            model,
            system_prompt.clone(),
            current_prompt.clone(),
            current_context.clone(),
            num_ctx,
            tx,
        );

        // Surface transport-level errors immediately (content starts with "ERROR:").
        if step_result.content.starts_with("ERROR:") {
            return step_result;
        }

        let content = step_result.content.clone();
        let parse_result =
            crate::agent::tools_parser::parse_tool_calls_with_diagnostics(&content);

        // No parseable tool calls — decide whether to retry or finalize.
        if parse_result.calls.is_empty() {
            if parse_result.is_failed_attempt() && correction_retries < MAX_TOOL_CORRECTION_RETRIES
            {
                correction_retries += 1;
                tracing::warn!(
                    "stream_step_with_tools round {}: malformed tool call, retrying ({}/{}) — diagnostics: {}",
                    round,
                    correction_retries,
                    MAX_TOOL_CORRECTION_RETRIES,
                    parse_result.diagnostics.format_for_correction()
                );
                // Re-prompt for correction without changing context_handle. The cached
                // prefix still matches; only the appended text is new.
                current_prompt = format!(
                    "{}\n\nuser: Your previous tool call could not be parsed. {}. \
                     Emit a valid <tool_call name=\"...\" call_id=\"...\">{{...}}</tool_call> block, \
                     or finish the step without a tool call.",
                    content,
                    parse_result.diagnostics.format_for_correction()
                );
                continue;
            }
            // No tool calls and not a recoverable failed attempt — step complete.
            tracing::info!(
                "stream_step_with_tools round {}: no tool calls, step complete (chars={})",
                round,
                content.chars().count()
            );
            return step_result;
        }

        // Valid tool calls — reset retry counter, check for infinite loop.
        correction_retries = 0;
        let tool_calls = parse_result.calls;
        let current_sig = tool_calls
            .iter()
            .map(|c| format!("{}:{}", c.name, c.parameters))
            .collect::<Vec<_>>()
            .join("|");
        if current_sig == prev_signature {
            tracing::warn!(
                "stream_step_with_tools round {}: repeating tool signature, finalizing step",
                round
            );
            return step_result;
        }
        prev_signature = current_sig;

        // Preserve this round's metadata in case we exhaust MAX_TOOL_ROUNDS_PER_STEP.
        last_step_result = Some(step_result.clone());
        current_context = step_result.context_handle.clone();

        // Dispatch each tool call.
        let mut tool_result_blocks: Vec<String> = Vec::new();
        for call in &tool_calls {
            tracing::info!(
                "stream_step_with_tools round {}: dispatching tool={} params={:?}",
                round,
                call.name,
                call.parameters
            );
            // Validate name.
            if !tools.has_tool(&call.name) {
                let err = tools::ToolResult::err(
                    &call.name,
                    &call.call_id,
                    format!(
                        "Unknown tool '{}'. Available tools: {}",
                        call.name,
                        tools.list_names().join(", ")
                    ),
                );
                let _ = tx.send(agent::retry::PipelineResult::ToolStart {
                    tool_name: call.name.clone(),
                    call_id: call.call_id.clone(),
                });
                let _ = tx.send(agent::retry::PipelineResult::ToolResultReady {
                    result: err.clone(),
                });
                tool_result_blocks.push(format_tool_result_block(&err));
                continue;
            }
            // Validate required params.
            if let Ok(validation) = tools.validate_params(&call.name, &call.parameters) {
                if !validation.is_empty() {
                    let err = tools::ToolResult::err(&call.name, &call.call_id, validation);
                    let _ = tx.send(agent::retry::PipelineResult::ToolStart {
                        tool_name: call.name.clone(),
                        call_id: call.call_id.clone(),
                    });
                    let _ = tx.send(agent::retry::PipelineResult::ToolResultReady {
                        result: err.clone(),
                    });
                    tool_result_blocks.push(format_tool_result_block(&err));
                    continue;
                }
            }
            // Dispatch.
            let _ = tx.send(agent::retry::PipelineResult::ToolStart {
                tool_name: call.name.clone(),
                call_id: call.call_id.clone(),
            });
            let result = tools
                .get(&call.name)
                .unwrap()
                .execute(call.parameters.clone(), call.call_id.clone())
                .unwrap_or_else(|e| {
                    tools::ToolResult::err(&call.name, &call.call_id, format!("{}", e))
                });
            tracing::info!(
                "stream_step_with_tools round {}: tool {} result success={} output_chars={}",
                round,
                call.name,
                result.success,
                result.output.chars().count()
            );
            let _ = tx.send(agent::retry::PipelineResult::ToolResultReady {
                result: result.clone(),
            });
            tool_result_blocks.push(format_tool_result_block(&result));
        }

        // Build next-round prompt: append verbatim assistant output + tool_result blocks
        // + continuation instruction. Ollama prefix-matches the cached KV tensors against
        // the prior (prompt + assistant output) and evaluates only the tool_result delta.
        let mut appended = String::new();
        appended.push('\n');
        appended.push_str(&content);
        for block in &tool_result_blocks {
            appended.push_str(block);
        }
        appended.push_str(
            "\nuser: Continue the step using the tool results above. Emit another <tool_call> block if more tools are needed, otherwise finish the step.\n",
        );
        current_prompt = format!("{}{}", current_prompt, appended);
    }

    tracing::warn!(
        "stream_step_with_tools: hit MAX_TOOL_ROUNDS_PER_STEP={}, returning last step result",
        MAX_TOOL_ROUNDS_PER_STEP
    );
    last_step_result.unwrap_or(StepResult {
        content: String::new(),
        context_handle: current_context,
        prompt_eval_count: None,
        eval_count: None,
    })
}

/// Format a `ToolResult` as a `<tool_result>` block for inclusion in the next `/api/generate`
/// prompt. The JSON payload shape mirrors what the agent loop uses; the closing-tag
/// escape handles the rare case where fetched HTML/Markdown contains the literal sequence.
fn format_tool_result_block(result: &tools::ToolResult) -> String {
    let escaped_output = result.output.replace("</tool_result>", "</tool_result_");
    let payload = if result.success {
        serde_json::json!({ "success": true, "output": escaped_output })
    } else {
        serde_json::json!({ "success": false, "output": escaped_output, "error": "tool execution failed" })
    };
    format!(
        "\n<tool_result tool=\"{}\" call_id=\"{}\">{}</tool_result>",
        result.tool_name, result.call_id, payload
    )
}

/// Build a flat prompt string from conversation history for `/api/generate`.
/// Extracts system messages and returns (system_prompt, user_prompt).
fn build_generate_prompt(
    history: &[ConversationMessage],
    current_input: &str,
) -> (Option<String>, String) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut conversation_parts: Vec<String> = Vec::new();

    for msg in history {
        if msg.role == "system" {
            system_parts.push(msg.content.clone());
        } else {
            conversation_parts.push(format!("{}: {}", msg.role, msg.content));
        }
    }
    conversation_parts.push(format!("user: {}", current_input));

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };
    let prompt = conversation_parts.join("\n");

    (system, prompt)
}

/// Estimate total tokens in a generate prompt.
fn estimate_prompt_tokens(system: Option<&str>, prompt: &str) -> usize {
    let mut total = util::text::estimate_tokens(prompt);
    if let Some(sys) = system {
        total += util::text::estimate_tokens(sys);
    }
    total
}

/// Spawn a skill-based request on a background thread.
fn spawn_skill_request(
    app_state: &AppState,
    _client: &ollama::OllamaClient,
    skill: &skills::Skill,
    args: &str,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    let model = app_state.config.exec_model.clone();
    if model.is_empty() {
        let _ = tx.send(agent::retry::PipelineResult::Retry(
            agent::retry::RetryResult::Failed {
                last_error: "No core model configured. Run setup or edit ~/.litepilot/config.toml"
                    .into(),
                attempts: 0,
            },
        ));
        return;
    }

    if args.is_empty() {
        let _ = tx.send(agent::retry::PipelineResult::Retry(
            agent::retry::RetryResult::Failed {
                last_error: format!("Usage: /{} <question or description>", skill.name),
                attempts: 0,
            },
        ));
        return;
    }

    // Build system prompt via PromptBuilder + skill content
    let base_prompt = app_state.prompt_builder.build();
    let system_prompt = format!("{}\n\n{}", base_prompt, skill.content);
    let max_retries = app_state.config.max_retries;
    let endpoint = app_state.config.ollama_endpoint.clone();
    let connect_timeout = app_state.config.connect_timeout;
    let context_window_limit = app_state.config.context_window_limit;
    let user_message = args.to_string();
    let kind = if skill.name == "simplify" || skill.name == "review" || skill.name == "test" {
        agent::retry::ResponseKind::CodeImplementation
    } else {
        agent::retry::ResponseKind::Chat
    };
    let think = app_state.think_enabled;

    tracing::info!(
        "spawn_skill_request: skill={}, model={}, kind={:?}, max_retries={}, think={}",
        skill.name,
        model,
        kind,
        max_retries,
        think
    );

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: format!("Runtime error: {}", e),
                        attempts: 0,
                    },
                ));
                return;
            }
        };

        let bg_config = config::Config {
            ollama_endpoint: endpoint,
            connect_timeout,
            context_window_limit,
            ..config::Config::default()
        };
        let bg_client = match ollama::OllamaClient::new(&bg_config) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::Retry(
                    agent::retry::RetryResult::Failed {
                        last_error: format!("Client error: {}", e),
                        attempts: 0,
                    },
                ));
                return;
            }
        };

        let result = rt.block_on(agent::retry::chat_with_retry(
            &bg_client,
            &model,
            &system_prompt,
            &user_message,
            kind,
            max_retries,
            think,
        ));
        let _ = tx.send(agent::retry::PipelineResult::Retry(result));
    });
}

fn render_retry_result(ui_state: &mut ui::UiState, result: agent::retry::RetryResult) {
    let content = match &result {
        agent::retry::RetryResult::Success { content, attempts } => {
            if *attempts > 0 {
                ui_state.add_output(OutputLine::System(format!(
                    "Got valid response after {} retries",
                    attempts
                )));
            }
            content.clone()
        }
        agent::retry::RetryResult::Exhausted {
            content,
            attempts,
            corrections,
        } => {
            ui_state.add_output(OutputLine::Error(format!(
                "Response still invalid after {} retries. Showing last attempt:",
                attempts
            )));
            for (_, reason) in corrections {
                ui_state.add_output(OutputLine::Error(format!("  - {}", reason)));
            }
            content.clone()
        }
        agent::retry::RetryResult::Failed { last_error, .. } => {
            ui_state.add_output(OutputLine::Error(last_error.clone()));
            return;
        }
    };

    // Display the assistant response
    ui_state.add_output(OutputLine::Assistant(content.clone()));

    // If the response contains file changes, present them for review
    let changes = agent::AgentPipeline::parse_file_changes(&content);
    if !changes.is_empty() {
        tracing::info!(
            "retry path: detected {} file change(s) in response",
            changes.len()
        );
        for change in &changes {
            tracing::debug!(
                "  retry file: {} {} ({} bytes)",
                change.action,
                change.path.display(),
                change.content.len()
            );
        }
        ui_state.add_output(OutputLine::System(format!(
            "Detected {} file change(s):",
            changes.len()
        )));
        for change in &changes {
            let action_icon = match change.action.as_str() {
                "create" => "+",
                "modify" => "~",
                "delete" => "-",
                _ => "?",
            };
            ui_state.add_output(OutputLine::System(format!(
                "  {} {} ({})",
                action_icon,
                change.path.display(),
                change.action
            )));

            // Show diff preview for modifications
            if change.action == "modify" {
                let full_path = ui_state.workspace_hint.join(&change.path);
                if full_path.exists() {
                    if let Ok(old_content) = std::fs::read_to_string(&full_path) {
                        let diff = util::diff::generate_unified_diff(
                            &old_content,
                            &change.content,
                            &change.path.to_string_lossy(),
                        );
                        let removed: Vec<String> = diff
                            .lines()
                            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
                            .map(|l| l.to_string())
                            .collect();
                        let added: Vec<String> = diff
                            .lines()
                            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
                            .map(|l| l.to_string())
                            .collect();
                        ui_state.add_output(OutputLine::Diff { added, removed });
                    }
                }
            }
        }
        ui_state.add_output(OutputLine::System(
            "Type /apply to write these files, or ignore to discard.".into(),
        ));
    }
}

/// Spawn a background eval review using the eval model.
/// After execution completes, the eval model reviews whether the implementation
/// fulfills the user's original request.
fn spawn_eval_review(
    app_state: &AppState,
    user_request: &str,
    exec_output: &str,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    let eval_model = app_state.config.effective_eval_model().to_string();
    let exec_model = app_state.config.exec_model.clone();
    if eval_model == exec_model || eval_model.is_empty() {
        return;
    }

    let endpoint = app_state.config.ollama_endpoint.clone();
    let connect_timeout = app_state.config.connect_timeout;
    let context_window_limit = app_state.config.context_window_limit;
    let user_request = user_request.to_string();
    let exec_output = exec_output.to_string();

    tracing::info!(
        "spawning eval review: eval_model={}, exec_output={} bytes",
        eval_model,
        exec_output.len()
    );

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("eval review runtime error: {}", e);
                return;
            }
        };

        let bg_config = config::Config {
            ollama_endpoint: endpoint,
            connect_timeout,
            context_window_limit,
            ..config::Config::default()
        };
        let client = match ollama::OllamaClient::new(&bg_config) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("eval review client error: {}", e);
                return;
            }
        };

        // Truncate exec output if too long for eval context
        let max_output_chars = 8000;
        let truncated_output = if exec_output.len() > max_output_chars {
            format!(
                "{}\n\n[...truncated, {} chars omitted]",
                &exec_output[..max_output_chars],
                exec_output.len() - max_output_chars
            )
        } else {
            exec_output.clone()
        };

        let messages = vec![
            ollama::chat::ChatMessage::system(
                "You are a code review evaluator. Your job is to assess whether \
                 an implementation fulfills the user's original request. \
                 Be concise. Start with PASS or FAIL, then list any issues or \
                 missing parts. Keep your review under 500 words."
            ),
            ollama::chat::ChatMessage::user(format!(
                "User request:\n{}\n\nImplementation output:\n{}\n\n\
                 Does this implementation fulfill the user's request? \
                 Start with PASS or FAIL.",
                user_request, truncated_output
            )),
        ];

        match rt.block_on(client.chat(&eval_model, messages, false)) {
            Ok(resp) => {
                let _ = tx.send(agent::retry::PipelineResult::EvalReady {
                    evaluation: format!("[Eval: {}]\n{}", eval_model, resp.content),
                });
            }
            Err(e) => {
                tracing::warn!("eval review failed: {}", e);
                let _ = tx.send(agent::retry::PipelineResult::EvalReady {
                    evaluation: format!("[Eval] Review skipped: {}", e),
                });
            }
        }
    });
}

fn spawn_request_for_mode(
    app_state: &mut AppState,
    client: &ollama::OllamaClient,
    input: &str,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    // Pre-turn snapshot (non-fatal)
    if let Err(e) = app_state.snapshot_manager.pre_turn(input) {
        tracing::debug!("pre-turn snapshot skipped: {}", e);
    }

    // Emit structured event
    app_state.event_sink.turn_started(
        &app_state.config.exec_model,
        &app_state.mode.to_string(),
        input.len(),
    );

    // Update PromptBuilder with current environment before spawning
    app_state
        .prompt_builder
        .update_environment(prompt::EnvironmentBlock::capture(&app_state.workspace));
    app_state.prompt_builder.set_volatile(
        app_state.working_set.summary(),
        app_state.conversation_summary.clone(),
    );
    // Set the current goal from user input — re-injected at prompt edges for small models
    app_state.prompt_builder.set_current_goal(input);

    tracing::info!(
        "request routing: mode={}, pipeline=plan_then_execute (single-path architecture)",
        app_state.mode,
    );
    tracing::debug!("input for routing: {:?}", input);

    // Single execution pipeline: plan-then-execute handles every request, including
    // tool-using ones. The planner sees tool descriptions in its prompt; the executor
    // parses text-format <tool_call> blocks and dispatches via ToolRegistry. Auto/Plan/
    // Edit modes all flow through here, with mode-specific behavior enforced inside
    // spawn_plan_then_execute / spawn_execution_with_plan.
    spawn_plan_then_execute(app_state, client, input, tx);
}

/// Spawn the full plan→implement→audit pipeline on a background thread.
#[allow(dead_code)]
fn spawn_auto_pipeline(
    app_state: &AppState,
    input: &str,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    let workspace = app_state.workspace.clone();
    let endpoint = app_state.config.ollama_endpoint.clone();
    let connect_timeout = app_state.config.connect_timeout;
    let max_retries = app_state.config.max_retries;
    let input = input.to_string();
    let web_search_enabled = app_state.web_search_enabled;
    let max_search_tokens = app_state.config.max_search_context_tokens;

    // Clone config values needed by the background thread
    let exec_model = app_state.config.exec_model.clone();
    let eval_model = app_state.config.effective_eval_model().to_string();

    tracing::info!(
        "spawn_auto_pipeline: exec={}, eval={}, max_retries={}, web_search={}",
        exec_model,
        eval_model,
        max_retries,
        web_search_enabled
    );

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::AutoFailed {
                    error: format!("Runtime error: {}", e),
                });
                return;
            }
        };

        // Run web search if enabled
        let user_message = if web_search_enabled {
            let search_ctx = rt.block_on(run_web_search(&input, max_search_tokens));
            if !search_ctx.is_empty() {
                let _ = tx.send(agent::retry::PipelineResult::SearchDone {
                    count: search_ctx.lines().filter(|l| l.starts_with('[')).count(),
                    context: search_ctx.clone(),
                });
            }
            format!("{}\n{}", search_ctx, input)
        } else {
            input
        };

        let bg_config = config::Config {
            ollama_endpoint: endpoint,
            connect_timeout,
            exec_model,
            eval_model,
            max_retries,
            ..config::Config::default()
        };
        let bg_client = match ollama::OllamaClient::new(&bg_config) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(agent::retry::PipelineResult::AutoFailed {
                    error: format!("Client error: {}", e),
                });
                return;
            }
        };
        let sandbox = sandbox::Sandbox::new(workspace.clone());

        let result = rt.block_on(agent::auto_run::run_auto_pipeline(
            &bg_client,
            &bg_config,
            &sandbox,
            workspace,
            &user_message,
            "",   // no project context for now
            None, // no codebase for now
        ));

        match result {
            Ok(changes) => {
                tracing::info!("auto pipeline produced {} file change(s)", changes.len());
                for c in &changes {
                    tracing::info!(
                        "  auto: {} {} ({} bytes)",
                        c.action,
                        c.path.display(),
                        c.content.len()
                    );
                }
                let applied: Vec<String> = changes
                    .iter()
                    .map(|c| format!("{} ({})", c.path.display(), c.action))
                    .collect();
                let _ = tx.send(agent::retry::PipelineResult::AutoSuccess { changes, applied });
            }
            Err(e) => {
                tracing::error!("auto pipeline failed: {}", e);
                let _ = tx.send(agent::retry::PipelineResult::AutoFailed {
                    error: e.to_string(),
                });
            }
        }
    });
}

/// Render auto pipeline results in the chat panel.
fn render_auto_result(
    ui_state: &mut ui::UiState,
    changes: &[agent::FileChange],
    applied: &[String],
) {
    if changes.is_empty() {
        ui_state.add_output(OutputLine::System(
            "Pipeline completed but produced no changes.".into(),
        ));
        return;
    }

    ui_state.add_output(OutputLine::System(format!(
        "Auto pipeline: {} file(s) generated and applied.",
        applied.len()
    )));
    for path in applied {
        ui_state.add_output(OutputLine::System(format!("  + {}", path)));
    }
    for change in changes {
        // Show content summary
        let line_count = change.content.lines().count();
        ui_state.add_output(OutputLine::Assistant(format!(
            "### FILE: {}\n### ACTION: {}\n```{}\n```\n",
            change.path.display(),
            change.action,
            if line_count > 20 {
                format!(
                    "{} ({} lines)",
                    &change
                        .content
                        .lines()
                        .take(20)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    line_count
                )
            } else {
                change.content.clone()
            }
        )));
    }
}

/// Handle /run <command> — execute a shell command via the sandbox.
/// Returns current date and time as a human-readable string for LLM context.
fn current_datetime() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M %A").to_string()
}

/// Replace {MAX_LINES} placeholder in prompts with the configured limit.
/// Check if conversation summarization is needed and spawn a background task.
fn maybe_trigger_summarization(
    app_state: &AppState,
    tx: mpsc::Sender<agent::retry::PipelineResult>,
) {
    let context_window = app_state.config.context_window_limit as usize;
    if !agent::summarizer::needs_summarization(
        &app_state.conversation_history,
        context_window,
        &app_state.summarizer_config,
    ) {
        return;
    }

    let exec_model = app_state.config.exec_model.clone();
    if exec_model.is_empty() {
        return;
    }

    let endpoint = app_state.config.ollama_endpoint.clone();
    let connect_timeout = app_state.config.connect_timeout;
    let context_window_limit = app_state.config.context_window_limit;
    let history = app_state.conversation_history.clone();
    let config = app_state.summarizer_config.clone();

    tracing::info!("spawning background summarization task");
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("summarization runtime error: {}", e);
                return;
            }
        };
        let bg_config = config::Config {
            ollama_endpoint: endpoint,
            connect_timeout,
            context_window_limit,
            ..config::Config::default()
        };
        let client = match ollama::OllamaClient::new(&bg_config) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("summarization client error: {}", e);
                return;
            }
        };
        match rt.block_on(agent::summarizer::summarize(
            &client,
            &exec_model,
            &history,
            &config,
        )) {
            Ok(result) => {
                let _ = tx.send(agent::retry::PipelineResult::SummaryReady {
                    summary: result.summary,
                    summarized_count: result.summarized_count,
                });
            }
            Err(e) => {
                tracing::warn!("summarization failed (non-fatal): {}", e);
            }
        }
    });
}

fn apply_prompt_limits(prompt: &str, max_file_lines: usize) -> String {
    prompt.replace("{MAX_LINES}", &max_file_lines.to_string())
}

/// Substitute `{MAX_LINES}` and `{TOOLS}` placeholders in a planner system prompt.
/// `{TOOLS}` is the prose listing of available tools (name + description per line)
/// from `ToolRegistry::descriptions_text()`. Kept as a sibling to
/// `apply_prompt_limits` so callers that don't need tool interpolation are unaffected.
fn apply_plan_prompt(prompt: &str, max_file_lines: usize, tools_description: &str) -> String {
    apply_prompt_limits(prompt, max_file_lines).replace("{TOOLS}", tools_description)
}

fn handle_run_command(app_state: &mut AppState, ui_state: &mut ui::UiState, cmd: &str) {
    if cmd.is_empty() {
        ui_state.add_output(OutputLine::Error("Usage: /run <command>".into()));
        return;
    }

    if !app_state.mode.can_execute_command() {
        ui_state.add_output(OutputLine::Error(
            "Command execution not allowed in Plan mode.".into(),
        ));
        return;
    }

    // Parse command into binary + args
    let parts: Vec<String> = cmd.split_whitespace().map(String::from).collect();
    if parts.is_empty() {
        ui_state.add_output(OutputLine::Error("Empty command.".into()));
        return;
    }

    let sandbox = sandbox::Sandbox::new(app_state.workspace.clone());
    let executor = sandbox::executor::Executor::new(&sandbox);

    let bin = parts[0].clone();
    let args: Vec<String> = parts[1..].to_vec();

    // Validate command against sandbox rules
    if let Err(e) = sandbox.validate_command(&bin, &args) {
        ui_state.add_output(OutputLine::Error(format!("Blocked: {}", e)));
        return;
    }

    tracing::info!("run: {} {}", bin, args.join(" "));
    ui_state.add_output(OutputLine::System(format!(
        "Running: {} {}",
        bin,
        args.join(" ")
    )));

    let result = std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Runtime::new().ok()?;
            rt.block_on(executor.run(&bin, &args, None)).ok()
        })
        .join()
        .ok()?
    });

    match result {
        Some(output) => {
            if output.success {
                tracing::info!(
                    "run success: {} {} (exit=0, stdout={} bytes)",
                    bin,
                    args.join(" "),
                    output.stdout.len()
                );
                if !output.stdout.is_empty() {
                    ui_state.add_output(OutputLine::System(output.stdout.trim_end().to_string()));
                }
                ui_state.add_output(OutputLine::System("Done.".into()));
            } else {
                tracing::warn!(
                    "run failed: {} {} (exit={:?})",
                    bin,
                    args.join(" "),
                    output.exit_code
                );
                ui_state.add_output(OutputLine::Error(format!(
                    "Exit {}: {}",
                    output.exit_code.unwrap_or(1),
                    if output.stderr.is_empty() {
                        output.stdout.trim_end()
                    } else {
                        output.stderr.trim_end()
                    }
                )));
            }
        }
        None => {
            tracing::error!("run failed: {} {} (thread panicked)", bin, args.join(" "));
            ui_state.add_output(OutputLine::Error("Failed to execute command.".into()));
        }
    }
}

/// Parse ```bash ... ``` blocks from LLM response content.
fn parse_bash_blocks(content: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_bash = false;
    let mut buffer = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if in_bash {
            if trimmed == "```" {
                in_bash = false;
                let cmds = buffer.trim().to_string();
                if !cmds.is_empty() {
                    commands.push(cmds);
                }
                buffer.clear();
            } else {
                // Skip comment-only lines
                let stripped = trimmed.trim_start_matches('#').trim();
                if !stripped.is_empty() && !trimmed.starts_with('#') {
                    buffer.push_str(trimmed);
                    buffer.push('\n');
                }
            }
        } else if trimmed == "```bash" || trimmed == "```sh" || trimmed == "```shell" {
            in_bash = true;
            buffer.clear();
        }
    }

    commands
}

/// Execute bash blocks extracted from the LLM response.
/// Uses `/bin/sh -c` so shell features (redirects, pipes, heredocs) work correctly.
fn execute_bash_blocks(app_state: &AppState, ui_state: &mut ui::UiState, content: &str) {
    let blocks = parse_bash_blocks(content);
    if blocks.is_empty() {
        return;
    }

    tracing::info!("auto-executing {} bash block(s) in Auto mode", blocks.len());

    let sandbox = sandbox::Sandbox::new(app_state.workspace.clone());

    for block in &blocks {
        // Validate the first command word in each line against sandbox rules
        for cmd_line in block.lines() {
            let cmd_line = cmd_line.trim();
            if cmd_line.is_empty() {
                continue;
            }
            let first_word = cmd_line.split_whitespace().next().unwrap_or("");
            if let Err(e) = sandbox.validate_command(first_word, &[]) {
                ui_state.add_output(OutputLine::Error(format!("Blocked: {} — {}", cmd_line, e)));
                continue;
            }
        }

        ui_state.add_output(OutputLine::System(format!("$ {}", block.lines().next().unwrap_or(""))));

        // Run the entire block through a shell so redirects/pipes/heredocs work
        let workspace = app_state.workspace.clone();
        let block_owned = block.to_string();
        let result = std::thread::scope(|s| {
            s.spawn(|| {
                let output = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(&block_owned)
                    .current_dir(&workspace)
                    .output();
                output.ok()
            })
            .join()
            .ok()?
        });

        match result {
            Some(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim_end().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim_end().to_string();
                if output.status.success() {
                    if !stdout.is_empty() {
                        ui_state.add_output(OutputLine::System(stdout));
                    }
                } else {
                    ui_state.add_output(OutputLine::Error(format!(
                        "Exit {}: {}",
                        output.status.code().unwrap_or(1),
                        if stderr.is_empty() { &stdout } else { &stderr }
                    )));
                }
            }
            None => {
                ui_state.add_output(OutputLine::Error(format!("Failed: {}", block)));
            }
        }
    }
}

/// Write a FileChange to disk, with sandbox validation.
/// Run web search and return formatted context string.
/// Returns empty string if search fails or is disabled.
async fn run_web_search(query: &str, max_tokens: usize) -> String {
    tracing::info!("web search: query={:?}, max_tokens={}", query, max_tokens);
    let config = config::Config::default();
    let engine = search::SearchEngine::new(&config);
    match engine.search(query, max_tokens).await {
        Ok(results) if !results.is_empty() => {
            tracing::info!("web search returned {} result(s)", results.len());
            search::format_search_context(&results)
        }
        Ok(_) => {
            tracing::warn!("web search returned 0 results for query={:?}", query);
            String::new()
        }
        Err(e) => {
            tracing::warn!("web search failed for query={:?}: {}", query, e);
            String::new()
        }
    }
}

/// Handle /uv subcommands: init, venv, add <pkg>, run <script>.
fn handle_uv_command(app_state: &mut AppState, ui_state: &mut ui::UiState, args: &str) {
    tracing::info!("uv command: {:?}", args);
    if !project::uv::UvManager::is_available() {
        ui_state.add_output(OutputLine::Error(
            "uv is not installed. Install it: https://docs.astral.sh/uv/".into(),
        ));
        return;
    }

    if !app_state.mode.can_execute_command() {
        ui_state.add_output(OutputLine::Error(
            "Command execution not allowed in Plan mode.".into(),
        ));
        return;
    }

    let (subcmd, rest) = match args.split_once(' ') {
        Some((s, r)) => (s, r.trim()),
        None => (args, ""),
    };

    let sandbox = sandbox::Sandbox::new(app_state.workspace.clone());
    let workspace = app_state.workspace.clone();

    let result = match subcmd {
        "init" => {
            ui_state.add_output(OutputLine::System("Running uv init...".into()));
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = tokio::runtime::Runtime::new().ok()?;
                    rt.block_on(project::uv::UvManager::init(&workspace, &sandbox))
                        .ok()
                })
                .join()
                .ok()?
            })
        }
        "venv" => {
            ui_state.add_output(OutputLine::System("Running uv venv...".into()));
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = tokio::runtime::Runtime::new().ok()?;
                    rt.block_on(project::uv::UvManager::create_venv(&workspace, &sandbox))
                        .ok()
                })
                .join()
                .ok()?
            })
        }
        "add" => {
            if rest.is_empty() {
                ui_state.add_output(OutputLine::Error("Usage: /uv add <package>".into()));
                return;
            }
            ui_state.add_output(OutputLine::System(format!("Running uv add {}...", rest)));
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = tokio::runtime::Runtime::new().ok()?;
                    rt.block_on(project::uv::UvManager::add(&workspace, rest, &sandbox))
                        .ok()
                })
                .join()
                .ok()?
            })
        }
        "run" => {
            if rest.is_empty() {
                ui_state.add_output(OutputLine::Error("Usage: /uv run <script>".into()));
                return;
            }
            ui_state.add_output(OutputLine::System(format!("Running uv run {}...", rest)));
            std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = tokio::runtime::Runtime::new().ok()?;
                    rt.block_on(project::uv::UvManager::run(&workspace, rest, &sandbox))
                        .ok()
                })
                .join()
                .ok()?
            })
        }
        _ => {
            ui_state.add_output(OutputLine::Error(format!(
                "Unknown /uv subcommand: {}. Use: init, venv, add, run",
                subcmd
            )));
            return;
        }
    };

    match result {
        Some(output) => {
            if output.success {
                if !output.stdout.is_empty() {
                    ui_state.add_output(OutputLine::System(output.stdout));
                }
                ui_state.add_output(OutputLine::System("Done.".into()));
            } else {
                ui_state.add_output(OutputLine::Error(format!(
                    "uv {} failed: {}",
                    subcmd,
                    if output.stderr.is_empty() {
                        &output.stdout
                    } else {
                        &output.stderr
                    }
                )));
            }
        }
        None => {
            ui_state.add_output(OutputLine::Error("Failed to execute uv command.".into()));
        }
    }
}

/// Auto-save session to disk (non-blocking, logs errors but doesn't crash).
fn auto_save_session(app_state: &AppState) {
    if let Err(e) = session::persistence::save_session(&app_state.current_session) {
        tracing::warn!("failed to auto-save session: {}", e);
    }
}

/// Install a panic hook that writes crash dumps to ~/.litepilot/crashes/.
fn install_crash_handler() {
    let crashes_dir = config::Config::crashes_dir();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(dir) = crashes_dir.as_ref() {
            let _ = std::fs::create_dir_all(dir);
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let path = dir.join(format!("crash_{}.log", timestamp));
            let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());
            let report = format!(
                "LitePilot Crash Report\n\
                 Time: {}\n\
                 Location: {}\n\
                 Message: {}\n",
                chrono::Utc::now().to_rfc3339(),
                location,
                payload
            );
            let _ = std::fs::write(&path, &report);
            eprintln!("Crash dump written to {}", path.display());
        }
    }));
}

fn pending_action_risk(action: &app::PendingAction) -> approval::RiskLevel {
    match action {
        app::PendingAction::DeleteFile { .. } => approval::RiskLevel::Destructive,
        app::PendingAction::WriteFile { .. } => approval::RiskLevel::Write,
        app::PendingAction::ExecuteCommand { cmd, args } => {
            let arg_strs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            approval::classify_command(cmd, &arg_strs)
        }
    }
}

/// Auto-apply all file changes (Auto mode — no confirmation needed).
fn auto_apply_changes(
    app_state: &mut AppState,
    ui_state: &mut ui::UiState,
    changes: &[agent::FileChange],
    result_tx: &mpsc::Sender<agent::retry::PipelineResult>,
) {
    tracing::info!(
        "auto_apply_changes: {} file(s) to apply in {} mode",
        changes.len(),
        app_state.mode
    );
    let sandbox = sandbox::Sandbox::new(app_state.workspace.clone());
    let file_ops = project::file_ops::FileOps::new(&sandbox, app_state.mode);

    for change in changes {
        let full_path = app_state.workspace.join(&change.path);
        let fc = match if change.action == "delete" {
            file_ops.prepare_delete(&full_path)
        } else {
            file_ops.prepare_write(&full_path, &change.content)
        } {
            Ok(fc) => fc,
            Err(e) => {
                ui_state.add_output(OutputLine::Error(format!(
                    "Blocked {}: {}",
                    change.path.display(),
                    e
                )));
                continue;
            }
        };

        match file_ops.apply_change(&fc) {
            Ok(()) => {
                ui_state.add_output(OutputLine::System(format!(
                    "{} {}",
                    if change.action == "delete" {
                        "Deleted"
                    } else {
                        "Wrote"
                    },
                    change.path.display()
                )));
                if change.action != "delete" {
                    run_syntax_check(ui_state, &full_path, &sandbox);
                    run_lsp_diagnostics(ui_state, &full_path, &app_state.workspace);
                }
            }
            Err(e) => {
                ui_state.add_output(OutputLine::Error(format!(
                    "Failed {} {}: {}",
                    change.action,
                    change.path.display(),
                    e
                )));
            }
        }
    }

    // Run structured diagnostics on written files and send results
    let written_paths: Vec<std::path::PathBuf> = changes
        .iter()
        .filter(|c| c.action != "delete")
        .map(|c| app_state.workspace.join(&c.path))
        .collect();

    if !written_paths.is_empty() {
        let sandbox = sandbox::Sandbox::new(app_state.workspace.clone());
        let tx = result_tx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                agent::diagnostics::run_diagnostics(&written_paths, &sandbox).await
            });
            let _ = tx.send(agent::retry::PipelineResult::DiagnosticReady { result });
        });
    }
}

/// Enter file-by-file confirmation flow (Edit mode — y/n/a).
fn enter_file_confirmation(
    app_state: &mut AppState,
    ui_state: &mut ui::UiState,
    changes: &[agent::FileChange],
) {
    tracing::info!(
        "enter_file_confirmation: {} file(s) for review",
        changes.len()
    );
    for change in changes {
        tracing::debug!(
            "  confirm: {} {} ({} bytes)",
            change.action,
            change.path.display(),
            change.content.len()
        );
    }
    let sandbox = sandbox::Sandbox::new(app_state.workspace.clone());
    let file_ops = project::file_ops::FileOps::new(&sandbox, app_state.mode);

    app_state.clear_pending();
    let mut queued = 0;

    for change in changes {
        let full_path = app_state.workspace.join(&change.path);
        let path_str = change.path.display().to_string();

        let fc = match if change.action == "delete" {
            file_ops.prepare_delete(&full_path)
        } else {
            file_ops.prepare_write(&full_path, &change.content)
        } {
            Ok(fc) => fc,
            Err(e) => {
                ui_state.add_output(OutputLine::Error(format!("Blocked {}: {}", path_str, e)));
                continue;
            }
        };

        // Show diff preview
        if !fc.diff_preview.is_empty() {
            ui_state.add_output(OutputLine::System(format!(
                "--- {} ({})",
                change.path.display(),
                change.action
            )));
            for line in fc.diff_preview.lines() {
                if line.starts_with('-') && !line.starts_with("---") {
                    ui_state.add_output(OutputLine::Diff {
                        added: vec![],
                        removed: vec![line.to_string()],
                    });
                } else if line.starts_with('+') && !line.starts_with("+++") {
                    ui_state.add_output(OutputLine::Diff {
                        added: vec![line.to_string()],
                        removed: vec![],
                    });
                }
            }
        } else {
            ui_state.add_output(OutputLine::System(format!(
                "{} {}",
                change.action,
                change.path.display()
            )));
        }

        let action = if change.action == "delete" {
            app::PendingAction::DeleteFile { path: full_path }
        } else {
            app::PendingAction::WriteFile {
                path: full_path,
                content: change.content.clone(),
                diff_preview: fc.diff_preview,
            }
        };
        app_state.push_pending(action);
        queued += 1;
    }

    if queued > 0 {
        app_state.awaiting_confirmation = true;
        ui_state.add_output(OutputLine::System(format!(
            "Apply {} file(s)? y/n/a/o (y=yes, n=no, a=apply all, o=other):",
            queued
        )));
    }
}

fn apply_pending_action(
    ui_state: &mut ui::UiState,
    app_state: &AppState,
    action: app::PendingAction,
    sandbox: &sandbox::Sandbox,
) {
    let file_ops = project::file_ops::FileOps::new(sandbox, app_state.mode);
    match action {
        app::PendingAction::WriteFile {
            ref path,
            ref content,
            diff_preview: _,
        } => {
            tracing::info!(
                "applying write: {} ({} bytes)",
                path.display(),
                content.len()
            );
            let fc = file_ops.prepare_write(path, content);
            match fc {
                Ok(fc) => match file_ops.apply_change(&fc) {
                    Ok(()) => {
                        ui_state
                            .add_output(OutputLine::System(format!("Wrote {}", path.display())));
                        run_syntax_check(ui_state, path, sandbox);
                        // LSP diagnostics needs workspace, not available here directly
                        // (apply_pending_action only has sandbox reference)
                    }
                    Err(e) => {
                        ui_state.add_output(OutputLine::Error(format!(
                            "Failed to write {}: {}",
                            path.display(),
                            e
                        )));
                    }
                },
                Err(e) => {
                    ui_state.add_output(OutputLine::Error(format!(
                        "Blocked {}: {}",
                        path.display(),
                        e
                    )));
                }
            }
        }
        app::PendingAction::DeleteFile { ref path } => {
            tracing::info!("applying delete: {}", path.display());
            let fc = file_ops.prepare_delete(path);
            match fc {
                Ok(fc) => match file_ops.apply_change(&fc) {
                    Ok(()) => {
                        ui_state
                            .add_output(OutputLine::System(format!("Deleted {}", path.display())));
                    }
                    Err(e) => {
                        ui_state.add_output(OutputLine::Error(format!(
                            "Failed to delete {}: {}",
                            path.display(),
                            e
                        )));
                    }
                },
                Err(e) => {
                    ui_state.add_output(OutputLine::Error(format!(
                        "Blocked {}: {}",
                        path.display(),
                        e
                    )));
                }
            }
        }
        app::PendingAction::ExecuteCommand { cmd, args } => {
            ui_state.add_output(OutputLine::System(format!(
                "Skipping command: {} {:?}",
                cmd, args
            )));
        }
    }
}

/// Run syntax check on a file and display results in chat.
fn run_syntax_check(
    ui_state: &mut ui::UiState,
    full_path: &std::path::Path,
    sandbox: &sandbox::Sandbox,
) {
    let path_display = full_path.display().to_string();
    let check_result = std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Runtime::new().ok()?;
            rt.block_on(agent::syntax::SyntaxChecker::check(full_path, sandbox))
                .ok()
        })
        .join()
        .ok()?
    });
    if let Some(result) = check_result {
        match result {
            agent::syntax::SyntaxResult::Pass => {
                tracing::debug!("syntax check PASS: {}", path_display);
                ui_state.add_output(OutputLine::System(format!("  Syntax OK: {}", path_display)));
            }
            agent::syntax::SyntaxResult::Fail { errors } => {
                tracing::warn!(
                    "syntax check FAIL: {} — {}",
                    path_display,
                    errors.lines().take(3).collect::<Vec<_>>().join("; ")
                );
                ui_state.add_output(OutputLine::Error(format!(
                    "  Syntax error in {}:\n  {}",
                    path_display,
                    errors.lines().take(5).collect::<Vec<_>>().join("\n  ")
                )));
            }
            agent::syntax::SyntaxResult::Skipped(reason) => {
                ui_state.add_output(OutputLine::System(format!(
                    "  Syntax check skipped: {}",
                    reason
                )));
            }
        }
    }
}

fn run_lsp_diagnostics(
    ui_state: &mut ui::UiState,
    full_path: &std::path::Path,
    workspace: &std::path::Path,
) {
    let client = match lsp::LspClient::for_file(full_path) {
        Some(c) => c,
        None => return,
    };
    if !client.is_available() {
        return;
    }
    let path_display = full_path.display().to_string();
    let ws = workspace.to_path_buf();
    let fp = full_path.to_path_buf();

    let result = std::thread::scope(|s| s.spawn(|| client.diagnostics(&fp, &ws)).join().ok());

    match result {
        Some(Ok(diagnostics)) => {
            if diagnostics.is_empty() {
                tracing::debug!("LSP diagnostics clean: {}", path_display);
            } else {
                let errs: Vec<String> = diagnostics
                    .iter()
                    .take(5)
                    .map(|d| format!("  L{} [{}]: {}", d.line, d.severity, d.message))
                    .collect();
                ui_state.add_output(OutputLine::System(format!(
                    "LSP diagnostics for {}:\n{}",
                    path_display,
                    errs.join("\n")
                )));
            }
        }
        Some(Err(e)) => {
            tracing::debug!("LSP diagnostics failed for {}: {}", path_display, e);
        }
        None => {
            tracing::debug!("LSP diagnostics thread panicked for {}", path_display);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase A: `apply_plan_prompt` must substitute both `{MAX_LINES}` and `{TOOLS}`
    /// into the planner system prompt. The planner uses text-only tool awareness —
    /// it sees prose descriptions but never gets a native `tools=` field.
    #[test]
    fn apply_plan_prompt_substitutes_both_placeholders() {
        let template = "Lines: {MAX_LINES}\nTools:\n{TOOLS}\nEnd.";
        let tools_desc = "- web_reader: fetch a URL\n- read_file: read a file";
        let rendered = apply_plan_prompt(template, 200, tools_desc);
        assert!(!rendered.contains("{MAX_LINES}"), "MAX_LINES not substituted");
        assert!(!rendered.contains("{TOOLS}"), "TOOLS not substituted");
        assert!(rendered.contains("Lines: 200"));
        assert!(rendered.contains("- web_reader: fetch a URL"));
        assert!(rendered.contains("- read_file: read a file"));
        assert!(rendered.ends_with("End."));
    }

    /// Phase A: tool descriptions come from a real ToolRegistry, so the planner
    /// sees every registered tool (file ops, web_reader, web_search, exec_shell).
    #[test]
    fn plan_prompt_tools_block_lists_all_registered_tools() {
        let reg = tools::ToolRegistry::new(
            std::path::PathBuf::from("."),
            &config::Config::default(),
        );
        let desc = reg.descriptions_text();
        // Every tool registered in ToolRegistry::new should be visible.
        for name in ["read_file", "write_file", "edit_file", "list_dir", "exec_shell"] {
            assert!(
                desc.contains(&format!("- {}: ", name)),
                "tool {} missing from descriptions_text: {}",
                name,
                desc
            );
        }
    }

    /// Phase D: routing helpers are gone. This test exists to fail loudly if anyone
    /// reintroduces mode-dependent branching into `spawn_request_for_mode` without
    /// updating the single-path architecture contract.
    #[test]
    fn no_agent_loop_routing_remnants() {
        // The two classifier helpers that used to drive agent-loop routing must not
        // be defined anywhere in the source. The check uses `"fn looks_like"` (without
        // the rest of the name) so this test's own source doesn't match itself.
        let main_rs = std::fs::read_to_string("src/main.rs").expect("read main.rs");
        let offending: Vec<&str> = main_rs
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("fn looks_like_")
                    || trimmed.starts_with("fn spawn_agent_loop")
                    || trimmed.starts_with("pub fn spawn_agent_loop")
            })
            .collect();
        assert!(
            offending.is_empty(),
            "single-path architecture violation — these functions must stay removed: {:#?}",
            offending
        );
    }
}

