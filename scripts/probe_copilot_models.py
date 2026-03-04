#!/usr/bin/env python3
"""
Probe GitHub Copilot model names.

Queries /models, then sends "What model are you?" to every chat-capable
model and prints what each one self-reports vs. what the catalog calls it.

Usage:
    python scripts/probe_copilot_models.py [--claude-only]
"""

import argparse
import json
import os
import sys
import time
import urllib.request
import urllib.error


# ── Auth ──────────────────────────────────────────────────────────────────────

def load_oauth_token() -> str:
    path = os.path.expanduser("~/.config/github-copilot/apps.json")
    with open(path) as f:
        data = json.load(f)
    entry = next(iter(data.values()))
    return entry["oauth_token"]


def exchange_token(oauth_token: str) -> str:
    req = urllib.request.Request(
        "https://api.github.com/copilot_internal/v2/token",
        headers={
            "Authorization": f"token {oauth_token}",
            "User-Agent": "forgiven-probe/0.1.0",
        },
    )
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read())["token"]


# ── Model list ─────────────────────────────────────────────────────────────────

def fetch_models(api_token: str) -> list[dict]:
    req = urllib.request.Request(
        "https://api.githubcopilot.com/models",
        headers={
            "Authorization": f"Bearer {api_token}",
            "User-Agent": "forgiven-probe/0.1.0",
            "Copilot-Integration-Id": "vscode-chat",
        },
    )
    with urllib.request.urlopen(req) as resp:
        data = json.loads(resp.read())
    return data.get("data", [])


def is_chat_capable(model: dict) -> bool:
    mid = model.get("id", "")
    for skip in ("embed", "whisper", "tts", "dall", "codex"):
        if skip in mid:
            return False
    cap_type = (model.get("capabilities") or {}).get("type", "")
    if cap_type and cap_type != "chat":
        return False
    return True


# ── Single-turn probe ─────────────────────────────────────────────────────────

def ask_model(api_token: str, model_version: str, question: str) -> tuple[str, str]:
    """
    Returns (routed_model_from_sse, first_reply_text).
    routed_model_from_sse is the `model` field in the first SSE chunk.
    """
    payload = json.dumps({
        "model": model_version,
        "messages": [{"role": "user", "content": question}],
        "stream": True,
        "max_tokens": 120,
    }).encode()

    req = urllib.request.Request(
        "https://api.githubcopilot.com/chat/completions",
        data=payload,
        headers={
            "Authorization": f"Bearer {api_token}",
            "Content-Type": "application/json",
            "User-Agent": "forgiven-probe/0.1.0",
            "Copilot-Integration-Id": "vscode-chat",
        },
    )

    routed = ""
    reply = ""
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            buf = ""
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").rstrip("\n")
                buf += line + "\n"
                if not line.startswith("data: "):
                    continue
                payload_str = line[6:]
                if payload_str == "[DONE]":
                    break
                try:
                    chunk = json.loads(payload_str)
                except json.JSONDecodeError:
                    continue
                if not routed:
                    routed = chunk.get("model", "")
                delta = (chunk.get("choices") or [{}])[0].get("delta", {})
                reply += delta.get("content") or ""
    except urllib.error.HTTPError as e:
        body = e.read().decode()
        reply = f"[HTTP {e.code}] {body[:200]}"

    return routed, reply.strip()


# ── Main ───────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude-only", action="store_true",
                        help="Only probe Claude models")
    args = parser.parse_args()

    print("Authenticating…")
    oauth = load_oauth_token()
    token = exchange_token(oauth)
    print("Token acquired.\n")

    print("Fetching model list…")
    all_models = fetch_models(token)
    chat_models = [m for m in all_models if is_chat_capable(m)]
    if args.claude_only:
        chat_models = [m for m in chat_models if "claude" in m.get("id", "").lower()]
    print(f"Found {len(chat_models)} chat-capable model(s) to probe.\n")

    question = "What exact model and version are you? Reply in one sentence."

    header = f"{'Catalog name':<28} {'id':<30} {'version':<30} {'SSE model':<30} Reply"
    print(header)
    print("─" * len(header))

    for m in chat_models:
        mid     = m.get("id", "?")
        mver    = m.get("version") or mid
        mname   = m.get("name") or mid

        routed, reply = ask_model(token, mver, question)

        # Truncate reply for display
        short_reply = (reply[:80] + "…") if len(reply) > 80 else reply
        print(f"{mname:<28} {mid:<30} {mver:<30} {routed:<30} {short_reply}")

        # Avoid hammering the API
        time.sleep(0.5)


if __name__ == "__main__":
    main()
