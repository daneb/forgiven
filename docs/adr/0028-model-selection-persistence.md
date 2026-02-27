# ADR 0028: Model Selection Persistence

## Status
Accepted

## Context

ADR 0014 introduced dynamic model discovery and Ctrl+T cycling, with a `default_copilot_model` field in the config file. However, there were three critical UX issues:

1. **Config Not Applied on Startup**: Even with `default_copilot_model = "claude-sonnet-4-5"` in `config.toml`, the agent panel always showed `[gpt-4o]` until the user pressed Ctrl+T or submitted a message.

2. **No Save Functionality**: The `Config` struct could be loaded from disk but had no `save()` method, so there was no way to persist runtime changes back to the config file.

3. **Manual Selection Lost**: When users pressed Ctrl+T to cycle to their preferred model, that choice was lost on restart — they had to manually select it again every time they opened the IDE.

This created a frustrating loop:
- User sets `default_copilot_model` in config → IDE ignores it and shows `[gpt-4o]`
- User presses Ctrl+T to fix → Selection lost on restart
- User manually edits config file again → cycle repeats

The root cause was **lazy model loading**: the model list was only fetched when the user submitted a message or pressed Ctrl+T, so the panel title always showed the fallback (`"gpt-4o"`) instead of the configured default.

## Decision

Implement automatic model persistence with three changes:

### 1. Config::save() Method

Add a `save()` method to persist the current `Config` state to disk:

```rust
impl Config {
    /// Save the current config to `~/.config/forgiven/config.toml`.
    /// Creates the directory if it doesn't exist.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::config_path()
            .ok_or("HOME environment variable not set")?;
        
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        let toml_string = toml::to_string_pretty(self)?;
        std::fs::write(&path, toml_string)?;
        Ok(())
    }
}
```

**Key features:**
- Creates `~/.config/forgiven/` if it doesn't exist
- Uses `toml::to_string_pretty` for human-readable output
- Returns `Result` for error handling

### 2. Eager Model Loading on Panel Open

Modified `Action::AgentToggle` and `Action::AgentFocus` handlers to fetch models immediately:

```rust
Action::AgentToggle => {
    self.agent_panel.toggle_visible();
    if self.agent_panel.visible {
        self.mode = Mode::Agent;
        // Eagerly load models on first show
        if self.agent_panel.available_models.is_empty() {
            let preferred = self.config.default_copilot_model.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    if let Err(e) = self.agent_panel.ensure_models(&preferred).await {
                        tracing::warn!("Could not fetch model list: {e}");
                    }
                });
            });
        }
    } else {
        self.mode = Mode::Normal;
    }
}
```

**Timing:**
- Happens when panel opens (Ctrl+A or Action::AgentToggle)
- Only on first open (checked via `available_models.is_empty()`)
- Network call (~300-700ms) happens before panel is fully interactive
- User sees configured model immediately in panel title

### 3. Auto-Save on Model Cycle

Modified the Ctrl+T handler to persist the selected model:

```rust
KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    // ... model loading code ...
    
    self.agent_panel.cycle_model();
    let model = self.agent_panel.selected_model_id().to_string();
    let n = self.agent_panel.available_models.len();
    let idx = self.agent_panel.selected_model + 1;
    
    // Save the selected model to config
    self.config.default_copilot_model = model.clone();
    if let Err(e) = self.config.save() {
        tracing::warn!("Failed to save config: {e}");
    }
    
    self.set_status(format!("Agent model → {model}  [{idx}/{n}]  (Ctrl+T to cycle)"));
}
```

**Flow:**
1. User presses Ctrl+T
2. Model cycles to next in list
3. `config.default_copilot_model` updated in memory
4. `config.save()` writes entire config to disk
5. Next IDE launch will use the saved model

### 4. Pass preferred_model Through Call Chain

Fixed `submit()` to respect the configured default (this was a separate bug):

```rust
// In submit() signature:
pub async fn submit(
    &mut self,
    context: Option<String>,
    project_root: PathBuf,
    max_rounds: usize,
    warning_threshold: usize,
    preferred_model: &str,  // ← Added parameter
) -> Result<()>

// In submit() body when fetching models:
self.set_models(models, preferred_model);  // ← Was hardcoded to "gpt-4o"

// In editor call site:
let preferred_model = self.config.default_copilot_model.clone();
let fut = panel.submit(context, project_root, max_rounds, warning_threshold, &preferred_model);
```

## Implementation Details

### Config File Format

After cycling to `claude-sonnet-4-5`, the saved config looks like:

```toml
tab_width = 4
use_spaces = true
default_copilot_model = "claude-sonnet-4-5"
max_agent_rounds = 20
agent_warning_threshold = 3

[[lsp.servers]]
language = "rust"
command = "rust-analyzer"
args = []

[[lsp.servers]]
language = "copilot"
command = "npx"
args = ["--yes", "@github/copilot-language-server@latest", "--stdio"]
```

**Important:** Root-level config must come **before** `[[lsp.servers]]` sections. TOML parses any key-value pairs after an array-of-tables declaration as part of that table.

### Network Timing

**Before (lazy loading):**
- Open panel → Instant (shows `[gpt-4o]`)
- Submit first message → 300-700ms delay (fetches models + starts chat)

**After (eager loading):**
- Open panel → 300-700ms delay (fetches models)
- Submit first message → Instant (models already loaded)

The total latency is the same, but it's moved to a more intuitive moment (when opening the panel) rather than when submitting.

### Error Handling

All I/O operations use `Result` with appropriate logging:

```rust
if let Err(e) = self.config.save() {
    tracing::warn!("Failed to save config: {e}");
}
```

Failures are non-fatal — the model selection still works, it just won't persist. Common failure cases:
- Disk full
- Permission denied on `~/.config/forgiven/`
- Invalid UTF-8 in existing config (toml serialization fails)

## Consequences

### Positive

- **Respects User Preferences**: Configured models are applied immediately on panel open
- **Zero Manual Edits**: Users can select models via Ctrl+T without touching the config file
- **Persistent Selection**: Model choice survives IDE restarts
- **Better UX Flow**: Network delay happens when opening panel (expected) not when submitting
- **Config Round-Trip**: Config can now be both loaded and saved programmatically

### Negative

- **Disk I/O on Every Cycle**: Each Ctrl+T press writes to disk (~1-2ms on SSD)
  - Acceptable: keyboard input is inherently throttled by human speed (~200ms+ between presses)
  - Alternative considered: debounced save after N seconds — rejected as adding complexity
- **Modal Blocking on Panel Open**: The `block_in_place` call freezes the UI thread for 300-700ms
  - Acceptable: only happens once per session, and users expect some delay when opening networked features
  - Alternative: async load with spinner — future enhancement
- **Full Config Rewrite**: Saving model selection rewrites entire config file
  - Risk: Could lose comments or custom formatting
  - Mitigation: TOML `to_string_pretty` preserves structure; comments are regenerated on manual edit

### Neutral

- Config changes made outside the IDE (manual edits) are not hot-reloaded — requires restart
- The `save()` method is public and could be used for other config mutations in the future
- Model list is still fetched even if config has a valid default (can't verify validity without fetching)

## Alternatives Considered

### 1. Separate Model Config File

Store model selection in `~/.config/forgiven/model.txt` instead of main config.

**Rejected:**
- Splits related settings across multiple files
- User expectation is that config.toml is the single source of truth
- More files = more complexity

### 2. Debounced Save (Write After Delay)

Save config only after user stops pressing Ctrl+T for 2-3 seconds.

**Rejected:**
- Adds state machine complexity (timer tracking, cancel logic)
- Risk: user cycles model then closes IDE quickly → preference not saved
- Premature optimization: disk writes are fast enough

### 3. Async Model Loading with Spinner

Show a loading spinner while fetching models on panel open, keep UI responsive.

**Deferred:**
- Would improve polish but adds rendering complexity
- Current blocking approach is simple and acceptable for ~500ms delay
- Could be future enhancement if users complain about lag

### 4. Cache Model List on Disk

Store fetched models in `~/.cache/forgiven/models.json` to avoid network call on every launch.

**Rejected:**
- Adds cache invalidation complexity (how old is too old?)
- Users would need Ctrl+Shift+T to refresh cached list
- Current approach is simpler: always fetch fresh data

## Related

- **ADR 0014**: Agent Model Selection (introduced dynamic discovery and Ctrl+T cycling)
- **ADR 0011**: Agentic Tool-Calling Loop (model is passed to agent loop)
- **ADR 0027**: Agent Round Limits (uses same config save mechanism for round settings)

## Future Enhancements

- [ ] **Async panel open**: Show spinner while loading models instead of blocking
- [ ] **Model validation**: Warn if `default_copilot_model` is not in fetched list
- [ ] **Per-project models**: Allow `.forgiven.toml` to override default model
- [ ] **Recent models list**: Track last 3 used models for quick switching
- [ ] **Model categories**: Group models by provider (OpenAI, Anthropic, Google) in UI
- [ ] **Hot config reload**: Watch `config.toml` for external changes and reload

## Testing Notes

Verified behavior:

1. ✅ Fresh install with no config → defaults to `gpt-4o`
2. ✅ Config with `default_copilot_model = "claude-sonnet-4-5"` → panel shows `[claude-sonnet-4-5]` immediately
3. ✅ Ctrl+T cycles through models → selection saved to disk
4. ✅ Restart IDE after Ctrl+T → persisted model is selected
5. ✅ Manual config edit to different model → applied on next launch
6. ✅ Invalid model in config → falls back to `gpt-4o`, then first available

## Notes

This ADR completes the model selection feature by closing the persistence gap. Users now have three ways to set their default model:

1. **Manual**: Edit `~/.config/forgiven/config.toml` and add `default_copilot_model = "model-id"`
2. **Interactive**: Press Ctrl+T in the agent panel to cycle and auto-save
3. **Programmatic**: Call `config.default_copilot_model = "..."; config.save()`

All three methods are now consistent and persistent across restarts.
