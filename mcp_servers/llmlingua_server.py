#!/usr/bin/env python3
"""
LLMLingua MCP Server — compress verbose text before it enters the context window.

Exposes one tool:
  compress_text(text, rate=0.5, keep_first_sentence=true)
    → compressed text at the requested token ratio

Usage in ~/.config/forgiven/config.toml:
  [[mcp.servers]]
  name    = "llmlingua"
  command = "python3"
  args    = ["/path/to/mcp_servers/llmlingua_server.py"]

Dependencies (install once):
  pip install llmlingua

The server uses the "llmlingua-2-bert-base-multilingual-cased-meetingbank" model
by default — it is ~670 MB and is cached by HuggingFace on first run.
Set LLMLINGUA_MODEL env var to override (e.g. "microsoft/phi-2" for stronger
but slower compression).

When to use:
  - Compressing long error logs / stack traces returned by run_command
  - Compressing long grep/test output before passing to the agent
  - DO NOT use on code you are about to edit — token removal may corrupt semantics
"""

import json
import os
import sys
import traceback

# ---------------------------------------------------------------------------
# Lazy-load LLMLingua so startup is fast even if the package is not installed.
# ---------------------------------------------------------------------------
_compressor = None


def _get_compressor():
    global _compressor
    if _compressor is not None:
        return _compressor
    try:
        from llmlingua import PromptCompressor  # type: ignore
    except ImportError:
        raise RuntimeError(
            "llmlingua package not found. Install it with: pip install llmlingua"
        )
    model_name = os.environ.get(
        "LLMLINGUA_MODEL",
        "microsoft/llmlingua-2-bert-base-multilingual-cased-meetingbank",
    )
    _compressor = PromptCompressor(
        model_name=model_name,
        use_llmlingua2=True,
        device_map="cpu",
    )
    return _compressor


# ---------------------------------------------------------------------------
# MCP JSON-RPC 2.0 stdio transport
# ---------------------------------------------------------------------------

def send(obj: dict):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def handle_initialize(req_id, _params):
    send({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "llmlingua", "version": "1.0.0"},
        },
    })


def handle_tools_list(req_id, _params):
    send({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "tools": [
                {
                    "name": "compress_text",
                    "description": (
                        "Compress verbose text (error logs, grep output, documentation) "
                        "to reduce token count before passing it to the agent. "
                        "Use rate=0.5 to keep 50% of tokens (safe default). "
                        "Use rate=0.3 for aggressive compression of very long outputs. "
                        "DO NOT compress source code you intend to edit."
                    ),
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "The text to compress.",
                            },
                            "rate": {
                                "type": "number",
                                "description": (
                                    "Target compression ratio: fraction of tokens to keep "
                                    "(0.3 = aggressive, 0.5 = moderate, 0.7 = light). "
                                    "Default: 0.5"
                                ),
                                "default": 0.5,
                            },
                            "keep_first_sentence": {
                                "type": "boolean",
                                "description": (
                                    "Always preserve the first sentence verbatim. "
                                    "Useful for retaining error type / log level. Default: true"
                                ),
                                "default": True,
                            },
                        },
                        "required": ["text"],
                    },
                }
            ]
        },
    })


def handle_tools_call(req_id, params):
    name = params.get("name")
    args = params.get("arguments", {})

    if name != "compress_text":
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {"code": -32601, "message": f"Unknown tool: {name}"},
        })
        return

    text = args.get("text", "")
    rate = float(args.get("rate", 0.5))
    keep_first = bool(args.get("keep_first_sentence", True))

    if not text.strip():
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {"content": [{"type": "text", "text": ""}]},
        })
        return

    # Short texts (< 200 chars) are not worth compressing.
    if len(text) < 200:
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {"content": [{"type": "text", "text": text}]},
        })
        return

    try:
        compressor = _get_compressor()
        result = compressor.compress_prompt(
            text,
            rate=rate,
            force_tokens=["\n"] if keep_first else [],
        )
        compressed = result["compressed_prompt"]
        orig_tokens = result.get("origin_tokens", "?")
        comp_tokens = result.get("compressed_tokens", "?")
        header = f"[compressed {orig_tokens}→{comp_tokens} tokens]\n"
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {"content": [{"type": "text", "text": header + compressed}]},
        })
    except Exception as exc:  # noqa: BLE001
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {"code": -32603, "message": f"Compression failed: {exc}\n{traceback.format_exc()}"},
        })


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue

        req_id = req.get("id")
        method = req.get("method", "")
        params = req.get("params", {})

        if method == "initialize":
            handle_initialize(req_id, params)
        elif method == "notifications/initialized":
            pass  # no response needed
        elif method == "tools/list":
            handle_tools_list(req_id, params)
        elif method == "tools/call":
            handle_tools_call(req_id, params)
        else:
            if req_id is not None:
                send({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": {"code": -32601, "message": f"Method not found: {method}"},
                })


if __name__ == "__main__":
    main()
