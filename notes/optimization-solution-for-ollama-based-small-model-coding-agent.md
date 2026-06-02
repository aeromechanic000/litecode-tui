# Optimization Solution for Ollama-based Small Model Coding Agent
Focus: **Prompt Template Solidification, Context Window Limitation, Session Reuse**, combined with inference parameter tuning, context management and incremental generation to boost inference efficiency and output quality.

## 1. Core Basic Configuration: Context Window & Precompiled Prompt Template
### 1.1 num_ctx Context Window Rules
Restrict context window size based on small model capabilities and KV Cache load to avoid slow inference and memory overflow.

| Scenario | Recommended num_ctx | Description |
| ---- | ---- | ---- |
| Simple scripts, single functions & short code snippets | 4096 | Optimal for general coding, low KV Cache consumption and fast inference |
| Multi-file projects, long code & complete modules/classes | 8192 | Do not exceed 8192. Oversized windows will drastically increase latency and degrade accuracy |

**Core Principle**: Never expand the context window arbitrarily. A larger window leads to higher KV Cache overhead and slower inference.

### 1.2 Prompt Template Solidification via Modelfile
Embed fixed role definitions, coding rules and output formats into Modelfile in advance, instead of concatenating prompts dynamically in each request. This reduces transmission and parsing overhead, and standardizes outputs.

#### 1.2.1 Modelfile for General Coding Agent
```dockerfile
FROM [your code-focused small model:quantization version]
# Set global fixed context window
PARAMETER num_ctx 4096

SYSTEM """
You are a professional coding assistant. Follow the rules strictly:
1. Write standardized code with concise necessary comments; avoid redundant content.
2. Implement functions step by step as required. Do not fabricate logic.
3. Follow the fixed output format: explain ideas first, then present code in code blocks.
4. Ensure syntax correctness and runnable code.
"""
```

#### 1.2.2 Multi-role Split Templates (Advanced)
Divide the full coding workflow into three independent agents with dedicated Modelfiles for better stability:
1. **Requirement & Architecture Agent**: Responsible for requirement analysis, task breakdown and architecture design.
2. **Coding Agent**: Focuses on code implementation and compliance with coding specifications.
3. **Code Review Agent**: Performs syntax checking, error detection and format optimization.

Independent prompt solidification simplifies responsibilities for each small model and delivers more reliable performance.

## 2. Session Reuse: Leverage KV Cache for Higher Efficiency
### 2.1 Universal Rule: Reuse a Single Session Throughout Workflow
Standard coding workflow: *Requirement Analysis → Architecture Design → Code Writing → Debugging*
- **Rule**: Use **one single Ollama session** for the whole process. Do not create new sessions or reset context.
- **Performance Gain**: Reuse incremental KV Cache. Inference speed increases by over 30% compared with frequent session recreation.
- **Mechanism**: Ollama caches KV data of historical context, so repeated computation for previous content is avoided in multi-turn interactions.

### 2.2 Implementation Guidelines
1. For regular coding: Append new instructions to the existing session and retain full conversation context.
2. For debugging: Attach original code plus error logs directly to the current session. The model can locate bugs accurately with historical context.
3. **Forbidden Operation**: Never create a new session for a single step, which causes context loss and illogical code modification.

## 3. Inference Sampling Parameters: Reduce Hallucinations & Standardize Code
Coding is a deterministic task. Tune sampling parameters to limit randomness and offset weaknesses of small models.
```json
{
  "temperature": 0.1,
  "top_p": 0.5,
  "top_k": 20,
  "repeat_penalty": 1.1
}
```

### Temperature Tuning by Scenario
- Engineering code, APIs and business logic scripts: `0.1 ~ 0.2` for maximum rigor and minimal deviation.
- Complex algorithms and creative scripts: `0.3` for moderate flexibility.

**Parameter Explanations**:
- `top_p=0.5`: Narrow token candidate range to prioritize valid syntax.
- `top_k=20`: Restrict sampling pool to prevent abnormal or invalid code.
- `repeat_penalty=1.1`: Suppress repeated code fragments and redundant loops.

## 4. Context Management: Dynamic Truncation & Chunked Input
Adopt context truncation and chunked input to solve the limited context window of small models.

### 4.1 Dynamic Truncation Rules
Apply automatic truncation when content approaches or exceeds the `num_ctx` limit:
- **Retain**: Latest requirements, pending code, error logs and core architecture notes.
- **Discard**: Completed historical code, outdated conversations, redundant logs and useless remarks.

### 4.2 Chunked Input for Long Code
For large files, lengthy functions and complex classes:
1. Split code into individual functions or independent blocks, and feed them into the same session in batches.
2. Keep only the currently processed snippet and truncate finished content.
3. Adapt to small models’ context limitation and improve inference accuracy.

## 5. Incremental Continuation: Native Ollama Feature for Long Code Generation
### 5.1 Application Scenario
Use this feature when one round of inference cannot finish long code generation.

### 5.2 Workflow
1. The agent detects incomplete or truncated code output.
2. Call Ollama continuation API to generate content based on existing session data.
3. Reuse existing KV Cache without retransmitting the full code.

### 5.3 Advantages
- Reduce data transmission and achieve faster inference than full regeneration.
- Ensure consistent logic and code style with the help of session memory.

## 6. End-to-end Implementation Workflow
1. **Model Preparation**: Write dedicated Modelfiles for different roles, solidify `num_ctx`, system prompts and coding rules, then build customized models.
2. **Session Initialization**: Start Ollama with a persistent single session to enable KV Cache reuse.
3. **Task Execution**
   - Simple code: Send direct requests with `num_ctx=4096` and standard sampling parameters.
   - Long code / multi-file projects: Set `num_ctx=8192` and adopt chunked input.
4. **Context Monitoring**: Trigger dynamic truncation once content reaches the window threshold.
5. **Code Completion**: Call incremental continuation for unfinished code.
6. **Debugging & Iteration**: Append error logs to the current session for troubleshooting. Do not switch sessions at any stage.

## 7. Key Constraints & Common Pitfalls
1. Stick strictly to two context window values: 4096 and 8192. Do not enlarge the window to avoid KV Cache overload.
2. Embed all fixed rules into Modelfile. Avoid dynamic prompt concatenation during runtime.
3. Reuse one single session for the entire coding process. Frequent session recreation is prohibited.
4. Keep a low `temperature` for engineering code to control randomness and hallucinations.
5. Split large code files into chunks and apply dynamic truncation to fit small models’ context capability.