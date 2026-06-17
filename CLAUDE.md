# LitePilot-TUI

Terminal AI coding assistant powered by Ollama-host local models. Written in Rust.

## Quick Start

```bash
# Build
cargo build
# Run (requires Ollama running locally)
cargo run
# Test
cargo test
cargo test -- --ignored  # integration tests that need Ollama
```

## Architecture

```
src/
├── main.rs              Entry point: CLI args (clap), terminal bootstrap,
│                        event loop with mpsc channel for non-blocking LLM calls,
│                        message queue for buffered input during processing
├── app.rs               AppState: mode, config, processing state, pending queue,
│                        ContextManager for KV cache context handle tracking
├── config.rs            Config struct (serde TOML), defaults, dir management,
│                        project-local (.litepilot) + global (~/.litepilot) loading
├── context.rs           Message history management: build_messages (budget-aware),
│                        maybe_compact (truncation), compact_with_summary (LLM-powered)
├── prompt.rs            PromptBuilder: layered system prompt construction
│                        (identity, mode, skills, project context, volatile tail)
├── wizard.rs            First-run setup wizard (Ollama URL, 2-tier model selection)
│
├── ui/
│   ├── mod.rs           TUI rendering: status bar (with ctx:% indicator), chat panel,
│   │                    input bar. OutputLine enum for typed chat history
│   └── theme.rs         Theme struct with configurable primary/accent/warning colors
│
├── ollama/
│   ├── mod.rs           OllamaClient + ContextManager (KV cache handle lifecycle,
│   │                    cache hit rate, context usage tracking, static prefix hash)
│   │                    tokenize() for accurate token counting via /api/tokenize
│   ├── chat.rs          /api/chat (blocking, for skills) + /api/generate (streaming,
│   │                    with KV cache context handle reuse). GenerateChunk carries
│   │                    prompt_eval_count, eval_count, context on final chunk.
│   └── model.rs         ModelInfo, ModelSize classification (Small/Medium/Large),
│                        context window estimation, parameter count heuristics
│
├── agent/
│   ├── mod.rs           Agent module root: submodules (auto_run, diagnostics, editor,
│   │                    planner, prompts, retry, summarizer, syntax, tools_parser)
│   ├── tools_parser.rs  Parse text/JSON tool calls from LLM output + sanitize_output()
│   │                    scrubs forged tool call markers from display
│   ├── planner.rs       Plan mode: builds prompt context for read-only analysis
│   ├── editor.rs        Edit mode: generates file changes, presents diff for approval
│   ├── auto_run.rs      Auto mode: full pipeline orchestration constants
│   ├── prompts.rs       System prompts per model tier, model-size-adaptive templates
│   ├── retry.rs         chat_with_retry() with exponential backoff,
│   │                    ErrorClass (Retryable/Permanent) error classification,
│   │                    PipelineResult (StreamChunk/StreamDone/StreamMeta/...)
│   ├── summarizer.rs    Background conversation summarization with priority pinning
│   ├── diagnostics.rs   Post-write syntax diagnostics for correction feedback
│   └── syntax.rs        Multi-language syntax checker (Python/JS/Shell/Rust/Go/C/C++)
│
├── tools/
│   ├── mod.rs           ToolRegistry: Ollama function-calling tool definitions
│   ├── file_ops.rs      read_file, write_file, edit_file, list_dir tools
│   ├── search.rs        search_files tool (grep-based)
│   └── shell.rs         run_command tool (sandboxed)
│
├── sandbox/
│   ├── mod.rs           Sandbox: path validation (traversal blocking), command allowlist
│   ├── executor.rs      Sandboxed command runner: allowed/blocked command dispatch
│   ├── landlock.rs      Linux Landlock sandbox (path restrictions)
│   └── seatbelt.rs      macOS Seatbelt sandbox (compiled profile)
│
├── search/
│   ├── mod.rs           SearchEngine: DuckDuckGo HTML scraping, result truncation
│   └── cache.rs         SearchCache: disk-based cache with TTL expiry
│
├── project/
│   ├── mod.rs           ProjectContext: file tree scan (respects .gitignore), git status
│   ├── file_ops.rs      File read/write/delete with sandbox + mode permission checks
│   └── uv.rs            UV toolchain: init, venv, add, run
│
├── codebase/
│   ├── mod.rs           CodeBase: template loading, tag-based search
│   ├── builtin.rs       Built-in template library (40+ templates, include_str! at compile)
│   ├── index.rs         Tag index: @LITE_DESC/@LITE_TAGS scanning, file discovery
│   └── retrieval.rs     Context budget: template selection within token limits
│
├── session/
│   ├── mod.rs           Session: id, messages, metadata, UUID-based
│   └── persistence.rs   JSON serialize/deserialize sessions to ~/.litepilot/sessions/
│
├── skills/
│   ├── mod.rs           SkillRegistry: load/lookup/trigger matching
│   ├── parser.rs        Markdown + YAML frontmatter skill parser
│   └── builtin.rs       Built-in skills population to ~/.litepilot/skills/
│
├── approval.rs          Risk classification (Safe/Write/Destructive), command classification
├── hooks.rs             JsonlSink: structured event logging (turn start/complete, tool events)
├── logger.rs            File logging init (tracing-appender)
├── lsp.rs               LSP client: pyright, typescript-language-server, rust-analyzer
├── recap.rs             Turn recap generation for substantial auto changes
├── router.rs            Request→model-tier routing (Exec/Eval) by input analysis
├── snapshot.rs          Git-based file snapshots (pre/post turn, undo/restore)
├── working_set.rs       WorkingSet: frecency-tracked file touch log for prompt context
│
└── util/
    ├── mod.rs
    ├── diff.rs          Unified diff generation (similar crate), change extraction
    └── text.rs          Token estimation, text/line truncation
```

---

## KV Cache Context Management

The streaming path uses `/api/generate` (not `/api/chat`) to gain manual control over the KV cache context handle. The `/api/chat` endpoint hides the `context` field internally, preventing cache reuse across turns.

### Context Handle Lifecycle

```
First request (new session):
  POST /api/generate { model, prompt, system, stream: true }
  → Response final chunk: context=[114, 514, ...], prompt_eval_count=1024

Subsequent requests:
  POST /api/generate { model, prompt, system, context=[114,514,...], stream: true }
  → Ollama prefix-matches against cached KV tensors
  → Response final chunk: context=[999, 888, ...], prompt_eval_count=64
  → Old handle discarded, new handle stored

/clear:
  ContextManager.clear() → handle = None, history cleared
  Next request omits context field → fresh session
```

### ContextManager (`ollama/mod.rs`)

Tracks: `context_handle`, `total_prompt_tokens`, `last_prompt_eval_count`, `last_model`, `static_prefix_hash`.

- `context_handle_for_model(model)` — returns handle only if it matches the model (incompatible across models)
- `update_from_response()` — stores new handle, replaces old, updates eval stats
- `cache_hit_rate()` — `(total - prompt_eval_count) / total * 100%`
- `context_usage_percent(window)` — current usage vs model's context window
- `set_static_prefix_hash(hash)` — tracks static prompt prefix hash, warns on change (KV cache miss)

### Display in UI

- After each response: `KV cache: 94.2% hit (1920 cached, 128 recomputed, 256 generated)`
- Warning at 80%: `Context 82% full (3328/4096 tokens). Consider /clear to start fresh.`
- Error at 100%: `Context OVERFLOW! Use /clear to reset.`
- Status bar shows `ctx:N%` with warning color when > 80%

---

## Execution Pipeline Architecture

There is **one** execution pipeline: plan-then-execute (`spawn_plan_then_execute` →
`spawn_execution_with_plan`). Every free-text request flows through it, regardless of
mode (Plan / Edit / Auto) or whether tools are needed.

### Tool awareness via prompt engineering on `/api/generate`

Tool calls are enabled without native `/api/chat` tool-calling. Both phases are
tool-aware through prompt text:

- **Planner**: `QUICK_PLAN_SYSTEM` interpolates a `{TOOLS}` block — a prose listing of
  tool names + descriptions from `ToolRegistry::descriptions_text()`. The planner does
  not call tools; it emits text steps that reference them (e.g. "Use web_reader to
  fetch https://…"). See `apply_plan_prompt()` in `src/main.rs`.

- **Executor**: `base_identity_prompt()` (`src/prompt.rs`) teaches the LLM to emit
  text-format `<tool_call name="…" call_id="…">{…}</tool_call>` blocks. The wrapper
  `stream_step_with_tools` (`src/main.rs`) wraps the unchanged `/api/generate`
  primitive (`stream_single_step_generate`), parsing each step's output via
  `parse_tool_calls_with_diagnostics` and dispatching through `ToolRegistry`.

### Per-step tool loop

For each plan step, `stream_step_with_tools` runs up to `MAX_TOOL_ROUNDS_PER_STEP = 5`
rounds. Each round:

1. Call `stream_single_step_generate` (which itself owns truncation continuation and
   KV-cache context-handle threading).
2. Parse the assistant output for `<tool_call>` blocks.
3. If no calls and not a failed-attempt → step done, return content.
4. If no calls but `is_failed_attempt()` and `correction_retries < 2` → append a
   correction prompt, retry the same round.
5. If calls found → validate name and params per call → emit `PipelineResult::ToolStart`
   → dispatch via `ToolRegistry::execute` → emit `PipelineResult::ToolResultReady`.
6. Build the next-round prompt by appending the verbatim assistant output, one
   `<tool_result tool="…" call_id="…">{…}</tool_result>` block per call, and a
   `user: Continue the step…` instruction.
7. Pass the previous round's `StepResult.context_handle` as input. Ollama prefix-matches
   the cached KV tensors and only evaluates the delta.

Termination caps: `MAX_TOOL_ROUNDS_PER_STEP = 5`, `MAX_TOOL_CORRECTION_RETRIES = 2`,
plus signature-repeat detection (same tool name + params as previous round).

### Pipeline flow

1. Exec model generates a numbered plan via `OllamaClient::chat` (with `{TOOLS}` in
   the system prompt — no `tools=` field).
2. Plan displayed for approval (Edit mode) or auto-executed (Plan / Auto mode).
3. Each step streamed via `/api/generate` with KV cache context handle.
4. Steps carry the context handle forward between iterations.
5. Within a step, tool rounds chain the context handle forward for cache reuse.

---

## Tool Call Sanitization

`tools_parser::sanitize_output()` scrubs forged tool call markers from LLM text output
before display. Incomplete `<tool_call` tags without closing markers are replaced with
`[invalid tool call]`. Applied at the display layer in `StreamChunk` and `StreamDone`
processing, so the parser still sees raw input for tool dispatch but the user never sees
forged markers.

---

## Layered System Prompt & KV Cache Stability

`PromptBuilder` (`src/prompt.rs`) separates the system prompt into:

- **Static layers** (byte-identical across turns): base identity → mode overlay → skills → project context
- **Volatile tail** (rebuilt each turn): working set summary, conversation summary, completed tasks, current goal, environment block (date/time)

The static prefix is hashed (`static_prefix_hash()`) and tracked by `ContextManager`. If the hash changes between turns, a warning is logged indicating a KV cache miss. `validate_for_kv_cache()` checks that no volatile data (e.g., date/time) has leaked into static layers.

---

## Event Processing Pipeline

### User Input → LLM Response

```
Enter key
  → classify input:
     ├─ /quit, /exit, /clear  → handle immediately
     ├─ /skills, /setup       → handle immediately
     ├─ /apply                 → parse last assistant msg → write files
     ├─ /run <cmd>             → sandboxed execution
     ├─ /skill_name args       → spawn_skill_request()
     └─ free text              → record in history → spawn_request_for_mode()
                                  → spawn_plan_then_execute()  (single pipeline)
  → OutputLine::User(msg) added immediately
  → app_state.is_processing = true
  → background thread → /api/generate with context handle
  → main loop receives: StreamChunk (tokens), StreamDone (content),
    StreamMeta (context handle, eval stats)
  → update ContextManager, display cache stats + context warnings
  → parse file changes → mode-dependent apply flow
  → drain pending_queue if non-empty
```

---

## Two-Tier Model Pipeline

| Tier | Size | Role | Config Field |
|------|------|------|-------------|
| Exec | 6-14B | Main work — planning, file generation, per-step tool dispatch | `exec_model` |
| Eval | 14B+ | Review — check results, quality assurance | `eval_model` |

Prompts adapt to model size via `agent::prompts::system_prompt_for_size()`: short/directive for small, standard+examples for medium, full/nuanced for large.

---

## Three Modes (Permission System)

| Mode | Write Files | Run Commands | Confirmation | Toggle |
|------|-------------|--------------|--------------|--------|
| Plan | No | No | N/A | Shift+Tab |
| Edit | Yes | Yes | Required (/apply) | Shift+Tab |
| Auto | Yes | Yes | None (sandboxed) | Shift+Tab |

File change confirmation in Edit mode requires y/n/a for every file change. Risk classification (Write/Destructive) requires double-key for destructive ops.

---

## Sandbox Security

- **Path validation**: Canonicalize paths, reject `..` traversal, block symlink escape outside workspace
- **Command filtering**: Allowlist (cargo, python, node, npm, git, make, gcc, go, uv) + Blocklist (sudo, rm -rf /, chmod 777, mkfs, dd)
- **Platform sandboxes**: Linux Landlock, macOS Seatbelt (compiled policy)
- **Mode enforcement**: File writes blocked at code level in Plan mode

---

## Response Validation & Retry

Two retry layers:

### Response Quality Retry
Retries on **response quality** since local models can produce malformed output:

1. `validate_response()` checks structure (file markers, code fences, action markers)
2. On failure: builds correction prompt showing previous mistakes
3. Retries up to `max_retries` times
4. Returns Success / Exhausted (last attempt) / Failed (Ollama error)

### Transport Retry with Exponential Backoff
When `chat_with_retry()` encounters an Ollama error:

1. `classify_error()` categorizes as Retryable (timeout, connection errors, 5xx) or Permanent (404, 400)
2. Retryable errors: sleep with exponential backoff (1s base, doubling, 16s cap, with jitter), then retry
3. Permanent errors: return immediately with descriptive message (e.g., "Model not found")
4. Backoff prevents overwhelming Ollama during model loading or GPU memory pressure

---

## Context Window Management

Two strategies prevent context overflow:

1. **Token budget truncation** (`context::build_messages`): walks history newest→oldest, only includes messages that fit within 90% of the model's context window
2. **LLM-powered summarization** (`agent::summarizer`): when history exceeds threshold, background task summarizes older messages while keeping pinned (file changes, errors) and recent messages verbatim

---

## Session Persistence

Sessions stored as JSON at `~/.litepilot/sessions/{uuid}.json` with atomic writes. Supports `--resume` (latest or by ID prefix) and `--sessions` (list). Auto-saved after each assistant response.

---

## Configuration

`~/.litepilot/config.toml` (or `.litepilot/config.toml` in project root):

```toml
ollama_endpoint = "http://127.0.0.1:11434"
connect_timeout = 15
context_window_limit = 262144
exec_model = "qwen3:8b"
eval_model = "qwen3:14b"
default_mode = "edit"
max_retries = 3
enable_free_web_search = true
search_cache_valid_days = 30
max_search_context_tokens = 2048
max_template_context_tokens = 2048

[theme]
primary = "cyan"
accent = "magenta"
warning = "yellow"
```

Config loading: project-local → global → defaults.

## Naming Conventions

- Package/binary: `litepilot-tui` / `litepilot`
- User config dir: `~/.litepilot/`
- Avoid referencing competitor product names in code or UI text.

## Testing Strategy

- **Unit tests**: Each module has `#[cfg(test)] mod tests` inline. ~300 tests.
- **Integration tests**: `tests/` directory. Tests needing live Ollama marked `#[ignore]`.
- **Sandbox tests**: Verify path traversal blocking, command allowlist enforcement.
- **Property tests**: `proptest` for config parsing, diff generation, token estimation.
- **Mock HTTP**: `wiremock` for Ollama response mocking.

## Dependencies (key)

- `ratatui` + `crossterm` — TUI rendering
- `tokio` — async runtime (for Ollama client + streaming)
- `reqwest` — HTTP client for Ollama API
- `serde` + `toml` — config serialization
- `async-stream` — SSE streaming via `stream!` macro
- `similar` — diff generation
- `syntect` — syntax highlighting
- `clap` — CLI argument parsing
- `anyhow` + `thiserror` — error handling
- `chrono` + `uuid` — session management
- `insta` — snapshot testing
- `wiremock` — HTTP mocking
- `proptest` — property-based testing
