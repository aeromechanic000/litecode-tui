# LitePilot-TUI Implementation Plan

## Overview

This plan tracks implementation progress. Milestones are vertical slices:
implement, write tests, verify green. Targets **v1.0** first, then v1.1–v1.3.

Status: DONE | PARTIAL | TODO

---

## Phase 0 — Project Bootstrap

### M0.1 Cargo project + CI skeleton DONE

Cargo.toml with all deps, main.rs entry point, CI workflow.
`cargo build` / `cargo test` / 160 tests pass.

---

## Phase 1 — Foundation Layer

### M1.1 Config module DONE

`src/config.rs` — Config struct (serde TOML), load/save/validate,
project-local + global loading, ThemeColors. Proptest round-trip tests.

### M1.2 First-run wizard DONE

`src/wizard.rs` — 4-step interactive wizard (URL → connect → model select → confirm).
Model selection for Fast/Core/Audit slots with list navigation.

### M1.3 Ollama client core DONE

`src/ollama/mod.rs` — OllamaClient with ping(), list_models().
`src/ollama/model.rs` — ModelInfo, ModelSize (Small/Medium/Large),
parameter estimation, context window heuristics.

### M1.4 Ollama streaming chat DONE

`src/ollama/chat.rs` — chat() (blocking) and chat_stream() (async SSE) both wired.
Streaming used in `spawn_llm_request()` for token-by-token rendering.
Cancellation token and `StreamChunk`/`StreamDone` event handling fully implemented.

---

## Phase 2 — Application State & Modes

### M2.1 App state machine DONE

`src/app.rs` — AppMode (Plan/Edit/Auto) with cycle(), permission checks
(can_write_file, can_execute_command, needs_confirmation).
AppState with is_processing + pending_queue for non-blocking UI.

### M2.2 Session persistence DONE

`src/session/mod.rs` — Session struct with UUID, messages, timestamps.
`src/session/persistence.rs` — JSON save/load/list with atomic writes.

---

## Phase 3 — Agent Pipeline

### M3.1 Agent orchestrator DONE

`src/agent/mod.rs` — AgentPipeline with plan()/implement()/audit() fully implemented.
Auto mode uses the tool-use agent loop (`agent_loop.rs`) instead of the classical
plan→implement→audit pipeline. Both approaches coexist. The agent loop is the
primary path; AgentPipeline available for explicit plan-based workflows.

### M3.2 Prompt engineering DONE

`src/agent/prompts.rs` — PLANNING_SYSTEM, CODING_SYSTEM, AUDIT_SYSTEM.
`system_prompt_for_size()` for model-size-adaptive prompts.

### M3.3 Syntax checker DONE

`src/agent/syntax.rs` — Multi-language (Python, JS/TS, Bash, Rust, Go, C/C++)
fully implemented with Language enum and SyntaxChecker.
Run after every file write in Auto mode via `run_syntax_check()`.
Diagnostic-based self-correction added in M10.5.5.

---

## Phase 4 — Sandbox & File Operations

### M4.1 Sandbox core DONE

`src/sandbox/mod.rs` — Path validation (canonicalize, `..` rejection, symlink escape).
`src/sandbox/executor.rs` — Command allowlist/blocklist, sandboxed execution.

### M4.2 Project file operations DONE

`src/project/file_ops.rs` — FileOps with mode-aware read/write/delete and diff
preview generation. Fully integrated in main.rs for all file operations.
Sandbox validation applied to every write.

### M4.3 UV toolchain integration DONE

`src/project/uv.rs` — UvManager with init/venv/add/run. Exposed via `/uv` slash
commands (`/uv init`, `/uv venv`, `/uv add`, `/uv run`).

---

## Phase 5 — TUI Rendering

### M5.1 Theme & layout primitives DONE

`src/ui/theme.rs` — Theme with configurable primary/accent/warning (hex + ANSI).
`src/ui/mod.rs` — Layout: status bar, main area, sidebar, input bar.

### M5.2 Status bar DONE

`src/ui/mod.rs::draw_status_bar()` — LitePilot logo, endpoint, F/C/A models,
mode badge, search toggle, working dir, thinking indicator + queued count.

### M5.3 Chat panel DONE

`src/ui/mod.rs` — Full rendering pipeline:
- Syntect syntax highlighting for code blocks (highlight_code with RGB spans)
- PageUp/PageDown scrolling with auto-scroll toggle
- Markdown formatting: headers (bold + primary color), inline code, bullet/numbered lists
- OutputLine variants: User/Assistant/System/Error/Code/Diff/Thinking/Pending

### M5.4 Sidebar DONE

`src/ui/mod.rs` — SidebarTab enum (ProjectFiles/CodeBase), toggle with Esc.
- File tree rendering with depth indentation and expand/collapse icons
- Arrow key navigation with selection highlight (inverted colors)
- Tab switching between Project Files and Code Base
- Sidebar auto-hides when terminal width < 60 cols

### M5.5 Input bar DONE

`src/ui/mod.rs::draw_input_area()` — Input with cursor, keybinding hints.

### M5.6 Event loop DONE

`src/main.rs::run_app()` — Non-blocking poll loop with mpsc channels.
Background thread spawning, message queue, auto-drain on completion.
Key routing: Shift+Tab, Ctrl+C, Ctrl+S, Enter, Esc, Backspace, PageUp/Down, Tab, Up/Down.

---

## Phase 6 — CodeBase & Search

### M6.1 Built-in code templates DONE

`src/codebase/` — 50+ templates embedded via include_str!. Tag parsing
(@LITE_DESC/@LITE_SCENE/@LITE_TAGS), search by tags/description.
`retrieval.rs` — Token-budget-aware template selection.

### M6.2 Web search DONE

`src/search/mod.rs` — DuckDuckGo HTML scraping, result truncation.
`src/search/cache.rs` — Disk cache with TTL expiry.
UI toggle (Ctrl+S) shows SEARCH:ON/OFF in status bar.
When enabled, web search runs automatically and results injected into LLM context.
`WebSearch` tool registered in ToolRegistry for agent loop access.

---

## Phase 7 — Diff & Edit Flow

### M7.1 Diff generation & display DONE

`src/util/diff.rs` — generate_diff(), generate_unified_diff(), apply_diff().
DiffLine enum (Context/Added/Removed) with similar crate.

### M7.2 Edit confirmation flow DONE

`/apply` command with diff preview, interactive y/n/a confirmation per file.
Auto mode: auto-apply changes, run syntax check, diagnostic self-correction.
Edit mode: interactive y/n/a confirmation before each file write.

---

## Phase 8 — Wiring & End-to-End

### M8.1 End-to-end integration DONE

All modules wired in main.rs. Config → wizard → Ollama → AppState →
terminal → event loop → agent pipeline → results. Skill system integrated.

### M8.2 Error handling & resilience DONE

`std::panic::catch_unwind` in main() restores terminal on panic.
Ollama connection error displayed in chat (no crash).
Model not found (404) handled. Empty model error. Message queue resilience.

---

## Phase 9 — Packaging & Distribution

### M9.1 Cross-platform builds DONE

GitHub Actions CI: ubuntu, macos, windows. cargo check/fmt/clippy/test.

### M9.2 NPM wrapper TODO

**Remaining**:
- [ ] package.json with bin/litepilot.js shim
- [ ] Platform-specific binary detection (darwin-arm64, darwin-x64, linux-x64, linux-arm64)
- [ ] npm publish setup

---
---

## Phase 10 — Agent Quality (v2.0)

The remaining phases (10-16) address the architectural gap between LitePilot
and production coding agents (DeepSeek-TUI, Claude Code, Codex). They are
organized by priority: P0 changes the core agent architecture, P1 improves
daily usability, P2 hardens the system, P3 adds advanced features.

Reference: `notes/deepseek-tui-agent-design.md`

---

### M10.1 Tool-Use Protocol & Agent Loop — P0 DONE

**Tasks:**
- [x] Create `src/tools/mod.rs` — `Tool` trait, `ToolDef`, `ToolResult`
- [x] Implement built-in tools: `read_file`, `write_file`, `edit_file`, `list_dir`, `exec_shell`, `web_search`
- [x] Create `src/agent/agent_loop.rs` — agent loop with max_steps guard
- [x] Wire Ollama tool definitions into `ChatRequest` via `chat_with_tools()`
- [x] Parse tool_use response blocks (JSON + text fallback) and dispatch to tools
- [x] Feed `tool_result` messages back into conversation for next LLM call
- [x] Add loop guard: detect and break on identical tool calls repeating
- [x] Register all tools (file ops, shell, web search) in `ToolRegistry`

---

### M10.2 Layered System Prompt Assembly — P0 DONE

**Tasks:**
- [x] Create `src/prompt.rs` — `PromptBuilder` struct
- [x] Define layers: base identity → mode overlay → skills → project context (static prefix)
- [x] Define volatile tail: working set summary + conversation summary + date/time
- [x] Store `PromptBuilder` in `AppState`, rebuild only when mode/skills/project changes
- [x] Inject project context (AGENTS.md / CLAUDE.md / .litepilot/instructions.md)
- [x] Add environment block: platform, version, shell, working directory
- [x] Preserve byte-identical prefix across turns for cache hits
- [x] Add `RECAP_SYSTEM` constant for post-summarization context continuity

---

### M10.3 Context Compaction with Summarization — P0 DONE

**Tasks:**
- [x] Create `src/agent/summarizer.rs` with `SummarizerConfig`, `SummaryResult`, `MessagePriority`
- [x] Add message pinning: error messages, file paths, code patches never summarized
- [x] Add `MessagePriority` enum (Normal, Pinned)
- [x] Implement `needs_summarization()` capacity checker
- [x] Implement `summarize()` using fast_model for background summarization
- [x] Add `compact_with_summary()` to `src/context.rs` — replaces truncation with LLM-powered compaction
- [x] Store conversation summary in `AppState` for injection into system prompt
- [x] Trigger summarization after `StreamDone` when >80% context used

---

## Phase 10.5 — Small-Model Cognitive Scaffolding (v2.05)

Reference: `notes/challenges-and-ideas-to-small-model-coding-agents.md`

Small models (4B-14B) need "cognitive scaffolding" to overcome three bottlenecks:
context window scarcity (lost-in-the-middle), instruction drift, and tool-use
reliability. Phase 10 built the foundations; this phase adds the scaffolding
that makes those foundations actually work reliably with small models.

---

### M10.5.1 Edge-Aware Prompt Construction — P0 DONE

**Tasks:**
- [x] Add `current_goal: Option<String>` and `completed_tasks: Vec<String>` to `PromptBuilder`
- [x] Set `current_goal` from the user's first message each turn (extracted in `spawn_request_for_mode`)
- [x] Add goal re-injection in `PromptBuilder::build()` volatile tail — places `## Current Objective` right before the user request
- [x] After `compact_with_summary()`, inject `[CURRENT OBJECTIVE]` + project instructions as the first history message after the system prompt
- [x] Add unit tests for goal placement and re-injection after summarization

**Files:** `src/prompt.rs`, `src/context.rs`, `src/main.rs`

---

### M10.5.2 Tool-Use Hardening for Small Models — P0 DONE

**Completed:**
- [x] Add `ToolRegistry::list_names()` returning all registered tool names
- [x] Add `ToolRegistry::has_tool()` and `validate_params()` — validate name + required fields
- [x] In `run_agent_loop()`, validate tool name exists before executing; if not, inject error with available tool list
- [x] Validate required parameters before execution; inject specific missing-param errors
- [x] Add `TOOL_CORRECTION_PROMPT` to `src/agent/prompts.rs` — shows both JSON and text format examples
- [x] Add `looks_like_failed_tool_call()` detection for malformed attempts
- [x] Modify `parse_tool_calls()` to return `ParseResult` with diagnostic info (ParseDiagnostics with hints_found + failure_reasons)
- [x] In `run_agent_loop()`, when parse fails but looks like a tool attempt, inject correction + continue loop (max 2 retries)
- [x] Add `REFLEXION_PROMPT` — on final retry attempt, ask model to verbalize what went wrong
- [x] Add unit tests for parse diagnostics, failed attempt detection, and correction formatting

---

### M10.5.3 Hierarchical Planning for Instruction Drift — P1 DONE

**Problem:** Planning is single-level (flat list of steps). Small models
become reactive to the most recent error instead of following a strategic
plan. No mechanism detects when the model has drifted from the original goal.

**Target:** Two-phase planning: strategic goal + operational steps. The
strategic goal is re-injected into every step's context. Drift detection
checks if the current response is still relevant to the active phase.

**Tasks:**
- [x] Extend `Plan` struct with `strategic_goal: String` field (the one-line user objective)
- [x] Modify planner prompt to extract strategic goal as first line, then operational steps
- [x] In plan-based execution, inject `[STRATEGIC GOAL: ...]` into each step's system context
- [x] Add `detect_drift(goal: &str, response: &str) -> bool` — checks if response mentions topics unrelated to goal (simple keyword overlap heuristic)
- [x] On drift detection, inject a warning: "You are drifting from the objective. Refocus on: {goal}"
- [x] Add unit tests for drift detection and goal re-injection

**Files:** `src/agent/planner.rs`, `src/agent/mod.rs`, `src/agent/retry.rs`

---

### M10.5.4 Semantic Reranking for Template Retrieval — P1 DONE

**Problem:** `codebase/retrieval.rs` uses a single LLM call to select templates
from a catalog. No two-stage retrieve-then-rerank pipeline. For small models,
noisy context (irrelevant templates) wastes precious context window and
degrades output quality.

**Target:** Two-stage retrieval: (1) broad candidate selection from catalog,
(2) fast_model reranking with code-aware scoring. Only top-K within budget
are injected into context.

**Tasks:**
- [x] Add `retrieve_with_reranking()` to `src/codebase/retrieval.rs`
- [x] Stage 1: existing `select()` call returns broad candidate set (top 10)
- [x] Stage 2: build rerank prompt with candidate code snippets (first 500 chars each), ask fast_model to rank by semantic relevance
- [x] Load only top-K reranked templates within token budget
- [x] Fall back to existing `retrieve()` if reranking fails (non-blocking)
- [x] Add `RERANK_SYSTEM` prompt to `src/agent/prompts.rs`
- [x] Add unit tests for reranking prompt construction and fallback behavior

**Files:** `src/codebase/retrieval.rs`, `src/agent/prompts.rs`

---

### M10.5.5 Diagnostic-Based Self-Correction — P1 DONE

**Tasks:**
- [x] Create `src/agent/diagnostics.rs` — `run_diagnostics(path, sandbox) -> DiagnosticResult`
- [x] `DiagnosticResult` contains `errors: Vec<DiagnosticError>` with file, line, message
- [x] After `auto_apply_changes()`, spawn background diagnostic run on written files
- [x] Send `DiagnosticReady` via channel; event loop displays errors in UI
- [x] `DiagnosticResult::format_for_correction()` builds correction prompt from actual errors
- [x] Non-blocking: diagnostic failure doesn't block the agent loop, just skips correction
- [x] Add `DIAGNOSTIC_CORRECTION_PROMPT` to `src/agent/prompts.rs`
- [x] Add unit tests for diagnostic result formatting and correction prompt

**Files:** `src/agent/diagnostics.rs`, `src/agent/prompts.rs`, `src/main.rs`

---

## Phase 11 — Usability (v2.1)

---

### M11.1 Working Set Tracking — P1 DONE

**Tasks:**
- [x] Create `src/working_set.rs` — `WorkingSet` with frecency-based ranking
- [x] Hook into file write/read tool results to observe paths
- [x] Prune to max 20 entries by frequency + recency
- [x] Inject `working_set.summary()` into volatile section of system prompt

---

### M11.2 Session Resume & Auto-Save — P1 DONE

**Tasks:**
- [x] Auto-save session after each StreamDone / RetryResult::Success
- [x] Add `--resume` and `--resume <id>` CLI flags
- [x] Add `--sessions` flag to list sessions
- [x] Load conversation history from resumed session into AppState

---

### M11.3 Project Context Auto-Discovery — P1 DONE

**Tasks:**
- [x] Search priority: AGENTS.md → CLAUDE.md → .litepilot/instructions.md → README.md
- [x] Auto-generate instructions from: project name, language, structure, build/test commands
- [x] Save auto-generated `.litepilot/instructions.md` for user customization
- [x] Inject discovered context into PromptBuilder static layer

---

### M11.4 Optimized First-Run Initialization — P1 DONE

**Tasks:**
- [x] Ollama ping already runs in background thread
- [x] Add crash dump handler: `std::panic::set_hook` → write to `~/.litepilot/crashes/`

---

### M11.5 Session Recap — P1 DONE

**Tasks:**
- [x] Create `src/recap.rs` — `generate_recap(client, messages, config) -> Result<String>`
- [x] Add `/recap` slash command handler in `main.rs` event loop
- [x] Add end-of-turn recap after Auto mode with >2 file changes (guard with config flag)
- [x] Add config flags: `enable_recap`, `enable_away_summary` in `src/config.rs`

**Deferred:**
- Away summary on terminal focus regain (requires complex crossterm focus event tracking)

---

## Phase 12 — Production Hardening (v2.2)

---

### M12.1 Multi-Provider LLM Client — DROPPED

LitePilot is local-first with Ollama. Multi-provider support is out of scope.

---

### M12.2 Streaming Guardrails & Resilient Transport — P2 DONE

**Tasks:**
- [x] Add `MAX_CONTENT_BYTES` (10 MB) guard in `chat_stream()`
- [x] Add `MAX_DURATION` (30 min) wall-clock limit
- [x] Add `MAX_ERRORS` (5) error tolerance — stream read errors and JSON parse errors tolerated individually
- [x] Error counter resets per-category, `total_bytes` tracks cumulative content size
- [x] Wall-clock timeout checked each loop iteration via `std::time::Instant`

**Files:** `src/ollama/chat.rs`

---

### M12.3 Risk-Classified Approval System — P2 DONE

**Tasks:**
- [x] Create `src/approval.rs` — `RiskLevel` (Safe, Write, Destructive)
- [x] Classify tools: read_file/list_dir = safe, write_file/edit_file = write, exec_shell = side-effect
- [x] Destructive operations (delete, rm) require two-key confirmation (YY)
- [x] Add `ApprovalCache`: HashSet of approved tool signatures, auto-approve for session
- [x] Skip approval for cached items — show "[cached]" prefix on auto-approved
- [x] `ApprovalCache` stored in `AppState`, persists for entire session
- [x] 14 unit tests for classification, caching, and decision logic

**Files:** `src/approval.rs`, `src/app.rs`, `src/main.rs`

---

### M12.4 Auto Model Routing — P2 DONE

**Tasks:**
- [x] Create `src/router.rs` — `classify_request()` heuristic
- [x] Keywords: question words → Fast, "create/fix/implement" → Core, "review/audit/check" → Audit
- [x] Add config flag: `auto_model_routing = false` (opt-in)
- [x] Respect manual model selection when `auto_model_routing = false`

**Files:** `src/router.rs`, `src/main.rs`, `src/config.rs`

---

## Phase 13 — Advanced Features (v2.3)

---

### M13.1 Workspace Snapshots (Side-Git) — P3 DONE

**Tasks:**
- [x] Create `src/snapshot.rs` — `SnapshotManager`
- [x] Implement `pre_turn()` / `post_turn()` using side git
- [x] Add retention: 7 days, 50 snapshots max, `prune()` method
- [x] Add `/undo`, `/restore <hash>`, `/snapshots` slash commands
- [x] Non-fatal: snapshot failures never block TUI operation
- [x] Pre-turn snapshot before each LLM request; post-turn after Auto mode changes
- [x] 8 unit tests (init, create, restore, hash stability)

**Files:** `src/snapshot.rs`, `src/app.rs`, `src/main.rs`

---

### M13.2 Structured Event Hooks — P3 DONE

**Tasks:**
- [x] Create `src/hooks.rs` — `HookEvent` enum (TurnStarted, ToolCalled, ToolResult, TurnComplete, Error)
- [x] Implement `JsonlSink` → write to `~/.litepilot/logs/events.jsonl`
- [x] Emit TurnStarted on each LLM request, TurnComplete on result
- [x] Emit Error events on pipeline failures
- [x] JSONL format: one JSON object per line, tagged with `type` field
- [x] 8 unit tests (serialization, file writing, append mode, directory creation)

**Files:** `src/hooks.rs`, `src/app.rs`, `src/main.rs`

---

### M13.3 OS-Level Sandboxing — P3 DONE

**Tasks:**
- [x] Create `src/sandbox/seatbelt.rs` — macOS `sandbox-exec` integration
- [x] Create `src/sandbox/landlock.rs` — Linux Landlock placeholder with kernel version detection
- [x] Build Seatbelt sandbox policy: allow read from /usr, read/write from workspace, allow network
- [x] Add `run_os_sandboxed()` to Executor — opt-in OS-level isolation
- [x] Fallback to allowlist/blocklist on unsupported platforms
- [x] 8 new tests (profile generation, availability checks, kernel version parsing)

**Files:** `src/sandbox/seatbelt.rs`, `src/sandbox/landlock.rs`, `src/sandbox/executor.rs`, `src/sandbox/mod.rs`

---

### M13.4 LSP Post-Edit Diagnostics — P3 DONE

**Tasks:**
- [x] Create `src/lsp.rs` — lightweight LSP client over stdio (JSON-RPC)
- [x] Auto-detect language server from file extension (rs→rust-analyzer, py→pyright, ts/tsx→typescript-language-server)
- [x] After file writes in Auto mode and cached approvals, query LSP diagnostics
- [x] Display diagnostics in TUI as System messages with line, severity, and message
- [x] Non-blocking: LSP failure doesn't block tool result, silently skipped
- [x] LSP client spawns server, sends initialize/didOpen, reads diagnostics, shuts down
- [x] 5 unit tests (file detection, URI format, constructors)

**Files:** `src/lsp.rs`, `src/main.rs`

---

## Implementation Priority Summary

| Priority | Milestones | Impact |
|----------|-----------|--------|
| **P0** | M10.1 (Tool-use + agent loop), M10.2 (Layered prompts), M10.3 (Summarization) | Core agent quality — DONE |
| **P0** | M10.5.1 (Edge-aware prompts), M10.5.2 (Tool-use hardening + correction retry) | Small-model reliability — DONE |
| **P1** | M10.5.3 (Hierarchical planning), M10.5.4 (Semantic reranking), M10.5.5 (Diagnostic self-correction) | Small-model quality — DONE |
| **P1** | M11.1 (Working set), M11.2 (Session resume), M11.3 (Project context), M11.4 (Init optimization), M11.5 (Session recap) | Daily usability — DONE |
| **P2** | M12.2 (Stream guardrails), M12.3 (Risk approval), M12.4 (Auto routing) | Production hardening — DONE |
| **P3** | M13.1 (Snapshots), M13.2 (Hooks), M13.3 (OS sandbox), M13.4 (LSP) | Enterprise-grade features — DONE |
| **P1** | M14.1–M14.4 | Agent reliability & transport hardening — TODO |
| **P2** | M14.5–M14.7 | KV cache optimization & context management — TODO |
| **P3** | M14.8 | Hardware-aware adaptation — TODO |

---

## The Five Core Patterns

All production coding agents share these patterns. LitePilot needs all five
to close the gap:

1. **Tool-use loop** — LLM calls tools, sees results, decides next action.
   Without this, you have a chatbot, not an agent.

2. **Context preservation** — Summarization + working set + project context.
   The agent remembers what it did, even in long sessions.

3. **Cache-stable prompts** — Layered prompts with byte-identical prefix.
   Critical for local model performance (Ollama KV cache reuse).

4. **Resilient transport** — Stream guardrails, transparent retry, error
   classification. The agent doesn't crash on network hiccups.

5. **Safety nets** — Snapshots for undo, risk-classified approval, OS
   sandboxing. Users trust the agent when they can undo its mistakes.

---

## Test Infrastructure

| Layer | Tool | Scope |
|-------|------|-------|
| Unit tests | `#[test]` + `wiremock` + `tempfile` | Each module in isolation |
| Property tests | `proptest` | Config parsing, diff, token math |
| Snapshot tests | `insta` | TUI rendering, diff display |
| Integration tests | `#[ignore]` + live Ollama | Full pipeline, file ops |
| CI | GitHub Actions | Build + test on 3 OS |

```bash
cargo test                          # All unit + snapshot tests (160 tests)
cargo test -- --ignored             # Integration tests (needs Ollama)
cargo clippy -- -D warnings         # Zero warnings
cargo fmt --check                   # Zero formatting issues
```

---

## Phase 14 — Agent Reliability & KV Cache Optimization (v3.0)

Reference: `notes/guide-to-ollama-kv-cache-and-context-management.md`,
`notes/challenges-and-ideas-to-small-model-coding-agents.md`,
`notes/deepseek-tui-agent-design.md`

Phases 10–13 built the core agent. Phase 14 closes the remaining gaps
identified by comparing with the design notes. Focus areas: accurate token
management, tool execution efficiency, transport resilience, and KV cache
prefix stability.

---

### M14.1 Accurate Token Counting via `/api/tokenize` — P1 TODO

**Problem:** Context window management (`context::build_messages`,
`context_usage_percent`) relies on heuristic token estimation (~4 chars/token).
This is inaccurate — can overshoot (truncating too aggressively) or undershoot
(risking context overflow and Ollama errors). The KV cache guide explicitly
recommends using `/api/tokenize` for precise counts.

**Target:** Use Ollama's `/api/tokenize` endpoint for actual token counts in
context budget calculations. Fall back to estimation only when the endpoint is
unavailable.

**Tasks:**
- [ ] Add `tokenize(model, prompt) -> Result<usize>` to `OllamaClient` — calls `POST /api/tokenize`
- [ ] Cache tokenize results per model (HashMap<String, usize>) to avoid redundant API calls for the same prompt
- [ ] Replace `estimate_prompt_tokens()` calls in `context::build_messages()` with `tokenize()` for budget calculation
- [ ] Use `tokenize()` in context overflow detection (`ContextManager::context_usage_percent`) when the model is known
- [ ] Keep `estimate_prompt_tokens()` as fallback for offline/tokenize-failure cases
- [ ] Add context budget test: build_messages stays within 90% of window with actual token counts
- [ ] Add wiremock test for tokenize endpoint with mocked responses

**Files:** `src/ollama/mod.rs`, `src/ollama/chat.rs`, `src/context.rs`, `src/util/text.rs`

---

### M14.2 Parallel Tool Execution — P1 TODO

**Problem:** The agent loop executes tools one at a time, even when multiple
read-only tools (read_file, list_dir, search_files) could run concurrently.
This is a significant latency penalty, especially for multi-tool agent turns
where the LLM requests 3–5 file reads.

**Target:** Classify tools as parallel-safe (read-only) or serial (writes,
commands). Execute parallel-safe tools concurrently using `join_all`. Serial
tools execute one at a time with approval gates.

**Tasks:**
- [ ] Add `ToolCapability` enum to `src/tools/mod.rs`: `ReadOnly`, `Write`, `SideEffect`
- [ ] Add `fn capabilities(&self) -> Vec<ToolCapability>` to the tool trait
- [ ] Classify existing tools: read_file/list_dir/search_files = ReadOnly, write_file/edit_file = Write, exec_shell = SideEffect
- [ ] In `agent_loop.rs`, split tool calls into batches: parallel-safe batch vs serial queue
- [ ] Execute parallel batch with `tokio::join!` or `futures::join_all`
- [ ] Collect all parallel results before feeding back to LLM
- [ ] Maintain ordering in UI output (show parallel results in tool-call order)
- [ ] Add integration test: 3 parallel reads complete faster than 3 serial reads

**Files:** `src/tools/mod.rs`, `src/agent/agent_loop.rs`, `src/tools/file_ops.rs`, `src/tools/search.rs`

---

### M14.3 Fake Tool Call Detection & Stream Hardening — P1 TODO

**Problem:** Small models sometimes output text that mimics tool-call markers
(e.g., `[TOOL_CALL]`, `<tool_call`) in their regular text responses. This can
cause the parser to misinterpret prose as a tool invocation. DeepSeek-TUI
scrubs these markers from text output to prevent prompt injection.

Separately, if a streaming connection drops early (first few chunks), the user
sees a confusing error. DeepSeek-TUI auto-retries transparently (up to 2x)
before surfacing the error.

**Target:** (a) Scrub forged tool-call markers from text output. (b) Add
transparent stream retry on early failures.

**Tasks:**
- [ ] Add `scrub_tool_call_markers(text: &str) -> String` to `src/agent/tools_parser.rs`
- [ ] Detect and remove: `[TOOL_CALL]`, `<tool_call`, `<function_call`, `### TOOL:` and similar patterns
- [ ] Apply scrubbing to text chunks in `agent_loop.rs` before displaying
- [ ] Add `MAX_TRANSPARENT_STREAM_RETRIES = 2` constant
- [ ] In `generate_stream()`, if the stream fails before receiving any content chunks, retry automatically
- [ ] Only surface error to user after all transparent retries exhausted
- [ ] Add unit tests for marker scrubbing with various patterns
- [ ] Add wiremock test: stream fails on first attempt, succeeds on retry

**Files:** `src/agent/tools_parser.rs`, `src/ollama/chat.rs`, `src/agent/agent_loop.rs`

---

### M14.4 Exponential Backoff for Ollama Retries — P1 TODO

**Problem:** `chat_with_retry()` retries immediately on failure. When Ollama
is under load (e.g., model loading, GPU memory pressure), immediate retries
compound the problem. DeepSeek-TUI uses exponential backoff with jitter for
transient errors.

**Target:** Add configurable backoff between retries. Distinguish between
retryable errors (timeout, 500, connection reset) and non-retryable errors
(404 model not found, 400 bad request).

**Tasks:**
- [ ] Add `LlmError` enum to `src/ollama/mod.rs` with variants: `Timeout`, `ServerError(status)`, `NetworkError`, `NotFound`, `InvalidRequest`
- [ ] Add `LlmError::is_retryable() -> bool` — timeout/500/network = true, 404/400 = false
- [ ] In `chat_with_retry()`, sleep with exponential backoff (1s, 2s, 4s) + random jitter (0–500ms) between retries
- [ ] Skip retry entirely for non-retryable errors (return immediately with clear error message)
- [ ] Log backoff timing: "retry {n}/{max} in {delay}ms for {error}"
- [ ] Add config: `retry_backoff_base_ms = 1000` (default 1s base)
- [ ] Add wiremock test: server returns 500, then 200 on retry with timing assertion

**Files:** `src/ollama/mod.rs`, `src/agent/retry.rs`, `src/config.rs`

---

### M14.5 KV Cache Prefix Stability Audit — P2 TODO

**Problem:** M10.2 implemented layered prompts with a static/volatile split,
but the KV cache only gets a hit when the **byte-identical prefix** matches.
If any part of the "static" layer changes between turns (e.g., date string
leaking into the wrong layer, working set changing the injected file list),
the entire prefix cache misses and all tokens must be recomputed.

**Target:** Audit and guarantee that the static prefix is truly byte-stable
across turns. Add a runtime verification mechanism.

**Tasks:**
- [ ] Audit `PromptBuilder::build()` to verify static layers don't include volatile data
- [ ] Ensure `current_datetime()` is only in the volatile tail (verify it's not in system prompt, project context, or mode overlay)
- [ ] Ensure working set summary is only in the volatile tail
- [ ] Ensure conversation summary is only in the volatile tail
- [ ] Add `PromptBuilder::static_hash() -> u64` — hash of the static prefix for cache validation
- [ ] Log static hash at start of each request: if hash changes between turns, log warning with diff
- [ ] Add unit test: build() called twice without config changes produces identical static prefix
- [ ] Add integration test: KV cache hit rate > 80% on second turn with same model

**Files:** `src/prompt.rs`, `src/main.rs`

---

### M14.6 Seam-Based Context Compaction — P2 TODO

**Problem:** Current compaction uses LLM summarization (M10.3) which replaces
older messages with a summary. This works but is lossy — specific code snippets,
error messages, and file paths from earlier turns are compressed away. The
DeepSeek-TUI design notes describe a "seam" approach: progressive compression
with a verbatim recent window.

**Target:** Multi-level context preservation. Recent turns (last N) stay
verbatim. Older turns get progressively compressed. Important messages (errors,
file changes, pinned) are always preserved.

**Tasks:**
- [ ] Add `SeamLevel` enum: `Verbatim`, `Compressed(summary)`, `Archived(dense_summary)`
- [ ] Define soft seams at context thresholds (e.g., 50%, 75%, 90%)
- [ ] Recent window: last 6 messages always verbatim (no summarization)
- [ ] Mid window: messages older than 6 but within 50% of context get compressed to summaries
- [ ] Deep window: messages beyond mid window get archived to dense one-line summaries
- [ ] Pinned messages (errors, file paths, code patches) are never summarized
- [ ] Progressive recompaction: on each new summarization trigger, existing seams can be fused into denser blocks
- [ ] Replace current `maybe_compact()` + `compact_with_summary()` with seam-based approach
- [ ] Add unit tests for seam level assignment and progressive compression

**Files:** `src/context.rs`, `src/agent/summarizer.rs`

---

### M14.7 Config Propagation Fix for Background Threads — P2 TODO

**Problem:** Background threads (plan step, summarizer, recap) construct their
own `Config` with `..Config::default()`, which can miss user-configured values
like `context_window_limit`. This caused the "fast model unavailable" bug
(fixed in commit immediately after v2.3). The fix should be systemic: all
background threads receive a complete config snapshot.

**Target:** Ensure all background thread spawns receive a complete, consistent
config rather than partial defaults.

**Tasks:**
- [ ] Audit all `std::thread::spawn` calls that create `OllamaClient` or `Config`
- [ ] Identify every `..Config::default()` usage in background thread construction
- [ ] For each: either (a) pass the full `app_state.config.clone()` or (b) pass only the specific fields needed via a dedicated struct
- [ ] Add `BackgroundConfig` struct or similar to make the contract explicit — which fields are needed by each background operation
- [ ] Verify `context_window_limit` is propagated everywhere `num_ctx` matters
- [ ] Add tracing: log the actual `num_ctx` and `model` used by each background operation

**Files:** `src/main.rs`, potentially `src/config.rs`

---

### M14.8 Hardware-Aware Inference Adaptation — P3 TODO

**Problem:** Ollama may run on GPU (fast inference, large KV cache) or CPU
(slow inference, memory-constrained). The agent has no awareness of this and
uses the same timeouts, context windows, and retry strategies regardless. The
KV cache guide recommends using `/api/show` for hardware detection.

**Target:** Detect GPU/CPU mode at startup and adapt timeouts, context
management, and UI indicators accordingly.

**Tasks:**
- [ ] Add `detect_hardware(endpoint, model) -> HardwareInfo` to `OllamaClient` — calls `GET /api/show`
- [ ] `HardwareInfo` struct: `gpu: bool`, `vram_mb: Option<u64>`, `device_family: Option<String>` (cuda/metal/rocm/cpu)
- [ ] Run hardware detection during startup, after model selection
- [ ] Adapt timeouts: CPU mode uses longer timeouts (600s vs 300s for streaming)
- [ ] Show hardware indicator in status bar: `GPU:metal` / `CPU`
- [ ] On CPU: warn if context_window_limit > 32K — recommend smaller window
- [ ] Log hardware info at startup for diagnostics
- [ ] Fall back gracefully if `/api/show` doesn't include hardware fields

**Files:** `src/ollama/mod.rs`, `src/ui/mod.rs`, `src/config.rs`, `src/main.rs`

---

## Phase 14 — Implementation Order

```
M14.7 Config propagation fix        (fix known bugs first)
  → M14.1 Token counting            (foundation for context management)
  → M14.5 KV cache prefix audit     (verify existing cache works correctly)
  → M14.4 Exponential backoff       (transport resilience)
  → M14.3 Fake tool call detection  (agent reliability)
  → M14.2 Parallel tool execution   (performance)
  → M14.6 Seam-based compaction     (advanced context management)
  → M14.8 Hardware detection        (polish)
```

M14.7 and M14.1 are highest priority because they fix known bugs and provide
the foundation (accurate token counts) that other improvements build on.
M14.5 validates that the existing KV cache system actually works as intended.
M14.2–M14.4 improve the agent loop reliability and performance.
M14.6 and M14.8 are quality-of-life improvements for later.
