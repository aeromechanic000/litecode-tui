# LitePilot Development Guide

## Building from Source

```bash
# Clone
git clone https://github.com/aeromechanic000/litepilot-tui.git
cd litepilot-tui

# Debug build
cargo build

# Release build (optimized, stripped)
cargo build --release

# Run tests
cargo test

# Run with clippy
cargo clippy -- -D warnings

# Check formatting
cargo fmt --check
```

## Project Structure

```
litepilot-tui/
├── src/                  Rust source code
├── target/               Build output (gitignored)
├── Cargo.toml            Rust package manifest
├── CLAUDE.md             AI assistant instructions
├── PLAN.md               Implementation progress tracker
├── TEST.md               Test plan (automated + manual)
├── README.md             User-facing documentation
├── DEVELOP.md            This file
├── package.json          npm package manifest (for npm publishing)
└── bin/                  Platform-specific binaries + JS shim (for npm publishing)
```

## Running Tests

```bash
# All unit tests (160 tests)
cargo test

# Property-based tests
cargo test config::tests::proptest_roundtrip

# Lint
cargo clippy -- -D warnings
cargo fmt --check

# Integration tests (requires Ollama running)
cargo test -- --ignored
```

## Packaging and Publishing to npm

LitePilot can be installed via npm by bundling pre-compiled platform binaries behind a Node.js shim. The shim detects the user's OS and architecture, then executes the matching binary.

### Prerequisites

- Node.js and npm (`npm login` to authenticate)
- Rust toolchain with cross-compilation targets installed
- GitHub Actions CI for automated builds (optional but recommended)

### Step 1: Create the npm package structure

```
litepilot-tui/
├── package.json
└── bin/
    ├── litepilot.js              # Node.js entry point (shim)
    ├── litepilot-darwin-arm64    # macOS Apple Silicon binary
    ├── litepilot-darwin-x64      # macOS Intel binary
    ├── litepilot-linux-x64       # Linux x86_64 binary
    └── litepilot-linux-arm64     # Linux ARM64 binary
```

### Step 2: Create `package.json`

```json
{
  "name": "litepilot-tui",
  "version": "0.1.0",
  "description": "Terminal AI coding assistant powered by local Ollama models",
  "bin": {
    "litepilot": "./bin/litepilot.js"
  },
  "files": [
    "bin/",
    "README.md"
  ],
  "os": ["darwin", "linux"],
  "cpu": ["x64", "arm64"],
  "keywords": ["tui", "ai", "coding", "assistant", "ollama", "local", "terminal"],
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "https://github.com/aeromechanic000/litepilot-tui"
  }
}
```

Key fields:
- **`bin`**: Maps the `litepilot` command to the JS shim
- **`files`**: Only includes `bin/` and `README.md` in the published package (keeps it small)
- **`os` / `cpu`**: npm shows a clear error if installed on unsupported platforms

### Step 3: Create the JS shim (`bin/litepilot.js`)

```javascript
#!/usr/bin/env node
const { execFileSync } = require('child_process');
const path = require('path');

const platform = process.platform;
const arch = process.arch;

const platformMap = {
  'darwin-arm64': 'litepilot-darwin-arm64',
  'darwin-x64': 'litepilot-darwin-x64',
  'linux-x64': 'litepilot-linux-x64',
  'linux-arm64': 'litepilot-linux-arm64',
};

const binaryName = platformMap[`${platform}-${arch}`];
if (!binaryName) {
  console.error(`Unsupported platform: ${platform}-${arch}`);
  console.error('Supported: darwin-arm64, darwin-x64, linux-x64, linux-arm64');
  process.exit(1);
}

const binaryPath = path.join(__dirname, binaryName);
const args = process.argv.slice(2);

try {
  execFileSync(binaryPath, args, { stdio: 'inherit' });
} catch (e) {
  process.exit(e.status || 1);
}
```

### Step 4: Build platform binaries

Install cross-compilation targets:

```bash
rustup target add aarch64-apple-darwin
rustup target add x86_64-apple-darwin
rustup target add x86_64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
```

Build for each target:

```bash
# macOS Apple Silicon
cargo build --release --target aarch64-apple-darwin

# macOS Intel
cargo build --release --target x86_64-apple-darwin

# Linux x86_64
cargo build --release --target x86_64-unknown-linux-gnu

# Linux ARM64
cargo build --release --target aarch64-unknown-linux-gnu
```

> **Note**: Linux cross-compilation from macOS requires a linker. For `x86_64-unknown-linux-gnu`, install `brew install filosottile/musl-cross/musl-cross` and set `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-musl-gcc`. Alternatively, build in CI on Linux runners.

Copy binaries into `bin/`:

```bash
mkdir -p bin
cp target/aarch64-apple-darwin/release/litepilot bin/litepilot-darwin-arm64
cp target/x86_64-apple-darwin/release/litepilot bin/litepilot-darwin-x64
cp target/x86_64-unknown-linux-gnu/release/litepilot bin/litepilot-linux-x64
cp target/aarch64-unknown-linux-gnu/release/litepilot bin/litepilot-linux-arm64
chmod +x bin/litepilot-*
```

### Step 5: Test the package locally

```bash
# Link globally to test the CLI
npm link

# Should launch LitePilot
litepilot

# Unlink when done
npm unlink -g litepilot-tui
```

### Step 6: Publish to npm

```bash
# Dry run to see what will be published
npm publish --dry-run

# Publish
npm publish
```

For scoped packages or first-time publishes:

```bash
# Public scoped package
npm publish --access public

# First time (if package name is taken, use a scoped name)
npm init --scope=@yourusername
npm publish --access public
```

### Step 7: Users install and run

```bash
# Install globally
npm install -g litepilot-tui
litepilot

# Run without installing
npx litepilot-tui

# Run in a specific directory
npx litepilot-tui -- -d /path/to/project
```

## Automating with GitHub Actions CI

Use CI to build binaries on native runners for each platform, then publish automatically.

### Release workflow (`.github/workflows/release.yml`)

```yaml
name: Release

on:
  push:
    tags: ['v*']

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: macos-latest
            target: aarch64-apple-darwin
            binary: litepilot-darwin-arm64
          - os: macos-13
            target: x86_64-apple-darwin
            binary: litepilot-darwin-x64
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            binary: litepilot-linux-x64
          - os: ubuntu-latest
            target: aarch64-unknown-linux-gnu
            binary: litepilot-linux-arm64
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - run: cargo build --release --target ${{ matrix.target }}
      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.binary }}
          path: target/${{ matrix.target }}/release/litepilot

  publish:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with:
          path: bin
      - run: |
          chmod +x bin/litepilot-*
          mv bin/litepilot-darwin-arm64/litepilot bin/litepilot-darwin-arm64
          mv bin/litepilot-darwin-x64/litepilot bin/litepilot-darwin-x64
          mv bin/litepilot-linux-x64/litepilot bin/litepilot-linux-x64
          mv bin/litepilot-linux-arm64/litepilot bin/litepilot-linux-arm64
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          registry-url: 'https://registry.npmjs.org'
      - run: npm publish
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

Setup:
1. Create an npm access token at https://www.npmjs.com/settings/tokens
2. Add it as `NPM_TOKEN` in GitHub repo Settings → Secrets and variables → Actions
3. Tag a release: `git tag v0.1.0 && git push --tags`
4. CI builds all platforms and publishes to npm

## Version Updates

When releasing a new version:

```bash
# Update version in both files
# Cargo.toml: version = "0.2.0"
# package.json: "version": "0.2.0"

# Commit and tag
git commit -am "v0.2.0"
git tag v0.2.0
git push && git push --tags
```

## Test Cases for Usage

These test cases evaluate LitePilot's capabilities as a coding agent, benchmarked against what users expect from Claude Code, OpenAI Codex, DeepSeek-TUI, and Open Code. Organized by difficulty level.

### Level 1: Basic Capability

These verify the agent can handle fundamental coding tasks. Any mainstream agent should pass these.

| ID | Task | Prompt | Pass Criteria |
|----|------|--------|---------------|
| L1.1 | Simple file creation | `Create hello.py that prints hello world` | File created with correct content, syntax valid |
| L1.2 | Single function | `Write a Python function is_even(n) that returns True if n is even` | Correct logic, runnable code |
| L1.3 | Bug fix — typo | `Fix the typo in this file: prnt("hello") should be print("hello")` | Correct fix applied via modify action |
| L1.4 | Simple explanation | `Explain what this code does: def foo(x): return x * 2` | Accurate natural language explanation |
| L1.5 | Basic HTML | `Create an index.html with a heading "Hello" and a paragraph` | Valid HTML5 structure |
| L1.6 | Copy file pattern | `Create a file config.json with this content: {"port": 8080}` | Exact content match |
| L1.7 | Simple refactor | `Rename the variable x to count in counter.py` | All instances renamed, no other changes |
| L1.8 | Add a comment | `Add docstrings to all functions in utils.py` | Docstrings added to every function |
| L1.9 | List comprehension | `Rewrite this loop as a list comprehension in convert.py` | Functionally equivalent, more Pythonic |
| L1.10 | Simple test | `Write a unit test for the add function in calc.py` | Test file with proper assertions |

### Level 2: Intermediate Capability

Tasks requiring multi-step reasoning, understanding existing code, or generating multiple files. Most agents handle these well.

| ID | Task | Prompt | Pass Criteria |
|----|------|--------|---------------|
| L2.1 | CRUD API | `Create a Flask REST API for a todo list with GET, POST, PUT, DELETE endpoints` | 4+ endpoints, valid Python, syntactically correct |
| L2.2 | Bug fix — logic error | `Fix the off-by-one error in this loop: for i in range(0, len(arr))` | Correct boundary handling |
| L2.3 | Add feature | `Add a Circle class with area() method to shapes.py (which already has Rectangle)` | New class added, existing code preserved |
| L2.4 | Error handling | `Add try/except error handling to the fetch_url function in client.py` | Network errors, timeouts, HTTP errors handled |
| L2.5 | Multi-file project | `Create a Python package with __init__.py, models.py, and views.py for a blog` | 3+ files, proper imports between them |
| L2.6 | Code review | `/review src/sandbox/mod.rs` | Identifies real issues: edge cases, security, style |
| L2.7 | Generate tests | `/test src/util/text.rs` | Tests cover edge cases: empty strings, unicode, boundary values |
| L2.8 | Refactor to separate module | `Extract the validation logic from main.py into validators.py` | New file created, main.py updated with imports |
| L2.9 | Configuration file | `Create a YAML config file for a web server with host, port, and logging settings` | Valid YAML, sensible structure |
| L2.10 | Regex pattern | `Write a regex to validate email addresses and create a validator.py` | Reasonable regex coverage, proper function wrapper |
| L2.11 | Database model | `Create SQLAlchemy models for User, Post, and Comment with relationships` | Foreign keys, relationships correct |
| L2.12 | CLI tool | `Create a Python CLI tool with argparse that converts CSV to JSON` | Proper argparse, handles file I/O, error messages |

### Level 3: Advanced Capability

Tasks requiring deep code understanding, cross-file reasoning, or architectural decisions. Competitive benchmark level — separates good agents from mediocre ones.

| ID | Task | Prompt | Pass Criteria |
|----|------|--------|---------------|
| L3.1 | Design a REST API from spec | `Design and implement a REST API for an e-commerce cart with product catalog, user auth stubs, and order management. Include proper HTTP status codes and error responses.` | 10+ endpoints, correct status codes, error handling |
| L3.2 | Refactor large function | `Refactor the run_app function in src/main.rs to separate event handling, key routing, and result rendering into distinct functions` | Clean separation, no functionality lost |
| L3.3 | Add caching layer | `Add an in-memory LRU cache to the search module in src/search/mod.rs with a max size of 100 entries` | Cache integrated correctly, eviction logic works |
| L3.4 | Implement design pattern | `Refactor the file operations code to use the Strategy pattern — different strategies for Plan, Edit, and Auto modes` | Proper strategy interface, mode-specific behavior |
| L3.5 | Cross-file feature | `Add structured logging to this project. Create a logger module, integrate it into the agent pipeline, and write log entries to ~/.litepilot/logs/` | 2+ files changed, log rotation, proper levels |
| L3.6 | Performance optimization | `Optimize the parse_file_changes function in src/agent/mod.rs to avoid string cloning where possible` | Reduces allocations, benchmarks show improvement |
| L3.7 | Security hardening | `Review and fix any security issues in src/sandbox/mod.rs — check for path traversal, symlink attacks, and race conditions` | Identifies real vulnerabilities, proposes fixes |
| L3.8 | Full test suite | `Write a comprehensive test suite for src/project/file_ops.rs covering all modes, edge cases, and concurrent access` | 10+ tests, covers Plan/Edit/Auto, error paths |
| L3.9 | Async migration | `Convert the search module from sync reqwest calls to proper async with connection pooling` | Async throughout, connection reuse, no blocking |
| L3.10 | Multi-language project | `Create a project with a Python backend (FastAPI), TypeScript frontend (React component), and a shared JSON schema for the API contract` | 4+ files, 2 languages, consistent API types |

### Level 4: Expert Capability

Tasks that push the boundaries of local models. These are challenging even for cloud agents. Success here demonstrates genuine agent capability.

| ID | Task | Prompt | Pass Criteria |
|----|------|--------|---------------|
| L4.1 | Architecture migration | `Convert the event loop in main.rs from a polling model to an event-driven model using tokio channels. Maintain all existing functionality.` | No polling loop, async channels, all features work |
| L4.2 | Implement a mini-framework | `Create a plugin system for LitePilot where plugins can register new slash commands, hooks for pre/post file write, and custom renderers for output types` | Trait-based plugin API, dynamic loading, 2+ example plugins |
| L4.3 | Cross-repo understanding | `Analyze this project and create a CONTRIBUTING.md that accurately describes the architecture, module dependencies, testing strategy, and contribution workflow` | Accurate architecture diagram, correct module descriptions |
| L4.4 | Formal spec to code | `Implement a simplified JSON Patch (RFC 6902) library in Rust supporting add, remove, replace, move, copy, and test operations. Include proper error types.` | All 6 operations, proper Error enum, comprehensive tests |
| L4.5 | Debug subtle concurrency bug | `There's a race condition in the auto pipeline where syntax check runs before the file is fully written. Find it and fix it.` | Identifies the TOCTOU issue, proposes atomic write + check |
| L4.6 | DSL implementation | `Create a small domain-specific language for defining API routes in YAML, with a Python code generator that produces FastAPI boilerplate` | YAML parser, code generator, handles nested routes, type safety |
| L4.7 | Dependency upgrade migration | `Upgrade this project from reqwest 0.11 to 0.12, handling all breaking changes in the streaming API, timeout configuration, and error types` | All breaking changes handled, tests pass, no deprecated APIs |

### Level 5: Frontier Challenges

Tasks at the edge of current agent capability — difficult for Claude Code, potentially impossible for smaller models. These define the cutting edge.

| ID | Task | Prompt | Pass Criteria |
|----|------|--------|---------------|
| L5.1 | Distributed system | `Implement a Raft consensus library in Rust with leader election, log replication, and commit index tracking. Include a test harness that simulates network partitions.` | Correct consensus under partition, log consistency proof via tests |
| L5.2 | Compiler pass | `Write a type checker for a small ML-like language with algebraic data types, pattern matching, and Hindley-Milner type inference` | Type-safe programs accepted, type errors reported with locations |
| L5.3 | Full-stack app with auth | `Create a full-stack todo app: React frontend with state management, FastAPI backend with JWT auth, PostgreSQL schema, and Docker Compose setup` | 8+ files, 3 languages, working auth flow, deployable |
| L5.4 | Binary protocol parser | `Implement a MQTT v5 protocol parser in Rust using nom for parsing. Support CONNECT, PUBLISH, SUBSCRIBE, and DISCONNECT packet types with proper error recovery.` | Zero-copy parsing, handles malformed packets, fuzz-test-ready |
| L5.5 | Performance-critical optimization | `Rewrite the diff generation in src/util/diff.rs to use Myers' diff algorithm with O(ND) time complexity instead of the current similar crate. Benchmark against the original.` | Correct diffs, measurable speedup on large files, benchmarks included |

### Scoring Guide

| Level | Expected Pass Rate (Claude Code) | Expected Pass Rate (qwen3:8b local) | Significance |
|-------|----------------------------------|--------------------------------------|-------------|
| Level 1 | 100% | 90-100% | Baseline — must work |
| Level 2 | 95-100% | 70-90% | Practical daily use |
| Level 3 | 80-95% | 30-60% | Professional capability |
| Level 4 | 50-80% | 10-30% | Expert-level reasoning |
| Level 5 | 20-50% | 0-10% | Research frontier |

### Comparison with Mainstream Agents

| Capability | Claude Code | OpenAI Codex | DeepSeek | LitePilot (qwen3:8b) |
|-----------|-------------|-------------|----------|------------------------|
| Simple file creation | Excellent | Excellent | Excellent | Good |
| Multi-file generation | Excellent | Good | Excellent | Moderate |
| Bug fix (logic errors) | Excellent | Good | Good | Good |
| Architecture reasoning | Excellent | Moderate | Good | Limited |
| Large refactoring | Excellent | Moderate | Good | Limited |
| Cross-file dependencies | Excellent | Good | Good | Limited |
| Security review | Excellent | Good | Good | Moderate |
| Performance optimization | Excellent | Moderate | Good | Limited |
| Streaming output | Excellent | Good | Excellent | Good |
| Local/offline operation | No | No | Partial | Excellent |
| Privacy (no data leaves machine) | No | No | No | Excellent |
| Latency (first token) | 1-3s | 1-3s | 1-3s | 2-8s |
| Cost per query | $0.01-0.10 | $0.01-0.05 | $0.001-0.01 | Free |

