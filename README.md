# LitePilot

I live in your terminal. I'm an AI coding assistant, but I don't call home to any cloud — I think entirely on your hardware, through local models hosted by Ollama. I'm written in Rust, because I wanted to be fast and I wanted to be yours.

No cloud. No API keys. Nothing leaves your machine. I run on three local models — a small one when I need quick reflexes, a medium one when it's time to do the real work, and a large one when I need to check my own thinking.

## What I can do for you

**I work in three modes.** I can Plan (read-only — I look, I think, I tell you what I see). I can Edit (I propose changes, and you approve them with `/apply`). Or I can go Auto — I plan, implement, review, and apply everything in one sandboxed pass. You switch between them with Shift+Tab.

**I have skills.** `/review` for code audits. `/explain` when you want to understand something. `/simplify` for refactoring. `/test` for generating tests. `/search` for finding things. You can teach me new skills too — drop a `.md` file in `~/.litepilot/skills/` and I'll learn it.

**I correct myself.** I validate my own output. When I produce something malformed — and local models sometimes do — I retry with an explanation of what went wrong. I back off exponentially so I don't overwhelm Ollama while I'm figuring it out.

**I manage my own memory carefully.** I use Ollama's `/api/generate` endpoint and track the KV cache context handle myself, reusing cached key-value tensors across turns. I split my system prompt into static layers (byte-identical every turn) and a volatile tail (per-turn data like the date and your working set) so the KV cache prefix matches reliably. After each response, I show you the cache hit rate and warn you when context is getting full.

**I stream my thoughts.** You see them as they form, token by token.

**I don't make you wait.** Type while I'm thinking — your messages queue up and I handle them in order.

**I remember our conversations.** Sessions saved to `~/.litepilot/sessions/`. Resume anytime with `--resume`.

**I can search the web.** Optional DuckDuckGo search, cached locally on your disk.

## Getting Started

### 1. Install Ollama and pull models

I need models to think. Install Ollama first:

```bash
curl -fsSL https://ollama.com/install.sh | sh

# One model is enough to start
ollama pull qwen3:4b

# But three is better — each tier of my thinking uses a different one
ollama pull qwen3:4b    # Fast  — routing, search, quick answers
ollama pull qwen3:8b    # Core  — coding, generation, real work
ollama pull qwen3:14b   # Audit — review, quality assurance
```

### 2. Install me

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

The first time we meet, I'll walk you through setup — your Ollama URL and model selection. After that, I remember.

## How to talk to me

Ask me anything about your code:

```
What does the handle_input function do in src/main.rs?
```

I'll read your files and answer in context. Or ask me to build something:

```
Create a Python REST API with Flask for a todo list with CRUD endpoints
```

I'll respond with file changes. Type `/apply` to write them to disk.

### How I manage my KV cache

I track the context handle from every Ollama response. Each turn, I reuse the cached tensors from the last one so I'm not recomputing what I already know. I keep my system prompt split into static layers (identical across turns) and a volatile tail (date, working set — rebuilt each turn) so the prefix always matches. The status bar shows how full my context is (`ctx:N%`), and after each response you'll see:

```
KV cache: 94.2% hit (1920 cached, 128 recomputed, 256 generated)
```

When I start running out of room:
```
Context 82% full (3328/4096 tokens). Consider /clear to start fresh.
```

Use `/clear` to reset my context and start a fresh session.

### How I adapt to the model I'm running on

I tailor my prompts to what the model can handle. Small models get short, directive instructions — no fluff. Medium models get examples. Large models get full, nuanced guidance. My code generation protocol (`### FILE:`, `### ACTION:`) is simple enough that even the smallest tier can produce it reliably.

## Key Bindings

| Key | What happens |
|-----|-------------|
| `Enter` | Send your input to me |
| `Shift+Enter` | Insert a newline |
| `Shift+Tab` | Switch my mode (Plan → Edit → Auto) |
| `Ctrl+Tab` | Toggle my thinking mode |
| `Ctrl+C` | Quit (double-press in Auto mode) |
| `Esc` | Cancel plan / scroll to bottom |
| `PageUp` / `PageDown` | Scroll chat history |
| `Up` / `Down` | Navigate your input history |

## Slash Commands

| Command | What I do |
|---------|-----------|
| `/clear` | Clear my context and conversation history |
| `/skills` | List all skills I know |
| `/setup` | Re-run the setup wizard |
| `/apply` | Write file changes from my last response |
| `/run <cmd>` | Execute a sandboxed shell command |
| `/uv <subcmd>` | UV toolchain (init, venv, add, run) |
| `/snapshots` | List recent file snapshots |
| `/undo` | Restore last snapshot |
| `/restore <hash>` | Restore a specific snapshot |
| `/recap` | Generate a session recap |
| `/quit` or `/exit` | End our session |

## Configuration

I read my config from `~/.litepilot/config.toml` (or `.litepilot/config.toml` in your project root):

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

## How I'm built

```
src/
├── main.rs              My event loop, channel bridge, request routing
├── app.rs               My state: mode, config, context manager, pending queue
├── context.rs           My memory: budget-aware truncation, LLM summarization
├── prompt.rs            My layered system prompt construction
├── config.rs            My TOML config, project-local + global loading
├── wizard.rs            My first-run setup wizard
├── ollama/              How I talk to Ollama + ContextManager (KV cache lifecycle)
│                          /api/generate (streaming, cache reuse)
│                          /api/chat (blocking, for skills)
│                          /api/tokenize (accurate token counting)
├── agent/               How I plan, edit, retry with exponential backoff,
│                          run my tool-use agent loop, summarize, check syntax,
│                          run diagnostics, scrub fake tool calls
├── tools/               Tool definitions I use in the agent loop (file ops, search, shell)
├── sandbox/             My security: path validation, command filtering, platform sandboxes
├── search/              DuckDuckGo search with disk cache
├── project/             File tree, git status, file operations, UV toolchain
├── codebase/            Template library, tag search, context retrieval
├── session/             Session persistence (JSON)
├── skills/              My loadable skills, stored as markdown with YAML frontmatter
├── ui/                  How I render myself in your terminal (ratatui), status bar with context indicator
└── util/                Diff generation, text processing, token estimation
```

## My sandbox

I don't trust myself blindly, and neither should you.

- **Path validation**: I won't follow `..` traversal, I won't chase symlinks out of your workspace, I won't write outside your project
- **Command filtering**: I can run `cargo`, `python`, `npm`, `git`, `make` — but not `sudo`, not `rm -rf /`, not anything that escapes
- **Mode enforcement**: In Plan mode, file writes are blocked at the code level — I physically cannot do it
- **Platform sandboxes**: Linux Landlock, macOS Seatbelt — OS-level containment

---

I was built by **Dr. Liam Ning**, with Claude Code powered by GLM-5.1. 

License: MIT
