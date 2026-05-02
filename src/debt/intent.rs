//! ADR directory analysis → [`IntentDebt`].

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::IntentDebt;

const STALE_THRESHOLD_SECS: u64 = 18 * 30 * 24 * 3600; // ~18 months
const VELOCITY_WINDOW_SECS: u64 = 6 * 30 * 24 * 3600; // ~6 months

pub fn analyse(adr_dir: &Path) -> IntentDebt {
    let mut debt = IntentDebt::default();

    let Ok(entries) = std::fs::read_dir(adr_dir) else {
        return debt;
    };

    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

    let mut md_files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    md_files.sort();

    for path in &md_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        debt.total_adrs += 1;

        let status = extract_status(&content);
        let is_implemented = status.as_deref().map(|s| s.contains("Implemented")).unwrap_or(false);
        let is_accepted = status.as_deref().map(|s| s.contains("Accepted")).unwrap_or(false);
        let is_proposed = status.as_deref().map(|s| s.contains("Proposed")).unwrap_or(false);
        let is_superseded = status.as_deref().map(|s| s.contains("Superseded")).unwrap_or(false);

        if is_implemented {
            debt.implemented += 1;
        } else if is_accepted {
            debt.accepted_pending += 1;
        } else if is_proposed {
            debt.proposed += 1;
        } else if is_superseded {
            debt.superseded += 1;
        }

        // Staleness and velocity via filesystem mtime.
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                let mtime = modified.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let age_secs = now.saturating_sub(mtime);

                if age_secs < VELOCITY_WINDOW_SECS {
                    debt.recent_velocity += 1;
                }

                let is_active = is_accepted || is_proposed;
                if is_active && age_secs > STALE_THRESHOLD_SECS {
                    debt.stale_count += 1;
                }
            }
        }
    }

    debt
}

/// Extract the value of the `**Status:**` line from an ADR document.
fn extract_status(content: &str) -> Option<String> {
    content.lines().find(|l| l.trim().starts_with("**Status:**")).map(|l| l.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_accepted_status() {
        let content = "# ADR 0001\n\n**Status:** Accepted\n\n## Context\n";
        assert_eq!(extract_status(content).as_deref(), Some("**Status:** Accepted"));
    }

    #[test]
    fn parses_implemented_status() {
        let content = "# ADR 0042\n\n**Status:** Accepted — Implemented\n";
        let s = extract_status(content).unwrap_or_default();
        assert!(s.contains("Implemented"));
    }
}
