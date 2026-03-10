# Constitution

## Vision
Build a fast, secure, keyboard-first terminal editor (Forgiven) that enables users to read, navigate, and change text with minimal friction.

## Principles
1. **Editor-first, low-friction UX** — core workflows (navigate, edit, search, act) should require minimal context switching and keystrokes.
2. **Deterministic, testable state** — editor behaviors are driven by explicit state and pure-ish transitions wherever practical; critical flows are unit-testable.
3. **Non-blocking UI** — rendering and input handling must remain responsive; slow work (I/O, network, indexing) stays off the UI thread/event loop.
4. **Minimal surface area** — ship small, composable primitives (buffers, views, actions, widgets) rather than sprawling frameworks.
5. **Simple by default** — prefer straightforward, predictable behavior over feature density; complexity must pay for itself.
6. **Configurable within existing patterns** — customization uses Forgiven’s established config/keymap conventions with sensible defaults.
7. **Keyboard-accessible & clear focus** — all features must be operable without a mouse, with obvious focus/selection states and consistent theming.
8. **High cohesion, loose coupling** — modules own clear responsibilities; integrations happen through narrow interfaces to keep components independently evolvable.
9. **Security-minded by design** — treat all external content as untrusted; minimize attack surface and make privacy-preserving defaults the norm.

## Constraints
- **Language/Runtime**: Rust.
- **Supported platforms**: macOS, Windows, and Linux (main focus: macOS).
- **Dependency policy**: Prefer existing crates already used in the repo; only add small, well-maintained dependencies with clear justification.
- **Security & privacy**:
  - No logging or persistence of sensitive user content by default (e.g., file contents, AI prompts, secrets).
  - Sanitize/escape all untrusted text rendered in the terminal UI; no unsafe terminal escape injection.
  - Keep integration boundaries narrow (explicit inputs/outputs) to reduce attack surface.
- **Performance**:
  - No noticeable input latency; avoid allocations in render hot paths.
  - Keep background work cancellable and incremental (e.g., indexing/search) where possible.
  - Leverage Rust for memory safety, performance, and concurrency without data races.
- **Compatibility**:
  - Must integrate with the existing event loop, keymap system, and UI rendering patterns.
  - Cross-platform behavior should be consistent unless a platform difference is explicitly required.

## Non-goals
- Becoming a full IDE replacement with heavy project systems by default.
- Adding complex GUI-only workflows that depend on a mouse.
- Introducing telemetry/analytics that capture sensitive user content.
- Building a general-purpose UI framework unrelated to editor needs.

## Success criteria
- Users can perform core editing workflows (open, navigate, edit, search, save) quickly and entirely via keyboard.
- The editor remains responsive under load (large files, background tasks, integrations) with no UI-thread blocking.
- Configuration and keymaps are discoverable, stable, and backward-compatible within reason.
- Sensitive content is protected by default: not logged, not persisted unintentionally, and rendered safely.
- The architecture remains modular: features can be added without tightly coupling subsystems.
