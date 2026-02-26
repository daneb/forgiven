# Configurable Copilot Model Default

## Overview

You can now configure your preferred Copilot model in `~/.config/forgiven/config.toml` and refresh the model list to pick up newly released models without restarting the editor.

## Configuration Example

```toml
# ~/.config/forgiven/config.toml

# Set your preferred Copilot model (defaults to "gpt-4o" if not specified)
default_copilot_model = "claude-sonnet-4-5"

# Other settings
tab_width = 4
use_spaces = true

[[lsp.servers]]
language = "rust"
command = "rust-analyzer"
args = []
```

## Keybindings

When in the Copilot Chat panel:

- **`Ctrl+T`** - Cycle through available models
  - On first press, fetches the model list from GitHub Copilot API
  - Selects your configured default (or `gpt-4o` if not set)
  - Shows: `Agent model → claude-sonnet-4-5 [2/8] (Ctrl+T to cycle)`

- **`Ctrl+Shift+T`** - Refresh model list from API
  - Picks up newly released models without restarting the editor
  - Preserves your current selection if the model is still available
  - Falls back to your configured default if the current model was removed
  - Shows: `Refreshed 8 models, selected: claude-sonnet-4-5`

## How It Works

### Smart Model Selection

The system uses a three-tier fallback strategy:

1. **Your configured preference** (`default_copilot_model` in config.toml)
2. **`gpt-4o`** (if your preference isn't available)
3. **First available model** (if neither of the above exist)

### Refresh Behavior

When you press `Ctrl+Shift+T`:
- Fetches the latest model list from GitHub Copilot
- If you were using `claude-sonnet-4-5` and it's still available → keeps it
- If your model was removed → switches to your configured default
- Updates the panel immediately with the new count and selection

### Model Persistence

- **Per-conversation**: The model you select is used for all rounds of that conversation
- **Across restarts**: Your configured default in `config.toml` is used on next startup
- **Session memory**: Your manually selected model persists until you close the editor

## Implementation Details

See [ADR 0014](docs/adr/0014-agent-model-selection.md) for technical details about:
- Dynamic model discovery via `/models` API
- Configuration integration
- Keybinding design decisions
- Fallback strategies
