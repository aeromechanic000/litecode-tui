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
├── wizard.rs            First-run setup wizard (Ollama URL, 3-tier model selection: Plan/Exec/Eval)
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
│   │                    in their content). has_final_answer() is a prose-detection
│   │                    helper, vestigial under Plan→Execute→Eval (the Eval step —
│   │                    not a fallback — produces the final answer).
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
│   ├── mod.rs           SearchEngine: multi-backend web search (Bing / Baidu /
│   │                    SearXNG / DuckDuckGo), region-aware fallback chain
│   │                    (auto_switch_network_region), result truncation
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
│   └── builtin.rs       Seeds built-in skills into ~/.litepilot/skills/ at
│                        startup (only-if-missing, never overwrites)
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
  no tool calls (the step's content becomes exec output for the Eval step) or
  `MAX_TOOL_ROUNDS_PER_STEP = 5` rounds elapse. Signature-repeat detection (same
  `name:arguments` as the previous round) also terminates the loop.

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
2. If `response.tool_calls.is_empty()` → step done; the step's content becomes part
   of the exec output the Eval step later reflects on. The exec step owes no prose
   answer (see *Final-Answer Guarantee*).
3. Else take the **first** tool call only (sequential agent loop — the model must see
   a tool's result before emitting a dependent call).
4. Echo the assistant turn + `tool_calls` to the message history.
5. Dispatch the call via `ToolRegistry::execute` → emit `ToolStart` then
   `ToolResultReady`.
6. Append the `tool`-role result message and re-call.

**No per-step final-answer fallback.** A step may legitimately end with only tool
calls or `### FILE:` blocks — that raw output is carried forward as exec content for
the Eval step, which judges satisfaction and writes the turn's final answer. The exec
model is told to call the next tool or finish the step; it is never asked to produce a
prose answer here.

### Pipeline flow — Plan → Execute → Eval

1. **Plan.** The **plan model** (`effective_plan_model()`) generates a numbered plan
   via `OllamaClient::chat` (with `{TOOLS}` in the system prompt, no `tools=`
   field).
2. The plan is displayed for approval (Edit mode) or auto-executed (Plan / Auto mode).
3. **Execute.** Each step runs via `stream_step_native_tools`; step outputs are
   concatenated into one exec output. `PipelineResult::StreamMeta` is emitted at the
   end with a rough prompt-token estimate.
4. **Eval.** The eval model reflects over the plan + exec output, judges
   whether the user's request was satisfied, and writes the turn's final answer (a
   concise summary). If unsatisfied, it proposes a user-approved redo / further round.
   See *Eval-Model Reflection & Redo Loop*.

The exec output is **not** the user-facing answer — the Eval step is.

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
  → parse file changes → mode-dependent apply flow   (exec output is the file source)
  → EvalReady → eval summary becomes the final answer; if unsatisfied,
    ask y/n/o to redo / further-round
  → drain pending_queue if non-empty
```

---

## Three-Tier Model Pipeline

> All three tiers are wired: `plan_model` + `effective_plan_model()` in `config.rs`,
> the planner call in `spawn_plan_then_execute`, a Plan slot in the wizard
> (Exec → Plan → Eval; Tab skips a slot to reuse Exec), and plan-model warmup. Each
> tier falls back to `exec_model` when its own field is empty.

| Tier | Size | Role | Config Field |
|------|------|------|-------------|
| Plan | 8B+ | Planning — turn the request into a numbered, tool-aware plan | `plan_model` |
| Exec | 6-14B | Execution — per-step tool dispatch, file generation | `exec_model` |
| Eval | 14B+ | Reflection — judge the result, write the concise final-answer summary, and (when unsatisfied) propose a redo / further round | `eval_model` |

Prompts adapt to model size via `agent::prompts::system_prompt_for_size()`: short/directive for small, standard+examples for medium, full/nuanced for large.

Each tier falls back to `exec_model` when its own field is empty
(`config::effective_plan_model()` / `effective_eval_model()`), so a single-model
setup still works — set only `exec_model`. A stronger `plan_model` (and `eval_model`)
improves plan quality and reflection; the plan and exec models may be the same, but
separating them lets a larger model do the reasoning while a smaller, faster model
does the step-by-step work.

**How it runs.** `spawn_plan_then_execute` (`src/main.rs`) calls the **plan model**
(`effective_plan_model()`) to generate the plan via `OllamaClient::chat` (text-only
tool awareness — `{TOOLS}` in the system prompt, no `tools=` field). After each
executed turn (when `mode != Plan` and an `eval_model` is configured),
`spawn_eval_reflection` calls the **eval model** with a single `submit_evaluation`
native tool. The eval model may equal the exec model — the reflection still yields a
clean answer-only summary; only an empty `eval_model` short-circuits (exec content
then stands as the answer). The eval verdict is `satisfied`, a concise answer-only
`summary` (the canonical assistant turn), and an optional redo `proposal`. Satisfied
paths commit the summary and finalize the turn; unsatisfied paths offer a y/n/o redo
(see *Eval-Model Reflection & Redo Loop*). Each phase is labeled in the UI: `◆ Plan`,
`◆ Exec`, `◆ Eval` (`OutputLine::Plan` / `OutputLine::Phase`).

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
plan_model = "qwen3:14b"
exec_model = "qwen3:8b"
eval_model = "qwen3:14b"
default_mode = "edit"
max_retries = 3
enable_free_web_search = true
auto_switch_network_region = true   # on backend failure, try region-reachable fallbacks
web_search_backend = "bing"          # bing | baidu | duckduckgo | searxng (bing is reachable in mainland China)
# searxng_url = "http://localhost:8080"   # optional self-hosted SearXNG instance (bypasses regional blocks)
search_cache_valid_days = 30
max_search_context_tokens = 2048

[theme]
primary = "cyan"
accent = "magenta"
warning = "yellow"
```

### Web search backends

`web_search` queries a configurable backend and, when
`auto_switch_network_region = true` (the default), falls through to
region-reachable alternatives on failure so search still works behind a regional
network block (e.g. mainland China, where DuckDuckGo is blocked). The configured
backend (`web_search_backend`) is tried first — strict preference; the fallback
order is SearXNG (only when `searxng_url` is set — a self-hosted instance
bypasses blocks entirely) → Bing (reachable in CN, broad coverage) → Baidu
(CN-local) → DuckDuckGo. With `auto_switch_network_region = false`, only the
configured backend is used.

Each backend request has a 15s timeout, so a blocked backend fails fast instead
of hanging the turn. Only non-empty results are cached, so a blocked/empty
response never poisons the cache. When every backend is unreachable,
`web_search` surfaces a distinct "all backends unreachable" error (hinting at a
regional block) rather than a bare failure — and the global instructions tell
the agent to treat repeated cross-host timeouts as a regional block, not a
per-site outage.

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

> **Default-skills convention:** unless a skill is explicitly marked as
> device-local / user-private (in this file or the skill's own frontmatter), every
> skill that exists in the LitePilot repo is a **default skill of LitePilot** and
> must ship with the installation. A new default skill is added by placing its
> `.md` in `src/skills_builtin/` and registering it in `BUILTIN_SKILLS`
> (`src/skills/builtin.rs`) — merely dropping it into `~/.litepilot/skills/`
> makes it device-local only, and it will be missing from every other install.

Skills live as Markdown + YAML frontmatter files in `~/.litepilot/skills/`
(`name`, `description`, `trigger: keyword1, keyword2`). `SkillRegistry` loads
them at startup. Also at startup, `Config::ensure_dirs_for` → `populate_skills`
**seeds the built-in skills into the global `~/.litepilot/skills/`** — always the
global dir, even when a project-local `.litepilot/` is the effective config dir.
The built-ins are: `search`, `review`, `explain`, `simplify`, `test`,
`translate`, `count-files`. Each is written **only if missing** — existing files
are never overwritten, so your edits are safe. Seeding covers *only* these
built-ins: a skill you add or delete yourself is never re-created or restored
(deleting a built-in's file *is* detected — see the startup check below).

**Startup missing-skill check.** `populate_skills` returns the names of the
built-ins it restored; `ensure_dirs_for` propagates that list to `main.rs`, which
logs it (`tracing::info!`, after logger init) and prints
`Restored missing built-in skill(s): …` to **stderr** (both TUI and headless
modes; stderr so piped `-p` stdout stays clean), so a default skill deleted by
accident (or absent from an upgrade) is silently healed and visibly reported,
never silently missing.

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

Every turn ends with a natural-language answer to the user, even after tool use or
file output. Under the **Plan → Execute → Eval** workflow this is the **Eval** step's
job, not the executor's — there is no exec-model fallback that asks the executor for
prose.

- **The Eval step always emits a prose answer.** After execution, the eval model
  reflects on the plan + exec output and writes a concise summary that *is* the
  user-facing final answer (see *Eval-Model Reflection & Redo Loop*). The executor's
  job is to call tools and write files; its raw output is the eval step's input, not
  the answer.
- **If no `eval_model` is configured** (empty), the eval step is a no-op and the
  exec output stands as the answer directly — so the guarantee still holds, just
  without a separate reflection pass. (An `eval_model` equal to `exec_model` still
  runs the eval step — the reflection yields a clean summary either way.)
- **If the eval step judges the result unsatisfied**, the turn still ends with an
  answer: either the user approves a redo (which itself ends with an eval answer) or
  declines (`n`) and keeps the current summary. The user is never left with only tool
  calls / file blocks / empty content.

Routing the answer through the larger eval model makes the guarantee stronger than an
exec-model fallback would: the final prose is written by the model best suited to
judge the work.

## Eval-Model Reflection & Redo Loop

> Implemented in `src/main.rs` (`spawn_eval_reflection`, `parse_eval_verdict`,
> `run_eval_redo`, `build_previous_attempt_block`, `commit_assistant_answer`,
> `turn_is_complete`) and `src/agent/retry.rs` (`EvalVerdict` / `RedoProposal` /
> `RedoKind`, carried on `PipelineResult::EvalReady`).

The eval model is the reflection stage that closes the plan→execute loop. After
every executed turn it judges whether the plan + execution actually satisfied the
user's request, writes the concise final-answer summary, and — when it judges the
result unsatisfactory — proposes a redo or a further round that the user approves.
This is what makes the two-tier pipeline meaningful: the eval model owns the final
answer.

### When it runs

- **Always**, once per turn, immediately after `spawn_execution_with_plan` emits
  its final `StreamDone` (the concatenated exec content). It does **not** check
  `has_final_answer()` first — there is no "did the exec answer?" gate; the eval
  step unconditionally reflects on the whole execution.
- **Short-circuit to a no-op** only when no `eval_model` is configured (empty). An
  `eval_model` equal to `exec_model` still runs — the reflection produces a clean
  summary, and the model is already warm so there is no reload cost. The eval call
  checks `config.eval_model.is_empty()` before spawning.
- **Cost note:** this adds one extra `/api/chat` call on every turn, on top of
  planning + execution. For local inference that is a per-turn latency cost; the
  short-circuit above keeps the no-`eval_model` configuration free of it.

### Inputs

The eval call receives: (1) the **original user request** (`ui_state.last_user_input`
/ the `current_goal`), (2) the **plan** (the `plan` passed to
`spawn_execution_with_plan`), and (3) the **execution output** — the same `content`
emitted in the final `StreamDone` (file `### FILE:` blocks, tool results, exec
prose). A truncation guard like the current 8000-char cap applies.

### Output (structured)

Reuse the existing `PipelineResult::EvalReady` channel, but expand its payload from
a display string to a small structured verdict so the loop can branch:

- `satisfied: bool` — does the execution fulfill the request? **Bias toward
  `true`**: a 14B model judging an 8B model's work is itself noisy, so the eval
  prompt should propose a redo only on a clear failure, not on style nitpicks —
  otherwise every turn nags the user.
- `summary: String` — the concise final answer the user sees. Focused on answering
  the request with the necessary details; it must **not** echo thinking logic,
  chain-of-thought, or rules copied from the system prompt / instructions. This
  `summary` is the canonical assistant turn stored in `conversation_history` and
  shown to the user.
- `proposal: Option<RedoProposal>` — present only when `satisfied == false`:
  `kind` ∈ {`FurtherRound` (one more targeted pass with a suggested request),
  `RedoPlan` (re-plan and re-execute from scratch)}, a one/two-sentence rationale,
  and the suggested follow-up request text.

### File changes vs. prose answer — keep them separate

The **execution output stays the source of file changes.** The existing post-
`StreamDone` apply flow (`parse_file_changes` → mode-dependent apply) parses
`### FILE:` / `### ACTION:` blocks from the *exec* `content`, and must keep doing
so. The eval `summary` is prose only — it never becomes the file source. So a turn
that produced files applies them from exec output, and *additionally* shows the
eval summary as the user-facing answer.

### Satisfied path

1. The eval `summary` is sanitized (`tools_parser::sanitize_output`) and promoted
   to the assistant turn — rendered via `OutputLine::Assistant` and pushed into
   `conversation_history`.
2. File changes from the exec `content` flow through the normal apply path (Edit
   confirmation / Auto apply / Plan hint).
3. Turn ends; `pending_queue` drains.

### Unsatisfied path — user-approved redo

When `satisfied == false`, surface the proposal and ask the user for approval,
reusing the existing interactive primitives (the `pending_plan` / `awaiting_other_input`
handlers in `src/main.rs`):

- A new awaiting flag (e.g. `pending_redo_decision: Option<RedoProposal>`) in the
  same family as `pending_plan`, set after `EvalReady` instead of `PlanReady`.
- The prompt follows the established wording pattern and uses the **same approval
  vocabulary as the other prompts** (y/n/o):
  `Eval: result may be incomplete. Redo? y/n/o`
  `(y=accept proposal, n=keep current result, o=type your own instruction)`.
- Key handling mirrors the plan-approval interceptor (`y/n/o`); `o` flips
  `awaiting_other_input = true` so the user's typed instruction reroutes to
  `spawn_request_for_mode` — the same path the existing `o` feedback uses.
- Choices: `y` → run the proposed `FurtherRound` / `RedoPlan`; `n` → keep the
  current result (apply any exec files, end turn — the user is never forced into a
  redo); `o` → free-text steering treated as a fresh request with prior-execution
  context attached (next bullet).
- **Redo cap.** At most a small number (e.g. 2) of eval-proposed redos per turn;
  beyond that the eval must stop proposing and either accept or hand control to
  the user, so an unhappy eval cannot loop forever.

### Context injection on redo (how the next attempt improves)

When the user approves a redo (or types `o` steering), the next turn carries the
prior attempt so the exec model does not repeat its mistakes. Two existing seams,
used together:

- A `[Previous attempt]` block appended to the next turn's context, mirroring the
  in-plan `[Previous steps completed]` block already built in
  `spawn_execution_with_plan` — it should carry the prior plan, a condensed view of
  the prior exec output (files touched + key results), and the eval's rationale +
  suggested request.
- The volatile `PromptBuilder` fields (`set_volatile` / `set_current_goal`, set per
  turn inside `spawn_request_for_mode`) carry the user's approved steering as the
  new `current_goal` so it lands at the prompt edge for small-model attention.

The prior exec output is summarized/condensed, not echoed verbatim, to respect the
context budget (see *Context Window Management*).

### Open decisions (to pin down before implementing)

- **Auto mode.** Auto has no confirmation today. Should an eval-proposed redo
  auto-run (respecting the redo cap) or always ask? Recommendation: always ask — a
  meta-decision about re-planning is worth a confirmation even in Auto.
- **Headless `-p/--prompt`.** Interactive approval is impossible. Recommendation:
  skip the redo loop in headless — emit the eval `summary` as the final answer, and
  if `satisfied == false`, print the proposal as a non-blocking note rather than
  blocking.
- **Exec fallback — removed (decided).** No per-step exec-model `tools=[]` answer
  call; the Eval step is the sole final-answer source (see *Final-Answer
  Guarantee*). `has_final_answer()` becomes vestigial and can be dropped when the
  eval step lands.
- **Eval prompt.** Must strongly bias toward PASS and forbid copying rules /
  chain-of-thought into the summary (the summary must be answer-only).

## Naming Conventions

- Package/binary: `litepilot-tui` / `litepilot`
- User config dir: `~/.litepilot/`
- Avoid referencing competitor product names in code or UI text.

## Testing Strategy

- **Unit tests**: Each module has `#[cfg(test)] mod tests` inline. ~325 tests.
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
