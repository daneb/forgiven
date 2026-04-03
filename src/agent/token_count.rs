//! Token counting via tiktoken-rs (cl100k_base — the GPT-4 / Copilot tokeniser).
//!
//! The BPE is initialised once on first use and reused for the process lifetime.
//! Falls back to the `len / 4` approximation if initialisation fails, so token
//! counts always succeed even if the BPE data is somehow unavailable.

use std::sync::OnceLock;

static BPE: OnceLock<Option<tiktoken_rs::CoreBPE>> = OnceLock::new();

fn bpe() -> Option<&'static tiktoken_rs::CoreBPE> {
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
}

/// Count tokens in `text` using cl100k_base.
/// Falls back to `text.len() / 4` if the BPE fails to initialise.
pub fn count(text: &str) -> u32 {
    match bpe() {
        Some(bpe) => bpe.encode_with_special_tokens(text).len() as u32,
        None => (text.len() / 4) as u32,
    }
}
