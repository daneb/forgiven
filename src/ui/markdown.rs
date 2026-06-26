use super::*;

/// Render markdown content as Ratatui lines.
/// (Agent panel and thinking-block rendering removed in slim build.)
#[allow(dead_code)]
pub(super) fn render_message_content(
    content: &str,
    width: usize,
    hl: &crate::highlight::Highlighter,
) -> Vec<Line<'static>> {
    crate::markdown::render(content, width, Some(hl))
}
