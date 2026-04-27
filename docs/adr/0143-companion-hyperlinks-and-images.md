# ADR 0143 — Companion: Hyperlink and Image Support

**Date:** 2026-04-26
**Status:** Implemented

---

## Context

The Tauri companion window renders Markdown via `marked.js` inside a webview.
Two content elements that appear naturally in Markdown were previously broken or
absent:

**Hyperlinks** — `marked.js` converts `[text](url)` to `<a href="url">` correctly,
but Tauri's webview intercepted click events and attempted to navigate the webview
itself to the URL, replacing the companion UI with the linked page.  Users had no
way to follow links from their notes or documentation.

**Images** — The CSP was locked to `default-src 'self'`, blocking all image
loads.  Remote images (`![alt](https://…)`) returned CSP errors.  Local project
images (`![alt](./diagram.png)`) had no path-to-URL mapping and could not be
served from disk to the webview at all.

---

## Decision

### 1. Hyperlinks — `tauri-plugin-opener`

Add `tauri-plugin-opener = "2"` to `companion/src-tauri/Cargo.toml` and register
it in `lib.rs` with `.plugin(tauri_plugin_opener::init())`.

A single `click` event listener is installed on `document` during bootstrap
in `main.js`.  It intercepts every `<a href>` click, cancels the default
navigation, and delegates to `window.__TAURI__.opener.openUrl(href)`, which
hands the URL to the OS default browser.  Same-page fragment anchors (`#…`)
are allowed through unchanged.

Grant `"opener:allow-open-url"` in `capabilities/default.json`.

### 2. Remote images — CSP relaxation

Extend the `img-src` directive in `tauri.conf.json`:

```
img-src * data: asset: blob:
```

`*` permits any `http://` / `https://` origin.  `data:` permits inline base64
images.  `asset:` and `blob:` are needed for local file serving (below) and
future canvas/SVG exports.  `script-src` and `style-src` are unchanged.

### 3. Local project images — `asset://` protocol

Tauri's built-in `assetProtocol` serves arbitrary local files under the
`asset://localhost/…` scheme.  Enable it in `tauri.conf.json`:

```json
"assetProtocol": { "enable": true, "scope": ["**"] }
```

`scope: ["**"]` grants access to all paths.  This is acceptable for a local
developer tool where the user controls which project is opened; a future
hardening pass could scope it to `["$HOME/**"]`.

After each content update, `rewriteLocalImages(filePath)` walks the rendered
`<img>` elements.  For any `src` that is not already a remote URL, `data:` URI,
or `asset:` URL it constructs an absolute path (resolving relative paths against
the directory of the currently open file) and rewrites `img.src` to an
`asset://` URL.  Conversion uses `window.__TAURI__.core.convertFileSrc()` when
available, which handles platform path differences correctly, with a direct
`asset://localhost${absPath}` fallback for dev/test mode outside Tauri.

---

## Alternatives considered

**`tauri-plugin-shell` instead of `tauri-plugin-opener`** — `shell` was the
Tauri v1 approach; `opener` is the canonical v2 replacement and has a narrower
permission surface (`allow-open-url` vs the broader shell execution permissions).

**Base64-encode images on the TUI side** — The Rust editor could detect image
references, read and base64-encode them, and embed them as `data:` URIs in the
`buffer_update` payload before sending over Nexus.  Rejected: this bloats the
IPC payload for every buffer update that contains images, adds Rust-side
complexity, and requires the TUI to understand image MIME types.  The
`asset://` approach keeps the pipeline clean — the webview fetches files on
demand, only when they are actually visible.

**`img-src 'self'` + proxy route** — Serve remote images through a local Rust
HTTP proxy.  Rejected: unnecessary complexity; the webview's native networking
is adequate for a developer tool without privacy constraints.

---

## Consequences

- Clicking any link in the companion opens the system browser — the companion
  UI is never navigated away.
- Remote images in Markdown render without CSP errors.
- Local project images (relative and absolute paths) resolve correctly relative
  to the file being previewed and render via the `asset://` protocol.
- `tauri-plugin-opener` is added as a new Cargo dependency in the companion
  crate only — the main `forgiven` TUI is unaffected.
- The `assetProtocol` scope is `"**"` (all paths). This is appropriate for a
  local tool but should be revisited if the companion is ever distributed more
  broadly.
