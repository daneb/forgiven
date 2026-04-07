# MCP Server Configuration Guide

## Overview

Claude Code supports two transports for MCP servers: **stdio** (forked child process) and **SSE/HTTP** (persistent server). Choosing the wrong one is the most common source of resource waste — particularly when Docker is involved.

## How MCP servers are spawned

### stdio (`command`/`args`)

```toml
[[mcp.servers]]
name = "my-server"
command = "npx"
args = ["-y", "some-mcp-package"]
```

Claude Code **forks a new child process** for every session that initializes this server. Each open window, tab, worktree, or background subagent gets its own isolated process.

- **1 session = 1 process**
- Processes are cleaned up when the session closes
- Fine for lightweight binaries (`npx`, local `node`/`python` scripts)

### SSE/HTTP (`url`)

```toml
[[mcp.servers]]
name = "my-server"
url = "http://localhost:3000/sse"
```

Claude Code **connects to an already-running server**. All sessions share the same server instance.

- **N sessions = 1 server**
- You are responsible for starting and stopping the server
- Required for Docker images, stateful servers, or anything with heavy startup cost

## The Docker trap

Using `docker run` in a `command`/`args` config creates a new container per session:

```toml
# This spawns one container per open Claude Code session
[[mcp.servers]]
name = "searxng"
command = "docker"
args = ["run", "--rm", "-i", "-e", "SEARXNG_URL", "isokoliuk/mcp-searxng"]
```

With 10 open sessions you get 10 running containers. The `--rm` flag cleans them up on close, but while sessions are live they all coexist. Background agents and subagents each count as a session.

## Choosing the right transport

| Transport | Config key | Process model | Best for |
|-----------|------------|---------------|----------|
| stdio | `command`/`args` | 1 process per session | `npx`, local binaries, Python scripts |
| SSE/HTTP | `url` | 1 shared server | Docker images, heavy servers, stateful servers |

## Fixing a Docker-based server

**Option A — Persistent container with `url` (recommended)**

Start the container once with a port exposed:

```bash
docker run -d --rm \
  -p 3000:3000 \
  -e SEARXNG_URL="http://searxng.searxng-mcp.orb.local" \
  isokoliuk/mcp-searxng
```

Then switch the config to `url`:

```toml
[[mcp.servers]]
name = "searxng"
url = "http://localhost:3000/sse"
```

**Option B — Replace Docker with `npx`**

If the server has an npm package, `npx` is cheaper to fork than Docker:

```toml
[[mcp.servers]]
name = "searxng"
command = "npx"
args = ["-y", "mcp-searxng"]
env = { SEARXNG_URL = "http://searxng.searxng-mcp.orb.local" }
```

**Option C — Accept it**

If you only have a few sessions open at a time and containers are lightweight, the `--rm` cleanup on close bounds the leak. Monitor with `docker ps`.

## Checklist for new MCP servers

- `args` contains `docker run` → prefer persistent container + `url`
- `args` contains `npx` / `node` / `python` → stdio is fine
- Server has shared state or a slow startup → persistent + `url`
- Server is stateless and fast to start → stdio is fine
