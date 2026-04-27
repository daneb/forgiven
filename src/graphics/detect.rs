use std::io::IsTerminal as _;
use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWriteExt as _;

/// Inline image protocol supported by the running terminal emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageProtocol {
    /// Kitty terminal graphics protocol (most capable, lossless).
    Kitty,
    /// Sixel bitmap graphics (widely supported on xterm, mlterm, WezTerm).
    Sixel,
    /// iTerm2 inline image protocol (iTerm2, WezTerm in iTerm2-compat mode).
    Iterm2,
    /// Unicode half-block fallback (▄/▀) — always available in any terminal.
    HalfBlock,
    /// No inline image support detected (headless, dumb terminal, etc.).
    #[default]
    None,
}

/// Detect which inline image protocol the current terminal supports.
///
/// Runs at startup before the crossterm event loop begins. Each probe uses a
/// 200 ms timeout so detection never blocks startup in slow or unresponsive
/// terminals. On a non-tty stdout (CI, pipes) returns `None` immediately.
pub async fn detect_protocol() -> ImageProtocol {
    if !std::io::stdout().is_terminal() {
        return ImageProtocol::None;
    }

    // ── Kitty: send APC query, look for APC response ──────────────────────
    if probe_kitty().await {
        return ImageProtocol::Kitty;
    }

    // ── Sixel: send DA1, look for ';4;' in the device attributes response ─
    if probe_sixel().await {
        return ImageProtocol::Sixel;
    }

    // ── iTerm2 / WezTerm: env-var heuristic (no reliable escape query) ────
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        if term_program == "iTerm.app" || term_program == "WezTerm" {
            return ImageProtocol::Iterm2;
        }
    }

    // Half-block always works: Unicode characters, no special protocol needed.
    ImageProtocol::HalfBlock
}

/// Write the Kitty APC identification query and wait up to 200 ms for a
/// response that starts with the Kitty APC prefix `\x1b_G`.
async fn probe_kitty() -> bool {
    // Minimal Kitty query: action=query, image-id=31, 1×1 pixels, direct payload.
    const QUERY: &[u8] = b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\";
    let mut stdout = tokio::io::stdout();
    let mut stdin = tokio::io::stdin();

    if stdout.write_all(QUERY).await.is_err() {
        return false;
    }
    if stdout.flush().await.is_err() {
        return false;
    }

    let mut buf = [0u8; 32];
    match tokio::time::timeout(std::time::Duration::from_millis(200), stdin.read(&mut buf)).await {
        Ok(Ok(n)) if n >= 3 => {
            // Response begins with ESC _ G (0x1b 0x5f 0x47)
            buf[..n].windows(3).any(|w| w == b"\x1b_G")
        },
        _ => false,
    }
}

/// Send the VT100 DA1 query and check for sixel capability code 4 in the
/// response (`\x1b[?...;4;...c` or `\x1b[?...;4c`).
async fn probe_sixel() -> bool {
    const DA1: &[u8] = b"\x1b[c";
    let mut stdout = tokio::io::stdout();
    let mut stdin = tokio::io::stdin();

    if stdout.write_all(DA1).await.is_err() {
        return false;
    }
    if stdout.flush().await.is_err() {
        return false;
    }

    let mut buf = [0u8; 64];
    match tokio::time::timeout(std::time::Duration::from_millis(200), stdin.read(&mut buf)).await {
        Ok(Ok(n)) if n > 0 => {
            let s = String::from_utf8_lossy(&buf[..n]);
            // Sixel capability is indicated by parameter '4' in the list.
            s.contains(";4;") || s.contains(";4c") || s.ends_with("[?4c")
        },
        _ => false,
    }
}
