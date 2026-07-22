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
│                        total_prompt_tokens (drives the ctx:% status indicator)
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
│   ├── mod.rs           OllamaClient. tokenize() for accurate token counting via
│   │                    /api/tokenize (currently unused).
│   ├── chat.rs          /api/chat (non-streaming, for planner and skills) +
│   │                    chat_native_streaming (the only executor path). The
│   │                    `think` field is intentionally omitted from both request
│   │                    types: sending it crashes some Ollama builds with thinking
│   │                    models (500 {"error":"EOF"}). Ollama applies the model
│   │                    default, which works reliably for native tool calling.
│   └── model.rs         ModelInfo, ModelSize classification (Small/Medium/Large),
│                        context window estimation, parameter count heuristics
│
├── agent/
│   ├── mod.rs           Agent module root: FileChange + parse_file_changes() (extracts
│   │                    ### FILE:/### ACTION: blocks). Submodules: diagnostics, prompts,
│   │                    retry, summarizer, syntax, tools_parser
│   ├── tools_parser.rs  sanitize_output() scrubs forged tool-call markers from
│   │                    display (native tool calls arrive via Ollama's structured
│   │                    `tool_calls` array, but models can still echo forged markers
│   │                    in their content); has_final_answer() drives the
│   │                    final-answer fallback.
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

## Context Usage Tracking

The executor uses `/api/chat` (native tool calling). That endpoint does not expose
Ollama's KV-cache `context` handle, so LitePilot does not track KV-cache hit-rate or
perform manual prefix reuse. What it *does* track:

- `AppState.total_prompt_tokens` — rough heuristic estimate of the last request's
  prompt size, fed from `PipelineResult::StreamMeta`.
- Status bar shows `ctx:N%` where `N = total_prompt_tokens / context_window_limit * 100`,
  warning-colored above 80%, red at 100%.
- Context-overflow warnings emitted from the `StreamMeta` handler in `src/main.rs`:
  - 80%: `Context NN% full (T/W tokens). Consider /clear to start fresh.`
  - 100%: `Context OVERFLOW! NN% of window used (T/W tokens). Use /clear to reset.`
- `/clear` resets `total_prompt_tokens = 0` and the conversation history.

The estimate is approximate (heuristic token counter, not the model's tokenizer). It
exists for at-a-glance context pressure, not for billing/precision.

---

## Execution Pipeline Architecture

There is **one** execution pipeline: plan-then-execute (`spawn_plan_then_execute` →
`spawn_execution_with_plan`). Every free-text request flows through it, regardless of
mode (Plan / Edit / Auto) or whether tools are needed.

### Tool awareness (native tool calling)

The planner and executor both flow through `/api/chat`. The executor uses Ollama's
**native tool-calling** (`tools=` field):

- **Planner**: `QUICK_PLAN_SYSTEM` interpolates a `{TOOLS}` block — a prose listing of
  tool names + descriptions from `ToolRegistry::descriptions_text()`. The planner does
  not call tools; it emits text steps that reference them (e.g. "Use web_reader to
  fetch https://…"). The planner's request does not include `tools=`.

- **Executor**: `spawn_execution_with_plan` calls `stream_step_native_tools`
  (`src/main.rs`) → `OllamaClient::chat_native_streaming` (`src/ollama/chat.rs`) with
  `ToolRegistry::ollama_tool_definitions()`. The model returns structured `tool_calls`
  (parsed into `NativeToolCall`); the loop dispatches the first one, appends the
  assistant turn (with its `tool_calls`) and a `tool`-role result message, then
  re-calls `/api/chat` so the model sees the result. Terminates when the model returns
  no tool calls (content is the step's final answer) or `MAX_TOOL_ROUNDS_PER_STEP = 5`
  rounds elapse. Signature-repeat detection (same `name:arguments` as the previous
  round) also terminates the loop.

- **Why native, not text-format:** thinking models (e.g. `qwen3.5`) put tool intent
  into their reasoning and stop with empty content on `/api/generate`; they emit native
  tool calls reliably. Non-thinking models also work — native tool calling is supported
  on Ollama 0.4+ across model families.

- **Tradeoff:** `/api/chat` does not expose the KV-cache `context` handle, so manual
  prefix reuse is bypassed. Context usage is shown as a token estimate (see above).

### Per-step tool loop (`stream_step_native_tools`)

For each plan step, up to `MAX_TOOL_ROUNDS_PER_STEP = 5` rounds:

1. Call `chat_native_streaming` with `tools = ollama_tool_definitions()`. Streamed
   content tokens emit `PipelineResult::StreamChunk` for live display.
2. If `response.tool_calls.is_empty()` → step done; the content is the answer
   (subject to the final-answer fallback below).
3. Else take the **first** tool call only (sequential agent loop — the model must see
   a tool's result before emitting a dependent call).
4. Echo the assistant turn + `tool_calls` to the message history.
5. Dispatch the call via `ToolRegistry::execute` → emit `ToolStart` then
   `ToolResultReady`.
6. Append the `tool`-role result message and re-call.

**Final-answer fallback**: if tools ran but the candidate content lacks a prose answer
(`tools_parser::has_final_answer()` returns false — fewer than 3 non-tool/file/code-fence
words), one more `/api/chat` call is made with empty `tools=[]` and an explicit
"answer now in prose" instruction. The model must respond in plain text. A no-op when
no tools ran.

### Pipeline flow

1. Exec model generates a numbered plan via `OllamaClient::chat` (with `{TOOLS}` in
   the system prompt, no `tools=` field).
2. Plan displayed for approval (Edit mode) or auto-executed (Plan / Auto mode).
3. Each step executed via `stream_step_native_tools`.
4. `PipelineResult::StreamMeta` emitted at the end with a rough prompt-token estimate.

---

## Tool Call Sanitization

`tools_parser::sanitize_output()` scrubs forged tool-call markers from LLM text output
before display. Even with native tool calling, a model can echo forged
`<tool_call>` markers in its content — incomplete `<tool_call` tags without closing
markers are replaced with `[invalid tool call]`. Applied at the display layer
(`StreamChunk` and `StreamDone` handling), so the structured dispatcher is unaffected.

---

## Thinking Field Handling

Ollama's `think` field is **omitted** from every `/api/chat` request body.
`ChatRequest` uses `#[serde(skip_serializing_if = "std::ops::Not::not")]` with the
field always set to `false`, and `chat_native_streaming`'s raw JSON body omits the
field entirely. Sending `think:true` or `think:false` explicitly crashes some Ollama
builds with thinking models (`500 {"error":"EOF"}`); omitting it lets Ollama apply the
model default, which works reliably for native tool calling. There is no
`enable_thinking` config flag.

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
  → background thread → /api/chat with native tools
  → main loop receives: StreamChunk (tokens), StreamDone (content),
    StreamMeta (prompt token count, model)
  → update app_state.total_prompt_tokens, refresh ctx:% status bar
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

[theme]
primary = "cyan"
accent = "magenta"
warning = "yellow"
```

Config loading: project-local → global → defaults.

## Knowledge Sources (instructions + skills)

LitePilot has two user-editable knowledge channels — both load from disk at
runtime, so no recompile is needed to add or change domain rules.

### 1. Global / project instructions — always on
`prompt::ProjectInstructions::discover()` reads, in priority order: `AGENTS.md`,
`CLAUDE.md` (workspace), `instructions.md` (effective config dir — project-local
`.litepilot/` if present, else global `~/.litepilot/`), and `README.md` (first
100 lines, fallback). These are injected as a static `project_context` prompt
layer (`set_project_context`), byte-identical across turns → KV-cache friendly.
This is the home for short, universal conventions (e.g. filesystem/shell accuracy
rules) that should apply on every turn.

### 2. Skills — invoked or auto-triggered
Skills live as Markdown + YAML frontmatter files in `~/.litepilot/skills/`
(`name`, `description`, `trigger: keyword1, keyword2`). `SkillRegistry` loads
them at startup; `populate_skills` writes the built-in skills only if absent
(never overwrites user edits).

- **By invocation**: the user types `/skill_name args` → `spawn_skill_request`
  appends the skill body to the system prompt.
- **By auto-trigger**: `SkillRegistry::match_triggers(input)` returns every skill
  whose non-empty trigger keyword appears (case-insensitive substring) in the
  user's input. The matched bodies are concatenated (token-capped by
  `matched_skills_block`) and injected into BOTH the planner system message and
  the executor's `coding_system` for that turn. Skills with empty triggers are
  skipped (an empty keyword would match every input). This surfaces relevant
  domain rules without the user invoking the skill, and only bloats the prompt
  when a rule is actually relevant.

The two channels compose: instructions carry universal rules; skills carry
task-specific methodologies that auto-attach when the task matches.

## Final-Answer Guarantee

Every turn ends with a natural-language answer to the user, even after tool use.
This is enforced by prompt rules plus a fallback:

- **Base identity** (`prompt.rs`): an "Always answer the user" rule tells the
  model to end every turn with prose, never only tool calls / file blocks / empty.
- **Planner** (`agent::prompts::QUICK_PLAN_SYSTEM`): every plan's LAST step must
  answer the request in plain text (summarize for file tasks; answer directly for
  questions).
- **Per-step workflow** (`spawn_execution_with_plan`): file output is conditional,
  the final step is flagged as the answer step, and every step must end with a
  prose answer.
- **Tool-loop continuation** (`stream_step_native_tools`): after a tool result,
  the model is told to either call the next tool or give the final answer in prose.
- **Fallback**: a tool-using step that ends with no prose answer — detected by
  `tools_parser::has_final_answer()` (≥3 words outside tool-call / tool-result /
  `### FILE:` / fenced-code blocks) — triggers one more `/api/chat` call with
  empty `tools=[]` that explicitly asks for the answer. Streams live like a
  normal round; a no-op when no tools ran.

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
