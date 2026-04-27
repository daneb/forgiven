// Tauri v2 API — imported from the injected global provided by the webview context.
// In Tauri v2 the API is available as window.__TAURI__ when the app is running.
// For dev/preview outside Tauri, we stub the listen() function gracefully.

const tauriAvailable = typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__;

function dbg(msg) {
  const el = document.getElementById('placeholder');
  if (el) el.innerHTML += `<p class="hint" style="font-size:10px;opacity:0.6">${msg}</p>`;
  console.log('[nexus]', msg);
}

function listen(event, handler) {
  if (!tauriAvailable) { dbg('no tauri'); return Promise.resolve(); }
  const api = window.__TAURI__?.event;
  if (!api) { dbg('no __TAURI__.event'); return Promise.resolve(); }
  return api.listen(event, handler).catch(e => dbg(`listen(${event}) err: ${e}`));
}

// ── DOM references ─────────────────────────────────────────────────────────

const contentEl  = document.getElementById('content');
const filePathEl = document.getElementById('file-path');
const modeBadge  = document.getElementById('mode-badge');
const cursorPos  = document.getElementById('cursor-pos');

let currentFilePath = null;

// ── Mermaid initialisation ─────────────────────────────────────────────────

if (typeof window.mermaid !== 'undefined') {
  window.mermaid.initialize({
    startOnLoad: false,
    theme: 'dark',
    themeVariables: {
      background: '#2b303b',
      primaryColor: '#4f5b66',
      primaryTextColor: '#c0c5ce',
      lineColor: '#65737e',
      edgeLabelBackground: '#343d46',
    },
  });
}

// ── Markdown rendering ─────────────────────────────────────────────────────

// marked.js is loaded from vendor/ (see package.json postinstall script).
// Falls back gracefully to plain text pre-wrap if marked is not available.
function renderMarkdown(text) {
  if (typeof window.marked !== 'undefined') {
    return window.marked.parse(text, { gfm: true, breaks: false });
  }
  // Fallback: plain text wrapped in a pre block.
  const pre = document.createElement('pre');
  pre.textContent = text;
  return pre.outerHTML;
}

async function setContent(html, filePath) {
  contentEl.innerHTML = html;

  // marked renders ```mermaid as <pre><code class="language-mermaid">.
  // Swap those out for <pre class="mermaid"> so mermaid.run() can find them.
  if (typeof window.mermaid !== 'undefined') {
    contentEl.querySelectorAll('pre > code.language-mermaid').forEach((code) => {
      const pre = document.createElement('pre');
      pre.className = 'mermaid';
      pre.textContent = code.textContent;
      code.parentElement.replaceWith(pre);
    });
    await window.mermaid.run().catch(() => {});
  }

  // Syntax-highlight remaining <pre><code> blocks if highlight.js is available.
  if (typeof window.hljs !== 'undefined') {
    contentEl.querySelectorAll('pre code').forEach((block) => {
      window.hljs.highlightElement(block);
    });
  }

  // Rewrite local image src attributes to asset:// URLs so the webview can
  // serve them from disk.  Remote URLs and data: URIs are left untouched.
  rewriteLocalImages(filePath);
}

// ── Local image rewriting ──────────────────────────────────────────────────

// Convert a file path to a Tauri asset:// URL using the platform helper when
// available, falling back to a direct construction for dev/test mode.
function toAssetUrl(absPath) {
  if (tauriAvailable && window.__TAURI__?.core?.convertFileSrc) {
    return window.__TAURI__.core.convertFileSrc(absPath);
  }
  return `asset://localhost${absPath}`;
}

function rewriteLocalImages(filePath) {
  const dir = filePath ? filePath.replace(/[^\\/]+$/, '') : null;

  contentEl.querySelectorAll('img[src]').forEach((img) => {
    const src = img.getAttribute('src');
    if (!src) return;
    // Leave remote URLs and already-converted URIs alone.
    if (/^(https?|data|asset|blob):/.test(src)) return;

    let abs;
    if (src.startsWith('/')) {
      abs = src;
    } else if (dir) {
      abs = dir + src;
    } else {
      return; // relative path but no base dir — can't resolve
    }
    img.src = toAssetUrl(abs);
  });
}

// ── Nexus event handlers ───────────────────────────────────────────────────

async function onUpdate({ payload }) {
  const { content, content_type, file_path, cursor_line } = payload;

  currentFilePath = file_path ?? null;
  filePathEl.textContent = file_path ?? '—';
  if (cursor_line != null) cursorPos.textContent = `L${cursor_line + 1}`;

  if (content_type === 'markdown' || file_path?.endsWith('.md')) {
    await setContent(renderMarkdown(content), file_path);
  } else {
    // Non-markdown: render as a syntax-highlighted code block.
    const lang = content_type ?? 'text';
    const escaped = content.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
    await setContent(`<pre><code class="language-${lang}">${escaped}</code></pre>`, file_path);
  }
}

function onCursor({ payload }) {
  const { cursor_line, file_path } = payload;
  if (file_path) filePathEl.textContent = file_path;
  if (cursor_line != null) cursorPos.textContent = `L${cursor_line + 1}`;
}

function onMode({ payload: mode }) {
  document.body.dataset.mode = mode;
  modeBadge.textContent = mode;
}

// ── Local file navigation ──────────────────────────────────────────────────

// Load a local file by absolute path into the companion via the asset:// protocol.
// Falls back to opener.openPath() if the fetch fails (e.g. binary or permission error).
async function loadLocalFile(absPath) {
  try {
    const text = await window.__TAURI__.core.invoke('read_text_file', { path: absPath });
    filePathEl.textContent = absPath;
    currentFilePath = absPath;
    await setContent(renderMarkdown(text), absPath);
  } catch (err) {
    dbg(`loadLocalFile failed: ${err}`);
    if (tauriAvailable && window.__TAURI__?.opener) {
      window.__TAURI__.opener.openPath(absPath).catch(() => {});
    }
  }
}

// ── Bootstrap ──────────────────────────────────────────────────────────────

// Intercept all link clicks.
// - http/https → open in system browser
// - local file paths → load in companion
// - #fragment → pass through for same-page anchors
document.addEventListener('click', (e) => {
  const a = e.target.closest('a[href]');
  if (!a) return;
  const href = a.getAttribute('href');
  if (!href || href.startsWith('#')) return;
  e.preventDefault();
  if (!tauriAvailable) return;

  if (/^https?:\/\//.test(href)) {
    window.__TAURI__?.opener?.openUrl(href).catch(() => {});
    return;
  }

  // Local file link — resolve relative path against current document's directory.
  let abs = href;
  if (!href.startsWith('/') && currentFilePath) {
    const dir = currentFilePath.replace(/[^\\/]+$/, '');
    abs = dir + href;
  }
  loadLocalFile(abs);
}, false);

(async () => {
  dbg(`tauri=${tauriAvailable} __TAURI__=${!!window.__TAURI__} event=${!!window.__TAURI__?.event}`);
  await listen('nexus-update', onUpdate);
  await listen('nexus-cursor', onCursor);
  await listen('nexus-mode',   onMode);
  await listen('nexus-status', ({ payload }) => dbg(`status: ${payload}`));
  if (tauriAvailable) dbg('listeners registered');
})();

// ── Dev preview (outside Tauri): load a demo payload ──────────────────────
if (!tauriAvailable) {
  setTimeout(() => {
    onUpdate({
      payload: {
        content: '# Forgiven Previewer\n\nConnect the Forgiven TUI to see live preview here.\n\n```rust\nfn main() {\n    println!("Hello, Forgiven!");\n}\n```\n',
        content_type: 'markdown',
        file_path: 'demo.md',
        cursor_line: 0,
      }
    });
  }, 300);
}
