# ADR 0084: LLMLingua MCP Sidecar for Tool Result Compression

**Date:** 2026-03-23
**Status:** Accepted

## Context

Some tool results are inherently verbose: long stack traces, test output across many files, large grep results. These can consume thousands of tokens even after the agent has extracted the key information. LLMLingua (Microsoft Research) is a prompt compression library that uses a small language model to identify and drop low-perplexity (predictable/redundant) tokens, achieving 4–10× compression with minimal information loss.

This is implemented as an optional MCP sidecar (`mcp_servers/llmlingua_server.py`) rather than built into Forgiven because:
- It requires Python + a ~670 MB model download (not suitable as a mandatory dependency)
- It is only valuable for heavy agent sessions with long tool output
- The existing MCP infrastructure handles all integration plumbing already

## Decision

`mcp_servers/llmlingua_server.py` is a stdio MCP server that exposes a single tool:

**`compress_text(text, rate=0.5, keep_first_sentence=true)`**
- `rate`: fraction of tokens to retain (0.3 = aggressive, 0.5 = moderate, 0.7 = light)
- Returns `[compressed N→M tokens]\n<compressed text>`
- Texts shorter than 200 characters are returned unchanged
- Uses `llmlingua-2-bert-base-multilingual-cased-meetingbank` by default (fast, multilingual)
- Override with `LLMLINGUA_MODEL` env var for stronger compression (e.g. `microsoft/phi-2`)

### Config (opt-in)

```toml
[[mcp.servers]]
name    = "llmlingua"
command = "python3"
args    = ["/path/to/mcp_servers/llmlingua_server.py"]
```

### Install dependency

```bash
pip install llmlingua
```

### When to use (documented in tool description)

| Use | Rate |
|-----|------|
| Stack traces / error logs | 0.5 |
| Long grep / test output | 0.3–0.4 |
| Documentation snippets | 0.5 |
| **Source code being edited** | **Never** |

Compressing code risks removing semantically critical tokens (operators, identifiers, brackets). The tool description explicitly warns against this.

## Consequences

- Zero Rust code changes; purely additive optional sidecar.
- ~670 MB model download on first use (cached by HuggingFace).
- Compression quality degrades below ~50 tokens of input — the 200-char threshold skips short texts.
- The model runs on CPU by default; GPU acceleration requires setting `device_map="cuda"` in the script.
- Not suitable as a default-on feature — users opt in by adding the `[[mcp.servers]]` entry.
