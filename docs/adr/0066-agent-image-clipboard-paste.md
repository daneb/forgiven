# ADR 0066 — Agent Panel Image Clipboard Paste

**Date:** 2026-03-17
**Status:** Accepted

---

## Context

Terminal applications cannot paste images via bracketed paste — terminals only transmit text clipboard content through the `Event::Paste` mechanism. However, AI models like GPT-4o and Claude support vision inputs via the OpenAI-compatible content-array format. Users working with the agent panel may want to paste screenshots (e.g. UI mockups, error dialogs, terminal output) to provide visual context.

Claude Code solved this by reading the clipboard directly via native OS APIs, encoding the image to base64, and sending it as a vision content block. We follow the same approach.

## Decision

### Keybinding

**Ctrl+V** in Agent mode reads the system clipboard via `arboard`:
1. Try `arboard::Clipboard::get_image()` first.
2. If an image is found: encode RGBA → PNG → base64, store as `ClipboardImage`.
3. If no image: fall back to `arboard::Clipboard::get_text()`, route through `handle_paste()`.
4. On failure: show error in status bar.

On macOS, Cmd+V continues to trigger the terminal's bracketed paste (text only via `Event::Paste`). Ctrl+V is passed to the application as a `KeyEvent`. On Linux, Ctrl+Shift+V is the terminal paste shortcut, so Ctrl+V is similarly available.

### Data Model

```rust
pub struct ClipboardImage {
    pub width: u32,
    pub height: u32,
    pub data_uri: String,  // "data:image/png;base64,..."
}
```

- `AgentPanel.image_blocks: Vec<ClipboardImage>` — pending images, cleared on `submit()`.
- `ChatMessage.images: Vec<(u32, u32)>` — dimension-only placeholders in history. Base64 data is NOT stored in message history to avoid unbounded memory growth. Images are ephemeral for the current submission turn.

### API Format

When images are present, `submit()` uses the OpenAI content-array format:

```json
{
  "role": "user",
  "content": [
    { "type": "text", "text": "What's in this screenshot?" },
    { "type": "image_url", "image_url": { "url": "data:image/png;base64,...", "detail": "auto" } }
  ]
}
```

When no images are attached, the existing plain string format is used (no change to existing behavior).

### UI

- **Input area**: Magenta+DIM badge: `"  Image (320x240)"`
- **Message history**: Magenta+DIM placeholder: `"  Image (320x240) [attached]"`
- **Hint text**: Updated to include `Ctrl+V=paste image`
- **No inline image rendering** — the terminal shows text placeholders only. Future enhancement could use `ratatui-image` for Kitty/Sixel/iTerm2 protocol thumbnails.

### Size Guard

Images larger than 20 MB (base64-encoded) are rejected with an error message. A 4K screenshot is typically ~8-11 MB encoded, so this limit is generous.

### Dependencies

- `image = { version = "0.25", default-features = false, features = ["png"] }` — RGBA→PNG encoding.
- `base64 = "0.22"` — PNG→base64 encoding (already a transitive dep via reqwest).

## Consequences

- Users can paste screenshots into the agent panel for vision-capable models.
- Models without vision support will return an API error (surfaced via existing error handling).
- Historical messages with images replay as text-only (no re-sending of image data on subsequent turns).
- No new terminal capability requirements — works in all terminals.
