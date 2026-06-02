# LitePilot

Terminal AI coding assistant powered by Ollama-host local models. Written in Rust.

No cloud. No API keys. No data leaving your hardware. Runs on local Ollama models — a small one for quick reflexes, a medium one for the heavy lifting, a large one to check its own work.

## What it does

**Three modes.** Plan (read-only analysis), Edit (propose changes, you approve with `/apply`), Auto (plan, implement, review, and apply in one sandboxed pass).

**Skills.** `/review` for code audits, `/explain` for understanding, `/simplify` for refactoring, `/test` for generating tests, `/search` for finding things. Add custom skills by dropping `.md` files in `~/.litepilot/skills/`.

**Self-correction.** Validates its own output. When it produces malformed code blocks, it retries with an explanation of what went wrong. Retries use exponential backoff so Ollama isn't overwhelmed.

**KV cache management.** Uses Ollama's `/api/generate` endpoint with manual context handle tracking for KV cache reuse across turns. Layered system prompts keep a byte-identical static prefix across turns for maximum cache hits. Shows cache hit rate after each response and warns when context is getting full.

**Streaming.** Shows thoughts as they form, token by token.

**Message queuing.** Type while it's thinking — messages queue up and are handled in order.

**Session persistence.** Conversations saved to `~/.litepilot/sessions/`. Resume with `--resume`.

**Web search.** Optional DuckDuckGo search, cached locally.

## Getting Started

### 1. Install Ollama and pull models

```bash
curl -fsSL https://ollama.com/install.sh | sh

# One model is enough to start
ollama pull qwen3:4b

# Three is better — each tier thinks differently
ollama pull qwen3:4b    # Fast  — routing, search, quick answers
ollama pull qwen3:8b    # Core  — coding, generation, real work
ollama pull qwen3:14b   # Audit — review, quality assurance
```

### 2. Install LitePilot

```bash
# Build from source
git clone https://github.com/csningli/litepilot-tui.git
cd litepilot-tui && cargo install --path .

# Or via npm
npm install -g litepilot-tui
```

### 3. Run

```bash
ollama serve
cd ~/my-project
litepilot
```

First launch walks you through setup — Ollama URL and model selection. After that, it remembers.

## Usage

```
What does the handle_input function do in src/main.rs?
```

It reads your files and answers in context. Or:

```
Create a Python REST API with Flask for a todo list with CRUD endpoints
```

It responds with file changes. Type `/apply` to write them to disk.

### KV Cache Context Management

LitePilot tracks the KV cache context handle from Ollama's `/api/generate` responses. Each turn reuses the cached key-value tensors from the previous turn, avoiding redundant computation. The system prompt is split into static layers (identical across turns) and a volatile tail (per-turn data like date and working set) so the KV cache prefix matches reliably. The status bar shows context usage (`ctx:N%`), and after each response you'll see the cache hit rate:

```
KV cache: 94.2% hit (1920 cached, 128 recomputed, 256 generated)
```

When context fills up:
```
Context 82% full (3328/4096 tokens). Consider /clear to start fresh.
```

Use `/clear` to reset the context and start a fresh session.

### How it adapts to model size

Prompts are tailored to the model's capability: small models get short, directive instructions; medium models get examples; large models get full, nuanced guidance. Code generation uses a simple protocol (`### FILE:`, `### ACTION:`) that even the smallest tier can produce reliably.

## Key Bindings

| Key | Action |
|-----|--------|
| `Enter` | Send input |
| `Shift+Enter` | Insert newline |
| `Shift+Tab` | Switch mode (Plan → Edit → Auto) |
| `Ctrl+Tab` | Toggle thinking mode |
| `Ctrl+C` | Quit (double-press in Auto mode) |
| `Esc` | Cancel plan / scroll to bottom |
| `PageUp` / `PageDown` | Scroll chat history |
| `Up` / `Down` | Navigate input history |

## Slash Commands

| Command | Description |
|---------|-------------|
| `/clear` | Clear context and conversation history |
| `/skills` | List all available skills |
| `/setup` | Re-run the setup wizard |
| `/apply` | Write file changes from the last response |
| `/run <cmd>` | Execute a sandboxed shell command |
| `/uv <subcmd>` | UV toolchain (init, venv, add, run) |
| `/snapshots` | List recent file snapshots |
| `/undo` | Restore last snapshot |
| `/restore <hash>` | Restore specific snapshot |
| `/recap` | Generate session recap |
| `/quit` or `/exit` | End the session |

## Configuration

`~/.litepilot/config.toml` (or `.litepilot/config.toml` in project root):

```toml
ollama_endpoint = "http://127.0.0.1:11434"
fast_model = "qwen3:4b"
core_model = "qwen3:8b"
audit_model = "qwen3:14b"
default_mode = "edit"
max_retries = 3
context_window_limit = 262144

[theme]
primary = "cyan"
accent = "magenta"
warning = "yellow"
```

## Architecture

```
src/
├── main.rs              Event loop, channel bridge, request routing
├── app.rs               AppState: mode, config, context manager, pending queue
├── context.rs           Message history: budget-aware truncation, LLM summarization
├── prompt.rs            Layered system prompt construction
├── config.rs            TOML config, project-local + global loading
├── wizard.rs            First-run setup wizard
├── ollama/              OllamaClient + ContextManager (KV cache handle lifecycle)
│                          /api/generate (streaming, cache reuse)
│                          /api/chat (blocking, for skills)
│                          /api/tokenize (accurate token counting)
├── agent/               Planning, editing, retry with exponential backoff,
│                          tool-use agent loop, summarization, syntax checking,
│                          diagnostics, fake tool call scrubbing
├── tools/               Tool definitions for agent loop (file ops, search, shell)
├── sandbox/             Path validation, command filtering, platform sandboxes
├── search/              DuckDuckGo search with disk cache
├── project/             File tree, git status, file operations, UV toolchain
├── codebase/            Template library, tag search, context retrieval
├── session/             Session persistence (JSON)
├── skills/              Loadable skills, stored as markdown with YAML frontmatter
├── ui/                  Terminal rendering (ratatui), status bar with context indicator
└── util/                Diff generation, text processing, token estimation
```

## Sandbox

- **Path validation**: No `..` traversal, no symlinks out of workspace, no writing outside the project
- **Command filtering**: Can run `cargo`, `python`, `npm`, `git`, `make` — not `sudo`, `rm -rf /`, or anything that escapes
- **Mode enforcement**: File writes blocked at code level in Plan mode
- **Platform sandboxes**: Linux Landlock, macOS Seatbelt

---

Built by **Dr. Liam Ning**

License: MIT
