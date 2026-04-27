// Copies marked.js from node_modules into ui/vendor/ so the webview can
// load it without a bundler. Run automatically via `npm postinstall`.
import { copyFileSync, mkdirSync } from 'fs';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = resolve(__dirname, '..');

const vendor = resolve(root, 'ui', 'vendor');
mkdirSync(vendor, { recursive: true });

const markedSrc = resolve(root, 'node_modules', 'marked', 'src', 'marked.js');
const markedEsm = resolve(root, 'node_modules', 'marked', 'marked.min.js');

// Try the minified UMD build first (sets window.marked), then fall back.
const src = (() => {
  try { copyFileSync(markedEsm, resolve(vendor, 'marked.min.js')); return 'marked.min.js'; }
  catch { copyFileSync(markedSrc, resolve(vendor, 'marked.js')); return 'marked.js'; }
})();

console.log(`[vendor] copied marked → ui/vendor/${src}`);
