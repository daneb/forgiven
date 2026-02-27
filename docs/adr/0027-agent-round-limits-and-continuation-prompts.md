# ADR 0027: Agent Round Limits and Continuation Prompts

## Status
Accepted

## Context

The agentic tool-calling loop had a hardcoded limit of 20 rounds before terminating with an error. This created several problems:

1. **No User Control**: Users couldn't configure the limit based on their needs or task complexity
2. **Abrupt Termination**: When the limit was reached, the agent simply stopped with an error message
3. **No Visibility**: Users had no indication of how many rounds had elapsed during long-running operations
4. **Wasted Work**: If a task needed 22 rounds and the limit was 20, the user lost all progress instead of being able to approve continuation

This differs from professional AI assistants (like VS Code Copilot) which pause before hitting limits and ask users if they want to continue, offering options to:
- Continue iterating
- Stop and refine the prompt  
- Configure the maximum rounds

## Decision

Implement a configurable, user-interactive round limit system with progress tracking and continuation prompts.

### 1. Configuration

Add to `~/.config/forgiven/config.toml`:

```toml
# Maximum agentic loop rounds before prompting user
max_agent_rounds = 20

# Warn when this many rounds remain before the limit
agent_warning_threshold = 3
```

### 2. New StreamEvent Variants

```rust
pub enum StreamEvent {
    // ... existing variants ...
    
    /// Progress indicator showing current round and max.
    RoundProgress { current: usize, max: usize },
    
    /// Warning that max rounds is approaching.
    MaxRoundsWarning { current: usize, max: usize, remaining: usize },
    
    /// Loop paused, waiting for user decision on continuation.
    AwaitingContinuation,
}
```

### 3. AgentPanel State Tracking

```rust
pub struct AgentPanel {
    // ... existing fields ...
    
    /// Channel to send continuation decisions to the agentic loop.
    pub continuation_tx: Option<mpsc::UnboundedSender<bool>>,
    
    /// Current round (for UI display).
    pub current_round: usize,
    
    /// Maximum configured rounds.
    pub max_rounds: usize,
    
    /// Whether paused waiting for user approval to continue.
    pub awaiting_continuation: bool,
}
```

### 4. Agentic Loop Behavior

The loop now:

1. **Reports Progress**: Sends `RoundProgress` at the start of each round
2. **Warns Near Limit**: When `remaining <= warning_threshold`, sends `MaxRoundsWarning`
3. **Pauses at Limit**: When reaching `max_rounds`, sends `AwaitingContinuation` and blocks waiting for user response
4. **Respects User Choice**:
   - `true` → Continue with more rounds
   - `false` or timeout → Stop gracefully with `Done` event

```rust
// At end of each round:
if round + 1 >= max_rounds {
    let _ = tx.send(StreamEvent::AwaitingContinuation);
    
    let decision = tokio::time::timeout(
        Duration::from_secs(300), // 5 minute timeout
        cont_rx.recv()
    ).await;
    
    match decision {
        Ok(Some(true)) => continue,  // User approved
        _ => {
            let _ = tx.send(StreamEvent::Done);
            return;
        }
    }
}
```

### 5. UI Handling

**Progress Display:**
```
⚙  read_file("src/main.rs") → 150 lines
⚠  Agent: 18 of 20 rounds complete (2 remaining)
⚙  edit_file("src/main.rs") → applied
```

**Continuation Prompt:**
```
⏸  Maximum rounds reached. Continue? (y/n)
```

**Keyboard Handling:**
- When `awaiting_continuation == true`:
  - `y` or `Y` → Call `approve_continuation()`, resume agent
  - `n` or `N` → Call `deny_continuation()`, stop agent
  - Other keys → Ignored

**Methods:**
```rust
impl AgentPanel {
    pub fn approve_continuation(&mut self) {
        // Send true via continuation_tx
        // Update UI to show "✓ Continuing..."
    }
    
    pub fn deny_continuation(&mut self) {
        // Send false via continuation_tx
        // Update UI to show "✗ Stopped by user"
    }
}
```

## Implementation Details

### Submit Method Signature

```rust
pub async fn submit(
    &mut self,
    context: Option<String>,
    project_root: PathBuf,
    max_rounds: usize,
    warning_threshold: usize,
) -> Result<()>
```

### Editor Integration

```rust
// In handle_agent_mode, KeyCode::Enter:
let max_rounds = self.config.max_agent_rounds;
let warning_threshold = self.config.agent_warning_threshold;
let fut = panel.submit(context, project_root, max_rounds, warning_threshold);
```

### Timeout Safety

The continuation prompt has a 5-minute timeout to prevent indefinite hangs if the user walks away. After timeout, the agent stops gracefully as if the user pressed 'n'.

## Consequences

### Positive
- **User Control**: Configurable limits via config file
- **No Lost Work**: Users can approve continuation instead of starting over
- **Better UX**: Clear progress indication and friendly prompts
- **Professional Feel**: Matches behavior of production AI assistants
- **Safety**: Prevents runaway loops while allowing flexibility

### Negative
- **Added Complexity**: ~150 lines of new code across multiple files
- **User Interruption**: Long-running tasks may pause for user input
- **Timeout Consideration**: Users must respond within 5 minutes or the agent stops

### Neutral
- Progress tracking adds minimal overhead (one channel send per round)
- Continuation prompt is blocking but with reasonable timeout
- Works seamlessly with existing retry and stream timeout mechanisms

## Alternatives Considered

### 1. Automatic Continuation
- **Rejected**: Safety concern - runaway loops could consume API quota
- Better to require explicit user approval

### 2. Shorter Timeout (1-2 minutes)
- **Rejected**: Too aggressive for users who may be reading the output or thinking
- 5 minutes balances safety with user convenience

### 3. No Progress Tracking
- **Rejected**: Users would have no visibility into long operations
- Progress indicators improve perceived responsiveness

### 4. Infinite Rounds with No Prompt
- **Rejected**: Would never prompt user, defeating the purpose
- Current design allows continuation indefinitely via repeated approvals

## Configuration Examples

**Conservative (for simple tasks):**
```toml
max_agent_rounds = 10
agent_warning_threshold = 2
```

**Permissive (for complex refactors):**
```toml
max_agent_rounds = 50
agent_warning_threshold = 5
```

**Default (balanced):**
```toml
max_agent_rounds = 20
agent_warning_threshold = 3
```

## Related
- ADR 0011: Agentic Tool-Calling Loop (base architecture)
- ADR 0012: Agent UX and File Refresh (user experience patterns)
- ADR 0026: Copilot Stream Resilience (timeout and error handling)

## Future Enhancements

- [ ] Show round progress in panel title: `Copilot Chat [gpt-4o] (Round 5/20)`
- [ ] Allow user to extend by specific number of rounds: `+10`, `+5`
- [ ] Track rounds per conversation and display in history
- [ ] Log round count and tool calls for debugging/analysis
- [ ] Add "Continue without asking" option (stores preference in session)

## Notes

This ADR addresses a key user experience gap where long-running agentic tasks would fail abruptly without giving users a chance to approve continuation. The implementation follows the pattern established by professional AI coding assistants while maintaining forgiven's focus on simplicity and configurability.
