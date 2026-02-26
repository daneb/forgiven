# ADR 0026: Copilot Stream Resilience

## Status
Accepted

## Context
The Copilot chat integration experienced reliability issues during streaming responses:

1. **Infinite Hangs**: When the API stalled without closing the connection, the editor would hang indefinitely waiting for data that never arrives.

2. **Incomplete Responses**: When network connections dropped mid-stream, buffered data that hadn't yet been parsed was lost, resulting in truncated responses shown to the user.

3. **Poor Error Recovery**: Stream errors immediately terminated processing without attempting to salvage any buffered data.

These issues created a poor user experience, especially during long agentic tool-calling sessions where the agent performs significant work but the user sees no feedback.

## Decision
Implement comprehensive stream resilience mechanisms in the SSE (Server-Sent Events) parsing logic:

### 1. Stream Timeout Protection
Add a configurable timeout wrapper around all stream read operations:
- Default timeout: 60 seconds of no data
- Gracefully exit with error notification when timeout occurs
- Prevents infinite hangs on stalled connections

### 2. Buffer Flush on Stream Completion
Process all remaining buffered data after the stream ends:
- Parse any complete SSE events left in the buffer
- Extract text content and tool call deltas
- Ensure users receive the full response even on connection drops

### 3. Enhanced Error Recovery
Attempt data recovery before terminating on errors:
- Process complete lines from the buffer when errors occur
- Extract any valid content before reporting failure
- Log warnings with context for debugging

## Implementation Details

### Stream Timeout
```rust
const STREAM_TIMEOUT_SECS: u64 = 60;

'sse: loop {
    let item = match tokio::time::timeout(
        tokio::time::Duration::from_secs(STREAM_TIMEOUT_SECS),
        byte_stream.next()
    ).await {
        Ok(Some(result)) => result,
        Ok(None) => break 'sse,
        Err(_) => {
            warn!("Stream timeout after {STREAM_TIMEOUT_SECS}s");
            let _ = tx.send(StreamEvent::Error("Stream stalled".to_string()));
            break 'sse;
        }
    };
    // Process item...
}
```

### Buffer Flush Logic
After the SSE loop exits, process remaining data:
```rust
if !sse_buf.is_empty() {
    debug!("Processing {} bytes of remaining buffer data", sse_buf.len());
    for line in sse_buf.lines() {
        // Parse SSE events, extract content and tool calls
    }
}
```

### Error Recovery
On stream errors, salvage buffered data:
```rust
Err(e) => {
    warn!("Stream error, attempting to process buffered data: {e}");
    // Process complete lines from buffer
    while let Some(pos) = sse_buf.find('\n') {
        // Parse and extract content
    }
    let _ = tx.send(StreamEvent::Error(format!("{e}")));
    return;
}
```

## Consequences

### Positive
- **No More Infinite Hangs**: 60-second timeout prevents indefinite waiting
- **Complete Responses**: Buffer flush ensures all data reaches the user
- **Better UX**: Users see partial results even on connection failures
- **Improved Debugging**: Enhanced logging provides visibility into stream issues
- **Graceful Degradation**: System fails predictably with useful error messages

### Negative
- **Slight Complexity**: Additional ~80 lines of stream handling logic
- **Fixed Timeout**: 60-second timeout may need tuning for very slow responses
- **Memory Usage**: Buffering data for post-stream processing (minimal impact)

### Neutral
- Timeout value is configurable via `STREAM_TIMEOUT_SECS` constant
- Existing retry logic (5 attempts with exponential backoff) complements these changes
- Token expiration checking (already implemented) works alongside stream reliability

## Alternatives Considered

### 1. Shorter Timeout (30 seconds)
- **Rejected**: Too aggressive for large tool execution or slow API responses
- May terminate legitimate long-running operations

### 2. No Buffer Flush
- **Rejected**: Loses data on early stream termination
- Poor UX when users lose the end of responses

### 3. Retry on Timeout
- **Deferred**: Would require careful state management to avoid duplicate work
- Better addressed at higher level (user can retry entire conversation)

### 4. Heartbeat/Keepalive
- **Not Applicable**: SSE protocol doesn't provide client-side keepalive
- Server-side concern, not controllable from client

## Related
- ADR 0004: Copilot Authentication (token management)
- ADR 0011: Agentic Tool-Calling Loop (context for long-running operations)
- ADR 0012: Agent UX and File Refresh (user experience during agent work)

## Notes
This ADR addresses production issues observed during development and testing:
- Streams that hung indefinitely when network connections stalled
- Responses that appeared truncated when connections dropped early
- Loss of agent work when errors occurred mid-stream

The 60-second timeout is conservative and can be adjusted based on real-world usage patterns.
