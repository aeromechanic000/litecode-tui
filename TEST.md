# LitePilot-TUI Test Plan

## 0. Prerequisites

```bash
# Ensure Ollama is running with the test model
ollama pull qwen3.5:4b
ollama list  # verify qwen3.5:4b appears

# Build the project
cargo build --release
```

---

## Part A: Automated Tests

### A1. Unit Test Suite

```bash
cargo test
```

**Expected**: 160 tests pass in <1s.

| Module | Test Count | Coverage |
|--------|-----------|----------|
| agent/mod.rs | 5 | Plan parsing, file change extraction, audit pass/fail |
| agent/retry.rs | 11 | Response validation (empty, whitespace, missing markers, unclosed fences), correction prompts, truncation |
| agent/prompts.rs | 2 | Prompt non-empty, model-size selection |
| agent/planner.rs | 1 | Message construction |
| agent/syntax.rs | 3 | Language detection by extension and path, nonexistent file |
| agent/auto_run.rs | 1 | MAX_RETRIES constant |
| app.rs | 7 | Mode cycle, permissions per mode, mode parsing, pending actions, state switch |
| config.rs | 16 | Defaults, load/save roundtrip, TOML serialization, theme roundtrip, validation, proptest |
| ollama/mod.rs | 3 | Client construction, ping connection refused, list models connection refused |
| ollama/chat.rs | 3 | Message constructors, request serialization, model not found |
| ollama/model.rs | 7 | Size classification, context window, parameter estimation |
| sandbox/mod.rs | 6 | Path traversal, outside workspace, symlink, command allowlist/blocklist |
| sandbox/executor.rs | 3 | Echo execution, blocked command, CWD execution |
| search/mod.rs | 17 | Backend parse + fallback-chain ordering, Bing/Baidu/SearXNG parsers, query-language classification, URL + percent encoding, entity unescape, truncation, disabled, toggle |
| search/cache.rs | 4 | Set/get, miss, expiry, case-insensitive hash |
| project/mod.rs | 2 | File tree listing, gitignore exclusion |
| project/file_ops.rs | 5 | Plan mode blocks, edit mode allows, auto mode applies, outside workspace, delete |
| project/uv.rs | 2 | UV availability check, init with missing binary |
| codebase/mod.rs | 3 | Load by name, search by tags/description, empty query |
| codebase/builtin.rs | 3 | Template count, non-empty content, populate without overwrite |
| codebase/index.rs | 4 | Tag scanning (found/skipped/empty/nonexistent) |
| codebase/retrieval.rs | 9 | Catalog format, budget fitting, selection parsing, context truncation |
| session/mod.rs | 5 | UUID, messages, preview, meta conversion |
| session/persistence.rs | 3 | Save/load roundtrip, nonexistent, empty list |
| skills/mod.rs | 3 | Empty registry, get by name, trigger matching |
| skills/parser.rs | 4 | Valid, minimal, missing name, no frontmatter |
| skills/builtin.rs | 2 | Populate creates files, no overwrite |
| ui/mod.rs | 11 | Input push/take/backspace, output, scroll, sidebar navigation/tab switch, truncate, diff colors, markdown headers |
| ui/theme.rs | 9 | Default, hex, ANSI, invalid, reset, mode indicators |
| util/diff.rs | 4 | Additions, removals, identical, unified format |
| util/text.rs | 4 | Token estimation, truncation (text and lines) |
| wizard.rs | 8 | Defaults, presets, step number, slot order/labels/set, input editing, config application |

### A2. Property-Based Tests

```bash
cargo test config::tests::proptest_roundtrip
```

**Expected**: Proptest generates random valid configs, verifies TOML round-trip.
100 iterations by default.

### A3. Clippy + fmt

```bash
cargo clippy -- -D warnings
cargo fmt --check
```

**Expected**: Zero warnings, zero formatting issues.

### A4. Build Verification

```bash
cargo build --release
```

**Expected**: Clean release build with LTO + strip. Binary at `target/release/litepilot`.

### A5. Cross-Platform Build Check

```bash
# macOS Apple Silicon
rustup target add aarch64-apple-darwin
cargo build --release --target aarch64-apple-darwin

# macOS Intel
rustup target add x86_64-apple-darwin
cargo build --release --target x86_64-apple-darwin
```

**Expected**: Both targets compile without errors.

---

## Part B: Manual Tests — Core UI

### B1. First-Run Wizard

**Steps**:
1. Delete config: `rm -rf ~/.litepilot`
2. Run: `cargo run`
3. Wizard should appear

**Verify**:
- [ ] Wizard shows setup UI with Ollama URL field
- [ ] Ollama URL defaults to `http://127.0.0.1:11434`
- [ ] Press Enter → connecting screen → model list from Ollama
- [ ] Select models: qwen3.5:4b for Fast, Core, Audit (Tab to skip)
- [ ] Confirm page shows all selections
- [ ] On Enter, wizard saves config and enters main UI

### B2. Status Bar

**Verify**:
- [ ] Shows "LitePilot" logo (bold)
- [ ] Shows Ollama endpoint URL
- [ ] Shows F:qwen3.5:4b C:qwen3.5:4b A:qwen3.5:4b
- [ ] Shows [EDIT] mode badge (bold)
- [ ] Shows SEARCH:ON or SEARCH:OFF
- [ ] Shows working directory

### B3. Mode Switching

**Steps**: Press `Shift+Tab` three times.

**Verify**:
- [ ] EDIT → AUTO → PLAN → EDIT cycle works
- [ ] Each switch shows "[system] Switched to X mode" message

### B4. Sidebar

**Steps**:
- [ ] `Esc` toggles sidebar visibility
- [ ] Up/Down arrows navigate file tree
- [ ] Selected item highlighted with inverted colors
- [ ] `Tab` switches between "Project Files" and "Code Base" tabs
- [ ] Sidebar hides when terminal width < 60 cols (resize to test)

### B5. Scrolling

**Steps**: Send several messages to build scroll history, then:
- [ ] `PageUp` scrolls up, disables auto-scroll
- [ ] `PageDown` scrolls down
- [ ] New message re-enables auto-scroll (jumps to bottom)

### B6. Quit

**Verify**:
- [ ] In EDIT mode: `Ctrl+C` exits immediately
- [ ] In AUTO mode: first `Ctrl+C` shows confirmation, second within 2s quits
- [ ] `/quit` and `/exit` commands work
- [ ] Terminal is cleanly restored (no raw mode left on)

---

## Part C: Manual Tests — Streaming Chat (P6)

### C1. Token-by-Token Streaming

**Input**: `What is the Rust programming language?`

**Verify**:
- [ ] User message appears immediately in chat
- [ ] Status bar shows `thinking...`
- [ ] Response appears **token by token** (not all at once)
- [ ] Text grows incrementally in the Assistant output line
- [ ] Markdown formatting: headers bold with color, bullet lists rendered
- [ ] Auto-scroll follows streaming output

### C2. Streaming + Code Generation

**Input**: `Write a Python function that checks if a string is a palindrome`

**Verify**:
- [ ] Code block appears line by line during streaming
- [ ] Syntax highlighting applied as code block completes
- [ ] "Detected N file change(s)" message after stream completes
- [ ] "Type /apply to write these files" hint appears

### C3. Message Queuing During Streaming

**Steps**:
1. Send "Explain how HTTP works in detail"
2. While streaming is active, type and send a second message

**Verify**:
- [ ] Second message shows as `> message (queued)` in accent color
- [ ] Status bar shows `thinking... (1 queued)`
- [ ] First response streams to completion
- [ ] Second message auto-processes after first finishes
- [ ] Both responses appear in order

### C4. Streaming Error Handling

**Steps**: Start a request, then kill Ollama mid-stream (`ollama stop qwen3.5:4b`)

**Verify**:
- [ ] Partial response is preserved in chat
- [ ] Error message appears: "Stream error: ..."
- [ ] App remains responsive (doesn't hang)
- [ ] Can send another message after restarting Ollama

---

## Part D: Manual Tests — Three-Tier Agent Pipeline (P1)

These tests verify the Auto mode pipeline: plan (fast_model) → implement (core_model) → audit (audit_model) → auto-apply.

### D1. Auto Mode Code Request

**Setup**: Switch to Auto mode (Shift+Tab until [AUTO]).

**Input**: `Create a hello.py that prints hello world`

**Verify**:
- [ ] Response shows "Auto pipeline: N file(s) generated and applied."
- [ ] File appears in workspace **without needing `/apply`**
- [ ] Each applied file listed: `+ hello.py (create)`
- [ ] File content matches what the pipeline generated

### D2. Auto Mode Multi-File Generation

**Input**: `Create a simple Python calculator with add, subtract, multiply, divide functions in calc.py and a test file test_calc.py`

**Verify**:
- [ ] Multiple files generated and applied automatically
- [ ] Each file listed in the applied summary
- [ ] Both files exist in workspace with correct content

### D3. Edit Mode Uses Direct Chat (No Pipeline)

**Setup**: Switch to Edit mode (Shift+Tab until [EDIT]).

**Input**: `Create a goodbye.py that prints goodbye`

**Verify**:
- [ ] Response comes through streaming (direct chat, no pipeline orchestration)
- [ ] "Detected N file change(s)" message appears
- [ ] "Type /apply to write these files" hint shown
- [ ] File is **NOT** auto-applied — requires explicit `/apply`

### D4. Auto Mode Simple Question (No Pipeline)

**Setup**: In Auto mode.

**Input**: `What is 2 + 2?`

**Verify**:
- [ ] Simple questions in Auto mode use direct chat (not the pipeline)
- [ ] Response streams normally
- [ ] No "Auto pipeline" message (only code requests trigger pipeline)

---

## Part E: Manual Tests — Interactive Edit Confirmation (P5)

### E1. Edit Mode Confirmation Flow

**Setup**: Switch to Edit mode.

**Steps**:
1. Input: `Create a demo.py that prints "demo"`
2. Wait for response with file changes
3. Type `/apply`

**Verify**:
- [ ] Diff preview shown for each file (green + added lines)
- [ ] "Review N file(s). Press y/n/a (y=yes, n=no, a=apply all):" prompt appears
- [ ] Pressing `y` writes the file and shows "Wrote demo.py"
- [ ] Syntax check runs: "Syntax OK: demo.py"
- [ ] Confirmation state clears after all files reviewed

### E2. Skip a File with 'n'

**Setup**: Generate a response with 2+ file changes, then `/apply` in Edit mode.

**Steps**:
1. Press `n` for the first file

**Verify**:
- [ ] "Skipped <file>" message appears
- [ ] File is **NOT** written to disk
- [ ] Prompt continues for remaining files

### E3. Apply All with 'a'

**Setup**: Generate a response with 3+ file changes, then `/apply` in Edit mode.

**Steps**:
1. Press `a` to apply all remaining

**Verify**:
- [ ] All remaining files written without further prompts
- [ ] "Applied N/N remaining file(s)" summary shown
- [ ] Syntax check runs on each written file

### E4. Auto Mode Bypasses Confirmation

**Setup**: Switch to Auto mode. Generate a response with file changes.

**Steps**: Type `/apply`

**Verify**:
- [ ] All files applied immediately (no y/n/a prompt)
- [ ] Syntax check runs on each file
- [ ] "Applied N/N file(s)" summary shown

### E5. Plan Mode Blocks /apply

**Setup**: Switch to Plan mode. Generate a response with file changes.

**Steps**: Type `/apply`

**Verify**:
- [ ] Error: "Blocked <file>: Current mode does not allow file writes"
- [ ] No files written to disk

---

## Part F: Manual Tests — Syntax Checker (P3)

### F1. Syntax Check on Valid Python

**Steps**:
1. In Edit mode, input: `Create a file called valid.py with a hello world function`
2. `/apply` → confirm with `y`

**Verify**:
- [ ] "Wrote valid.py" message
- [ ] "Syntax OK: valid.py" message appears after write

### F2. Syntax Check on Invalid Code

**Steps**:
1. In Edit mode, input: `Create a file called bad.py with this exact content: def foo(\n    return 1`
2. `/apply` → confirm with `y`

**Verify**:
- [ ] "Wrote bad.py" message
- [ ] "Syntax error in bad.py:" with error details from `python3 -m py_compile`
- [ ] Error shows first 5 lines of compiler output

### F3. Syntax Check Skipped for Unsupported Languages

**Steps**:
1. Create a `.txt` or `.md` file via the LLM
2. `/apply` → confirm

**Verify**:
- [ ] "Syntax check skipped: Unsupported language" message

### F4. Syntax Check in Auto Pipeline

**Steps**:
1. Switch to Auto mode
2. Input: `Create a simple python script called auto_check.py that prints numbers 1 to 10`

**Verify**:
- [ ] File auto-applied by pipeline
- [ ] Syntax check result shown (OK or error)

---

## Part G: Manual Tests — Web Search Integration (P2)

### G1. Toggle Web Search

**Steps**: Press `Ctrl+S`

**Verify**:
- [ ] "Web search: ON" or "Web search: OFF" message appears
- [ ] Status bar toggles between SEARCH:ON and SEARCH:OFF

### G2. Search Results in Chat Context

**Setup**: Enable web search (`Ctrl+S` until SEARCH:ON).

**Input**: `What are the latest features in Python 3.13?`

**Verify**:
- [ ] "[search] Found N result(s)" message appears before LLM response
- [ ] Search results prepended to LLM context (LLM references web info)
- [ ] Response reflects up-to-date information from search

### G3. Search Disabled — No Search Messages

**Setup**: Disable web search (`Ctrl+S` until SEARCH:OFF).

**Input**: `What are the latest features in Python 3.13?`

**Verify**:
- [ ] No "[search]" messages appear
- [ ] Response is based purely on model training data

### G4. Search + Auto Pipeline

**Setup**: Enable search, switch to Auto mode.

**Input**: `Create a Python script that uses the latest asyncio features to fetch URLs concurrently`

**Verify**:
- [ ] "[search] Found N result(s)" appears before pipeline starts
- [ ] Auto pipeline uses search context in generation
- [ ] File auto-applied with search-informed code

### G5. Search Cache

**Steps**:
1. Enable search, ask "What is Rust?" → note search results
2. Ask "What is Rust?" again → check cache hit

**Verify**:
- [ ] Second search is faster (served from disk cache)
- [ ] Same results appear

---

## Part H: Manual Tests — FileOps Mode Enforcement (P4)

### H1. FileOps Enforces Plan Mode Block

**Steps**:
1. Switch to Plan mode
2. Ask for code: `Create a plan_test.py that prints hello`
3. Wait for response, type `/apply`

**Verify**:
- [ ] "Blocked plan_test.py: Current mode does not allow file writes"
- [ ] File NOT created on disk

### H2. FileOps Diff Preview in Edit Mode

**Steps**:
1. Create `modify_me.py` with content: `x = 1`
2. In Edit mode: `Change modify_me.py to set x = 42`
3. Wait for response, type `/apply`

**Verify**:
- [ ] Diff preview shows: `-x = 1` (red) and `+x = 42` (green)
- [ ] Confirmation prompt appears (y/n/a)
- [ ] Press `y` → file updated

### H3. FileOps Delete via /apply

**Steps**:
1. Create `to_delete.py` with any content
2. Ask: `Delete the file to_delete.py`
3. Wait for response, type `/apply`

**Verify**:
- [ ] "Deleted to_delete.py" or "Wrote to_delete.py" message
- [ ] File removed from workspace

---

## Part I: Manual Tests — UV Commands (P7)

### I1. /uv Help

**Input**: `/uv`

**Verify**:
- [ ] "Usage: /uv init | /uv venv | /uv add <package> | /uv run <script>"

### I2. /uv init

**Setup**: Run in an empty directory. UV must be installed.

**Input**: `/uv init`

**Verify**:
- [ ] "Running uv init..." message
- [ ] "Done." message on success
- [ ] `pyproject.toml` created in workspace

### I3. /uv venv

**Input**: `/uv venv`

**Verify**:
- [ ] "Running uv venv..." message
- [ ] `.venv/` directory created in workspace

### I4. /uv add

**Input**: `/uv add requests`

**Verify**:
- [ ] "Running uv add requests..." message
- [ ] `pyproject.toml` updated with requests dependency
- [ ] `uv.lock` created or updated

### I5. /uv run

**Setup**: Create a `hello.py` that prints "hello from uv".

**Input**: `/uv run hello.py`

**Verify**:
- [ ] "Running uv run hello.py..." message
- [ ] Script output shown
- [ ] "Done." message

### I6. /uv Not Installed

**Setup**: Temporarily rename/remove `uv` from PATH.

**Input**: `/uv init`

**Verify**:
- [ ] "uv is not installed. Install it: https://docs.astral.sh/uv/"

### I7. /uv in Plan Mode

**Setup**: Switch to Plan mode.

**Input**: `/uv add numpy`

**Verify**:
- [ ] "Command execution not allowed in Plan mode."

### I8. /uv Missing Package Argument

**Input**: `/uv add`

**Verify**:
- [ ] "Usage: /uv add <package>"

### I9. /uv Unknown Subcommand

**Input**: `/uv build`

**Verify**:
- [ ] "Unknown /uv subcommand: build. Use: init, venv, add, run"

---

## Part J: Manual Tests — Skills

### J1. /skills List

**Input**: `/skills`

**Verify**:
- [ ] Lists: /search, /review, /explain, /simplify, /test
- [ ] Each skill shows name and description

### J2. /explain Skill

**Input**: `/explain what does the fn main() function do in a Rust program`

**Verify**:
- [ ] Response is an explanation (not code)
- [ ] Thinking indicator + queued message behavior works for skills too

### J3. /search Skill

**Input**: `/search find all TODO comments in the project`

**Verify**:
- [ ] Response contains grep-like search results
- [ ] Returns relevant matches from workspace files

### J4. /review Skill

**Input**: `/review src/main.rs`

**Verify**:
- [ ] Response reviews the code for bugs, style, security

### J5. /simplify Skill

**Input**: `/simplify make the handle_input function shorter`

**Verify**:
- [ ] Response contains refactored code with FILE/ACTION markers
- [ ] Can `/apply` the changes

### J6. /test Skill

**Input**: `/test generate tests for src/util/text.rs`

**Verify**:
- [ ] Response contains test code
- [ ] FILE markers present for test file

---

## Part K: Manual Tests — Coding Agent Workflows (qwen3.5:4b)

These simulate real coding tasks a user would do with a coding agent like Claude Code.

### K1. New Project Scaffolding (Auto Mode)

**Setup**: Switch to Auto mode.

**Input**: `Create a new Flask REST API project with endpoints for a todo list (CRUD). Include a requirements.txt and a basic project structure.`

**Verify**:
- [ ] Auto pipeline runs: plan → implement → audit
- [ ] Multiple files generated and auto-applied
- [ ] "Auto pipeline: N file(s) generated and applied." summary
- [ ] Files are syntactically valid: `python3 -m py_compile app.py`

### K2. Bug Fix (Edit Mode with Confirmation)

**Setup**: Create `bug.py`:
```python
def divide(a, b):
    return a / b  # bug: no ZeroDivisionError handling
```

**Input**: `Fix the divide function in bug.py to handle division by zero`

**Steps**:
1. Wait for response
2. `/apply` → review diff
3. Press `y` to confirm

**Verify**:
- [ ] Diff shows red (-) for old line, green (+) for fixed line
- [ ] Syntax check: "Syntax OK: bug.py"
- [ ] Fixed code includes try/except or if b == 0 check

### K3. Add Feature to Existing Code (Auto Mode)

**Setup**: Create `shapes.py`:
```python
class Rectangle:
    def __init__(self, width, height):
        self.width = width
        self.height = height
    def area(self):
        return self.width * self.height
```

Switch to Auto mode.

**Input**: `Add a Circle class and a Triangle class to shapes.py, each with an area() method`

**Verify**:
- [ ] Auto pipeline modifies shapes.py
- [ ] Original Rectangle class preserved
- [ ] Syntax check passes

### K4. Multi-File Feature (Auto Mode)

**Input**: `Create a simple logging module in logger.py and a main.py that uses it to log "hello world"`

**Verify**:
- [ ] Multiple files generated and auto-applied
- [ ] logger.py created (new file)
- [ ] main.py created (new file)
- [ ] Syntax check on both files

### K5. Code Review (Plan Mode)

**Setup**: Switch to Plan mode.

**Input**: `/review src/sandbox/mod.rs`

**Verify**:
- [ ] Read-only analysis (no file changes proposed)
- [ ] Direct chat response with review findings
- [ ] `/apply` blocked if tried

### K6. Generate Tests (Edit Mode)

**Setup**: Switch to Edit mode.

**Input**: `/test generate tests for src/util/text.rs`

**Verify**:
- [ ] Test code generated with FILE markers
- [ ] `/apply` → confirmation prompt
- [ ] Confirm with `a` → test file written
- [ ] Syntax check on generated test file

### K7. Web-Search-Augmented Query

**Setup**: Enable web search (Ctrl+S until SEARCH:ON).

**Input**: `What is the current latest version of the Rust programming language and what are its key features?`

**Verify**:
- [ ] "[search] Found N result(s)" appears
- [ ] Response references current/up-to-date information
- [ ] Search results cached for subsequent similar queries

---

## Part L: Manual Tests — Edge Cases

### L1. Ollama Not Running

**Steps**: Stop Ollama, then `cargo run`

**Verify**:
- [ ] Wizard shows "Connection failed" error
- [ ] Can retry with correct URL
- [ ] After entering main UI, error message shows "Cannot connect to Ollama"
- [ ] Sending a message shows Ollama error (not crash)

### L2. Model Not Found

**Setup**: Configure a non-existent model name via `/setup`

**Verify**:
- [ ] Chat returns "Model 'xxx' not found in Ollama" error
- [ ] No crash

### L3. Very Long Response

**Input**: `Write a complete implementation of a binary search tree in Python with insert, delete, search, and traversal methods`

**Verify**:
- [ ] Response streams completely (no truncation in view)
- [ ] Scrolling works for long output
- [ ] Code block highlighting works for the full response

### L4. Empty Input

**Steps**: Press Enter with empty input

**Verify**:
- [ ] Nothing happens (no error, no empty message)

### L5. Unknown Skill

**Input**: `/foobar test`

**Verify**:
- [ ] Error: "Unknown skill: /foobar. Type /skills to see available skills."

### L6. /apply Without Changes

**Steps**: Send "hello", then type `/apply`

**Verify**:
- [ ] Error: "No file changes found in the last response."

### L7. /apply Without Assistant Response

**Steps**: Start app, immediately type `/apply`

**Verify**:
- [ ] Error: "No assistant response to apply."

### L8. Terminal Resize

**Steps**: Resize terminal window while running

**Verify**:
- [ ] Layout adapts (sidebar hides if < 60 cols)
- [ ] No crash or garbled display
- [ ] Status bar wraps/truncates properly

### L9. Plan Mode Blocks /apply via FileOps

**Steps**:
1. Switch to Plan mode (Shift+Tab until PLAN)
2. Ask for code generation
3. Type `/apply`

**Verify**:
- [ ] "Blocked <file>: Current mode does not allow file writes"
- [ ] No files written to disk

### L10. Confirmation Keys During Normal Input

**Steps**:
1. Generate file changes and `/apply` in Edit mode (confirmation prompt active)
2. Try typing regular text instead of y/n/a

**Verify**:
- [ ] Regular keypresses (letters other than y/n/a) pass through to input field
- [ ] Only y/n/a trigger confirmation actions
- [ ] Confirmation state eventually clears

### L11. Streaming Interrupted by New Request

**Steps**:
1. Send a long request
2. While streaming, send another message (it queues)
3. Wait for first to complete

**Verify**:
- [ ] First response streams to completion
- [ ] Queued message processes after
- [ ] No crash or garbled output

---

## Part M: Performance Benchmarks

### M1. Cold Start Time

```bash
time cargo run --release -- -d /tmp
# Accept wizard defaults quickly
```

**Target**: < 2s from launch to first prompt.

### M2. Streaming First-Token Latency

**Input**: `Say "hello"`
**Measure**: Time from Enter to first token appearing.
**Target**: < 5s with qwen3.5:4b on local Ollama.

### M3. Memory Usage

```bash
# Run in one terminal
cargo run --release
# In another terminal, check memory
ps aux | grep litepilot
```

**Target**: < 50MB RSS during idle. < 100MB during LLM processing.

### M4. UI Responsiveness During Streaming

**Steps**:
1. Send a complex request
2. While streaming, toggle sidebar, switch modes, type input

**Verify**:
- [ ] UI remains responsive (no lag during streaming)
- [ ] Sidebar toggle works during streaming
- [ ] Mode switching works during streaming
- [ ] Input typing is smooth during streaming

---

## Part N: NPM Wrapper (P8)

### N1. Package Structure

**Verify**:
- [ ] `package.json` exists with correct name, version, bin mapping
- [ ] `bin/litepilot.js` exists and is executable (`chmod +x`)
- [ ] `.github/workflows/release.yml` exists

### N2. JS Shim Platform Detection

```bash
# Test locally (requires at least one platform binary)
cargo build --release
mkdir -p bin
cp target/release/litepilot bin/litepilot-$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m | sed 's/x86_64/x64/;s/arm64/arm64/')
node bin/litepilot.js
```

**Verify**:
- [ ] Platform detected correctly (darwin-arm64, darwin-x64, linux-x64, linux-arm64)
- [ ] Error shown on unsupported platform
- [ ] Binary executes with forwarded arguments

### N3. npm link Test

```bash
npm link
litepilot --help
npm unlink -g litepilot-tui
```

**Verify**:
- [ ] `litepilot` command available globally after link
- [ ] CLI args forwarded correctly
- [ ] Unlink removes global command

### N4. Release Workflow Validation

```bash
# Dry-run the workflow syntax
gh workflow validate release.yml
```

**Verify**:
- [ ] Workflow YAML is valid
- [ ] Matrix builds 4 platforms: darwin-arm64, darwin-x64, linux-x64, linux-arm64
- [ ] Publish job depends on all builds
- [ ] NPM_TOKEN secret required

---

## Test Results Template

| Test ID | Description | Status | Notes |
|---------|-------------|--------|-------|
| A1 | Unit tests (160) | PASS | <1s |
| A2 | Proptest | PASS | 100 iterations |
| A3 | Clippy + fmt | PASS | Zero warnings |
| A4 | Release build | PASS | LTO + strip |
| A5 | Cross-platform build | | |
| B1 | First-run wizard | | |
| B2 | Status bar | | |
| B3 | Mode switching | | |
| B4 | Sidebar | | |
| B5 | Scrolling | | |
| B6 | Quit | | |
| C1 | Token streaming | | |
| C2 | Streaming + code gen | | |
| C3 | Streaming + queuing | | |
| C4 | Streaming error handling | | |
| D1 | Auto mode code request | | |
| D2 | Auto mode multi-file | | |
| D3 | Edit mode direct chat | | |
| D4 | Auto mode simple question | | |
| E1 | Edit mode y/n/a flow | | |
| E2 | Skip file with n | | |
| E3 | Apply all with a | | |
| E4 | Auto mode bypasses confirm | | |
| E5 | Plan mode blocks apply | | |
| F1 | Syntax check valid | | |
| F2 | Syntax check invalid | | |
| F3 | Syntax check skipped | | |
| F4 | Syntax check in auto pipeline | | |
| G1 | Toggle web search | | |
| G2 | Search results in context | | |
| G3 | Search disabled | | |
| G4 | Search + auto pipeline | | |
| G5 | Search cache | | |
| H1 | FileOps plan mode block | | |
| H2 | FileOps diff preview | | |
| H3 | FileOps delete | | |
| I1 | /uv help | | |
| I2 | /uv init | | |
| I3 | /uv venv | | |
| I4 | /uv add | | |
| I5 | /uv run | | |
| I6 | /uv not installed | | |
| I7 | /uv plan mode block | | |
| I8 | /uv missing args | | |
| I9 | /uv unknown subcmd | | |
| J1 | /skills list | | |
| J2 | /explain skill | | |
| J3 | /search skill | | |
| J4 | /review skill | | |
| J5 | /simplify skill | | |
| J6 | /test skill | | |
| K1 | Project scaffolding (auto) | | |
| K2 | Bug fix (edit confirm) | | |
| K3 | Add feature (auto) | | |
| K4 | Multi-file feature (auto) | | |
| K5 | Code review (plan) | | |
| K6 | Generate tests (edit) | | |
| K7 | Web-search-augmented query | | |
| L1 | Ollama not running | | |
| L2 | Model not found | | |
| L3 | Long response | | |
| L4 | Empty input | | |
| L5 | Unknown skill | | |
| L6 | /apply no changes | | |
| L7 | /apply no response | | |
| L8 | Terminal resize | | |
| L9 | Plan mode blocks apply | | |
| L10 | Confirm keys vs input | | |
| L11 | Streaming interrupted | | |
| M1 | Cold start time | | |
| M2 | First-token latency | | |
| M3 | Memory usage | | |
| M4 | UI during streaming | | |
| N1 | Package structure | | |
| N2 | JS shim detection | | |
| N3 | npm link test | | |
| N4 | Release workflow | | |
