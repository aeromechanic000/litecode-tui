# Dual Resident Model Deployment on Single Ollama Host for Coding Agent
**Conclusion upfront**: Ollama natively supports loading & keeping **two models permanently resident in VRAM/RAM** on one single host (same or two distinct small coding models), fully compatible with your pre-defined `num_ctx=4096/8192`, Modelfile prompt solidification & session reuse design for dual-role Coding Agent pipeline.

## 1. Core Ollama Config Mechanism for Dual Permanent Resident Models
### 1.1 Global Server Environment Variables (Start before `ollama serve`)
Set these env vars to lock max loaded model count = 2 to avoid automatic LRU model eviction by Ollama scheduler:
```bash
# Linux/macOS launch config
export OLLAMA_MAX_LOADED_MODELS=2    # Hard limit: max 2 models loaded concurrently
export OLLAMA_NUM_PARALLEL=3         # Parallel inference slots per resident model
ollama serve
```
- `OLLAMA_MAX_LOADED_MODELS=2`: Critical — blocks Ollama from evicting either model when receiving cross-model requests; default value = GPU count×3 / CPU=3, overwrite to fixed 2 for dual-agent fixed setup.
- If you need persistent system-level config (Ubuntu systemd): edit ollama service env permanently.

### 1.2 Permanent Resident via `keep_alive=-1` (Ollama official fixed resident syntax)
After Ollama server boots, warm-up & pin both models into memory permanently with one-time API call (`keep_alive=-1 = infinite resident, never auto-unload on idle`):
```bash
# Warm Model A (Planning & Architecture Agent, independent Modelfile fixed prompt + num_ctx)
curl http://127.0.0.1:11434/api/generate -d '{
  "model": "code-planner:q5_K_M",
  "prompt": "warmup init",
  "keep_alive": -1,
  "stream": false
}'

# Warm Model B (Code Implementation + Review Agent, separate Modelfile rules)
curl http://127.0.0.1:11434/api/generate -d '{
  "model": "code-writer:q5_K_M",
  "prompt": "warmup init",
  "keep_alive": -1,
  "stream": false
}'
```
> Same-model scenario: You can pin two copies of identical model tag (e.g., `deepseek-coder:6.7b-q5`) with separate Modelfile builds (different SYSTEM prompt & fixed `num_ctx` config), still counted as two independent resident runners by Ollama scheduler.

### 1.3 Unload manually when needed
```bash
# Unload single resident model instantly
curl http://127.0.0.1:11434/api/generate -d '{"model":"code-planner:q5_K_M","keep_alive":0}'
```

## 2. Two Typical Dual-Model Split Design for Your Coding Agent Pipeline
Align with your prior layered agent architecture: **Planner Agent ↔ Coder Agent** split, each model pre-baked with independent Modelfile, fixed `num_ctx` & sampling params, two models always resident for low-latency cross-call.
| Model Role | Fixed num_ctx | Sampling Params | Modelfile Core Solidified Prompt |
|------------|---------------|-----------------|----------------------------------|
| Model1: Requirement & Architecture Planner | 8192 | temp=0.2, top_p=0.5, top_k=20, repeat_penalty=1.1 | SYSTEM fixed: decompose coding requirement, output modular architecture, split task into sub-functions, no raw code output |
| Model2: Code Write + Bug Review | 4096 | temp=0.1, top_p=0.5, top_k=20, repeat_penalty=1.1 | SYSTEM fixed: implement code strictly per planner’s design, format output with markdown code block, auto spot syntax error |

### Modelfile sample for separated dual models
```dockerfile
# Modelfile for code-planner (Model A)
FROM deepseek-coder:6.7b-q5_K_M
PARAMETER num_ctx 8192
PARAMETER temperature 0.2
PARAMETER top_p 0.5
SYSTEM """
You are coding architecture planner. Split business requirement into module structure, define function input/output specs only, never generate full source code.
"""

# Modelfile for code-writer (Model B)
FROM deepseek-coder:6.7b-q5_K_M
PARAMETER num_ctx 4096
PARAMETER temperature 0.1
PARAMETER top_p 0.5
SYSTEM """
Implement full executable code following given architecture spec, output fixed format: [Code] + ```lang\ncode\n``` + [Note], add necessary inline comment only.
"""
```
Build two independent custom tags after writing Modelfile:
```bash
ollama build -t code-planner:q5_K_M ./Modelfile-planner
ollama build -t code-writer:q5_K_M ./Modelfile-writer
```

## 3. Agent Runtime Workflow with Dual Resident Models (Combine your session/KV cache reuse logic)
Full coding chain: `User Req → Resident Planner(ModelA, single persistent session) → Arch Spec Output → Resident Coder(ModelB, independent persistent session) → Source Code → Debug append log back to Coder’s session`
1. **Isolated independent session per model**: Each model maintains its own exclusive Ollama chat session ID; Planner’s session reuse KV cache for multi-turn requirement refinement, Coder’s separate session reuse KV for iterative coding & bug fix (follow your original single-session-no-reset rule).
2. Context truncation rules inherit your existing logic: truncate obsolete history when context approaches `num_ctx` ceiling for each model individually.
3. Incremental code continuation: call ModelB’s native Ollama continue API for unfinished long code generation, reuse existing KV cache of Coder’s resident session.

## 4. Hardware VRAM Pre-Calculation Rule for Dual Resident Setup
- Q4_K_M / Q5_K_M 7B~8B small coding model: ~5~7GB VRAM per loaded instance + ~15% extra overhead for runtime + KV Cache peak.
- Example: Two × 7B Q5_K_M = ~13~15GB total reserved VRAM; 16GB+ VRAM GPU fully supports stable dual permanent resident.
- If insufficient VRAM: downgrade quantization to Q4_K_M or switch smaller 3B coding variants.

## 5. Two Optional Advanced Deployment Modes
### Option A: Single-Ollama-Instance Dual Model (Recommended for your Coding Agent, above main scheme)
Single port `11434`, two pinned resident models via `keep_alive=-1` + `OLLAMA_MAX_LOADED_MODELS=2`; Agent backend dispatch API request to target `model=code-planner` or `model=code-writer` dynamically. Lowest deployment cost, native KV cache per separate session.

### Option B: Dual Independent Ollama Instance (Complete resource isolation)
Spin two separated Ollama services on different ports, each binds dedicated GPU core via `CUDA_VISIBLE_DEVICES`:
```bash
# Instance1: Planner only port 11434, GPU0
CUDA_VISIBLE_DEVICES=0 OLLAMA_KEEP_ALIVE=-1 ollama serve

# Instance2: Coder only port 11435, GPU1
CUDA_VISIBLE_DEVICES=1 OLLAMA_KEEP_ALIVE=-1 OLLAMA_HOST=127.0.0.1:11435 ollama serve
```
Each instance holds exactly one permanently resident model, zero resource contention; preferred for multi-GPU production environment.

## 6. Key Optimization Alignment with your prior specification
1. Prompt solidification: All fixed role/code rules pre-compiled into separate Modelfile for two models, eliminate runtime dynamic prompt concat.
2. num_ctx fixed split: Planner=8192 / Coder=4096 strictly as your defined threshold to avoid oversized KV cache inflation.
3. Session reuse: Each model’s long-running coding iteration uses its own persistent chat session to maximize incremental KV Cache hit ratio (>30% speed gain vs frequent new-session creation).
4. Sampling locked per role: predefine temp/top_p/penalty inside Modelfile to avoid runtime parameter misconfiguration.

## 7. Common Pitfall Avoidance
1. Never set `OLLAMA_MAX_LOADED_MODELS=1` when running dual resident, otherwise Ollama evicts one model automatically on cross-model request.
2. `keep_alive=-1` must be triggered by at least one valid warm-up API call per model after server restart; otherwise model gets unloaded after default 5min idle timeout.
3. No cross-model context sharing between Planner & Coder; pass only final architecture text from ModelA output as fresh user prompt into ModelB’s independent session to prevent context overflow of either window limit.