# ADR 0037 — Think-Block Rendering in the Agent Panel

**Status:** Accepted

---

## Context

Some AI models — notably DeepSeek-R1, QwQ, and Qwen 3 in thinking mode — emit
chain-of-thought reasoning wrapped in `<think>…</think>` XML tags before their
actual reply. The tags are part of the raw response text, not a separate API field.

Before this change, any `<think>` tags in a Copilot reply flowed unchanged into the
markdown renderer. This had two problems:

1. **Visual noise** — the raw `<think>` and `</think>` tag strings appeared literally
   in the chat panel alongside the reasoning text, which was confusing.
2. **Wrong formatting** — chain-of-thought text is informal, unpunctuated, and
   sometimes incomplete mid-sentence; rendering it as markdown (with heading
   detection, list parsing, bold markers, etc.) produced garbled output.

The fix separates the two kinds of content: thinking blocks are shown as plain,
dim-gray word-wrapped text, while the actual reply continues to be rendered as
formatted CommonMark markdown.

---

## Decision

### `split_thinking` — content segmentation (`src/agent/mod.rs`)

A new `ContentSegment` enum and `split_thinking` free function are added alongside
the existing `ChatMessage` / `Role` data types:

```rust
pub enum ContentSegment {
    Thinking(String),  // inside <think>…</think>
    Normal(String),    // outside any think tag
}

pub fn split_thinking(content: &str) -> Vec<ContentSegment>
```

The function walks the string, alternating between `Normal` and `Thinking` segments:

- Everything before `<think>` → `Normal`
- Everything between `<think>` and `</think>` → `Thinking`
- An **unclosed** `<think>` (no `</think>` found) → trailing `Thinking` segment
  containing the rest of the string

The unclosed-tag case is the streaming path: while the model is still emitting
reasoning, the `</think>` has not yet arrived. The function handles this correctly
without any extra state in the caller.

Multiple `<think>…</think>` blocks in a single message are supported (the function
loops until `remaining` is exhausted).

### `render_message_content` — think-aware rendering (`src/ui/mod.rs`)

A new module-level free function replaces the direct `crate::markdown::render()`
calls in the agent panel's chat history loop and streaming-reply block:

```rust
fn render_message_content(content: &str, width: usize) -> Vec<Line<'static>>
```

For each `ContentSegment`:

| Segment | Output |
|---------|--------|
| `Thinking(text)` | `◌ thinking` header (dim gray, italic) + plain word-wrapped dim-gray text |
| `Normal(text)` | Passed through `crate::markdown::render()` unchanged |

A blank spacer line is appended after each thinking block to visually separate it
from the answer that follows.

The plain word-wrap for thinking blocks is intentionally simple: split on `\n` into
paragraphs, then greedily pack words up to `width` columns. No list detection, no
heading detection, no inline spans — the raw reasoning is shown as-is.

### Call-site changes

Both existing `crate::markdown::render()` calls inside `render_agent_panel` are
replaced with `render_message_content()`:

```rust
// completed messages
lines.extend(render_message_content(&msg.content, content_width));

// live streaming reply
lines.extend(render_message_content(partial, content_width));
```

No changes to `ChatMessage`, `StreamEvent`, `AgentPanel`, or the streaming loop.

---

## Alternatives considered

**Strip thinking content entirely**

Hiding the chain-of-thought would keep the panel clean, but developers often want to
inspect the model's reasoning — especially when debugging unexpected answers. Showing
it dimmed preserves the information without competing with the reply.

**Collapsible / toggle section**

A folded `▶ thinking (3 lines)` widget that expands on a keypress would be the
ideal long-term UX. It requires per-block expanded/collapsed state in `AgentPanel`,
scroll-position accounting when a block is toggled, and additional key handling in
`handle_agent_mode`. Deferred; the dimmed-but-visible approach is sufficient now.

**Parse thinking content as markdown anyway**

Chain-of-thought text from reasoning models is deliberately unstructured — it uses
partial sentences, ellipses, and informal notation. Feeding it to a CommonMark parser
produces false positives (random `*` pairs becoming bold, bare `#` becoming a
heading). Plain word-wrap is more appropriate.

**Separate `thinking` field on `ChatMessage`**

Splitting the struct into `content` + `thinking_content` would require changes to
the streaming parser (which today accumulates all tokens into a single `String`), the
agentic loop's message assembly, and potentially the Copilot API request body. The
string-splitting approach keeps the diff minimal and works retroactively on any
already-stored messages that happen to contain `<think>` tags.

---

## Consequences

**Positive**
- Thinking content is immediately distinguishable from the model's actual answer.
- No markdown false-positives inside chain-of-thought text.
- Works during streaming: the unclosed-tag path means the dim style activates the
  moment the model begins reasoning, before `</think>` arrives.
- Retroactively correct for any message already in `panel.messages` that contains
  `<think>` tags.
- Zero overhead for models that never emit `<think>` tags — `split_thinking` finds no
  match and returns a single `Normal` segment identical to the previous behaviour.
- No new modes, no new state, no API changes.

**Negative / trade-offs**
- Thinking blocks are always visible; long reasoning chains push the actual answer
  further down and require scrolling. A collapsible widget (deferred) would address
  this.
- The simple greedy word-wrap ignores tab characters and runs of multiple spaces
  inside thinking text. Acceptable for informal chain-of-thought content.
- Tag matching is case-sensitive (`<think>`, not `<THINK>`). All known reasoning
  models use lowercase; this is not a practical limitation.
