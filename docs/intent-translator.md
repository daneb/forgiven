# SPEC: Intent Translator (Option D)

**Status:** Draft — not yet reviewed
**Priority:** Rank 1 of 4 in the AI-IDE architecture exploration
**Estimated size:** ~400 LoC
**Estimated time:** 3–5 days of focused work
**Dependencies:** none — orthogonal to existing agent architecture

---

## Goal

Add a lightweight preprocessing step that rewrites the user's message into a structured task specification *before* the main agent sees it. A small, fast model (Claude Haiku 4.5, DeepSeek V3.2, or a local 7–9B model) acts as an **intent translator**. The main agent operates on the structured spec instead of the raw message.

This is explicitly **not** a "new language for LLMs" or a compression layer. It is upstream structure-adding: a documented research pattern (Haseeb 2025, "Context Engineering for Multi-Agent LLM Code Assistants") shown to improve single-shot success rates on ambiguous prompts.

---

## Problem

Forgiven currently passes the user's raw message directly into the agent loop after prepending the system prompt and open-file context. This works well for precise prompts ("add a test for `HoverHandler::request` that covers the error path") but produces rambling, tool-heavy sessions for underspecified prompts ("fix the thing in the agent panel that's weird").

Observed cost pattern from `sessions.jsonl`:

- Ambiguous prompts trigger exploratory `read_file` loops (3–8 calls) as the agent tries to infer what the user means.
- Those tool calls enter history and are re-sent every round.
- The resulting session routinely accumulates 150k+ tokens before producing a usable answer.

An Intent Translator prevents this at the source: it either clarifies the intent *in-chat* or rewrites it as a crisp task spec before the expensive agent loop begins.

---

## Proposed design

### Translator contract

A new async function `translate_intent(user_message: &str, ctx: &TranslationContext) -> Intent` runs before `submit()` dispatches to the agent loop.

```rust
pub struct TranslationContext<'a> {
    pub open_file: Option<&'a str>,        // path, not content
    pub recent_files: &'a [String],        // last N opened files
    pub project_root: &'a Path,
    pub language_hint: Option<&'a str>,    // e.g. "rust"
}

pub struct Intent {
    pub goal: String,                      // one sentence — the intended outcome
    pub scope: IntentScope,                // files/symbols expected to be relevant
    pub expected_output: OutputType,       // Code | Diff | Explanation | Question
    pub ambiguities: Vec<String>,          // clarifications the agent should ask, or []
    pub structured_prompt: String,         // the rewritten prompt for the main agent
}

pub enum IntentScope {
    SingleFile(PathBuf),
    MultiFile(Vec<PathBuf>),
    Symbol { file: PathBuf, symbol: String },
    ProjectWide,
    Unknown,
}

pub enum OutputType {
    Code,
    Diff,
    Explanation,
    Question,
    Mixed,
}
```

### Translator prompt (v1)

```
You are an intent translator for a Rust IDE. You do NOT answer the user's question.
You REWRITE it into a structured spec the main agent will execute.

Given:
- User message: {raw_message}
- Open file: {open_file or "none"}
- Recent files: {recent_files}
- Project language: {language_hint}

Produce JSON with:
  goal: one-sentence outcome (imperative, e.g. "Add error handling to X::foo")
  scope: which files/symbols are in scope (SingleFile | MultiFile | Symbol | ProjectWide | Unknown)
  expected_output: Code | Diff | Explanation | Question | Mixed
  ambiguities: list of clarifying questions (empty if the intent is clear)
  structured_prompt: the rewritten prompt, 1-3 sentences, that the main agent will receive

If the user message contains 2+ genuine ambiguities, populate `ambiguities` and leave
`structured_prompt` empty. The IDE will ask the user to clarify before dispatching.

Output only the JSON, no preamble.
```

### Decision flow in `submit()`

```
user presses Enter
    |
    v
translate_intent() — ~1s for cloud small model, ~2s for local 7B
    |
    +--> Intent.ambiguities.is_empty() == false
    |        |
    |        v
    |    Show `ask_user` popup with the ambiguities
    |    Collect answers, re-run translate_intent with the answers appended
    |
    +--> Intent.ambiguities.is_empty() == true
             |
             v
         Main agent receives: system_prompt + Intent.structured_prompt
         (NOT the raw message)
```

The user sees the translation in the chat panel as a dim preamble:

```
🎯 Goal: Add error handling to HoverHandler::request
   Scope: src/lsp/hover.rs (Symbol: HoverHandler)
   Output: Diff
```

They can press `Esc` during the translation to bypass (`SPC a t` toggle).

### Configuration

```toml
[agent.intent_translator]
# Enable the translator. Default: false until measured on corpus.
enabled = false

# Provider: "claude-haiku" | "deepseek" | "ollama" | "copilot"
provider = "claude-haiku"

# For ollama, the model to use
ollama_model = "qwen2.5-coder:7b"

# Skip translation for messages shorter than this (already crisp)
min_chars_to_translate = 40

# Skip translation if the message matches any regex (pre-translated intents)
skip_patterns = [
    "^\\s*/",              # slash commands
    "^\\s*(fix|add) test",  # already-crisp prefixes
]

# If the translator fails or times out, fall through to raw message
timeout_ms = 5000
```

### Provider abstraction

A `IntentProvider` trait mirrors the existing `ProviderKind` pattern from ADR 0098 / 0116:

```rust
#[async_trait]
pub trait IntentProvider: Send + Sync {
    async fn translate(&self, ctx: &TranslationContext, message: &str) -> Result<Intent>;
}
```

Implementations:

- `ClaudeHaikuProvider` — direct Anthropic API call, ~$0.001 per translation
- `DeepSeekProvider` — direct DeepSeek API, ~$0.0003 per translation
- `OllamaProvider` — local, zero cost, 1–3s latency
- `CopilotProvider` — reuses the existing Copilot gateway; counts against quota

The provider is selected per-session from config. No routing logic — the user picks.

---

## Acceptance criteria

- [ ] `IntentProvider` trait defined in `src/agent/intent.rs` with the four implementations.
- [ ] `translate_intent()` integrated into `submit()` before the agent loop dispatch.
- [ ] Translation appears as a dim preamble in the chat panel.
- [ ] `ambiguities` non-empty produces an `ask_user` popup pre-populated with the questions.
- [ ] `SPC a t` toggles the translator on/off for the current session without editing config.
- [ ] If the translator fails (timeout, JSON parse error, provider error), the raw message is passed through unchanged and an `info!` log line is emitted.
- [ ] A test suite asserts: (1) timeout triggers fallthrough, (2) malformed JSON triggers fallthrough, (3) valid JSON with `ambiguities = []` passes structured_prompt through, (4) non-empty `ambiguities` triggers `ask_user`.

---

## Measurement plan

Measured against `forgiven-bench/` corpus (once built; see corpus spec):

| Metric | Target |
|---|---|
| Mean tokens per task | ≥ 20% reduction vs. no translator |
| Answer quality (F1 vs. golden) | No drop > 2 percentage points |
| P95 first-token latency | ≤ +1.5s with cloud provider |
| Translation success rate | ≥ 95% (rest fall through silently) |

Until the corpus exists, interim signal: track `SPC d` for session-total token counts over 10 real sessions with translator on vs. off.

---

## Risks and trade-offs

**Latency.** Every user message adds 1–3s before the main agent starts. For short, well-phrased prompts this is pure overhead. Mitigated by `min_chars_to_translate` and `skip_patterns`.

**Over-translation.** A precise prompt like "rename `foo` to `bar` in `src/x.rs`" could be rewritten into something less precise. Mitigated by the translator prompt explicitly instructing it to keep crisp prompts unchanged (`structured_prompt = raw_message` when intent is already clear).

**Provider dependency.** Haiku and DeepSeek require an API key and network. Mitigated by Ollama fallback and by `enabled = false` default.

**Duplicate intent.** Every translated message enters history twice (raw in the UI, structured in the prompt). Mitigated by only sending the structured version to the API; the raw version is display-only.

---

## Out of scope

- Training a custom translation model.
- Using the translator for follow-up messages within a session (only the first message per conversation is translated — follow-ups inherit the established intent).
- Multi-language translation (translator is English-to-structured-English only).
- Fine-tuning provider selection per project.

---

## Implementation order

1. Define `Intent`, `TranslationContext`, `IntentScope`, `OutputType` in `src/agent/intent.rs`.
2. Implement `ClaudeHaikuProvider` — smallest dependency, easiest to test.
3. Wire `translate_intent()` into `submit()` behind the `enabled = false` config flag.
4. Add chat panel rendering for the dim preamble.
5. Wire `ask_user` popup for `ambiguities`.
6. Add `DeepSeekProvider`, `OllamaProvider`, `CopilotProvider`.
7. Add `SPC a t` keybind.
8. Write integration tests.
9. Document in a new ADR.

Total: 3–5 days focused.

---

## Related work

- **ADR 0057** — `ask_user` tool. The translator reuses the ask-user popup infrastructure.
- **ADR 0128** — Investigation subagent. Similar pattern (small prompt, structured output, injection into main loop), different purpose (exploration vs. intent clarification).
- **ADR 0100** — Spec Slicer. Same philosophy: pre-compute structure before the agent runs, don't ask the agent to do structure work itself.
- **ADR 0116** — Multi-provider LLM backend. Defines the provider abstraction pattern reused here.

## References

- Haseeb, M. (August 2025). *Context Engineering for Multi-Agent LLM Code Assistants Using Elicit, NotebookLM, ChatGPT, and Claude Code.* arXiv:2508.08322. — Introduces the "Intent Translator" pattern as component (1) in a multi-agent context-engineering workflow.
