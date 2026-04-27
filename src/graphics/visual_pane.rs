use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use super::ImageProtocol;

/// Placeholder widget that will host inline image rendering (Phase 2 — Glimpse).
///
/// Phase 1: renders a bordered block showing the detected protocol name so the
/// layout slot exists and is exercised by the render loop.
/// Phase 2: will hold a `ratatui_image::StatefulImage` and call its
/// `StatefulWidget::render()` instead.
#[allow(dead_code)]
pub struct VisualPane {
    pub protocol: ImageProtocol,
    pub label: String,
}

impl VisualPane {
    #[allow(dead_code)]
    pub fn new(protocol: ImageProtocol, label: impl Into<String>) -> Self {
        Self { protocol, label: label.into() }
    }
}

impl Widget for VisualPane {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let protocol_str = match self.protocol {
            ImageProtocol::Kitty => "Kitty",
            ImageProtocol::Sixel => "Sixel",
            ImageProtocol::Iterm2 => "iTerm2",
            ImageProtocol::HalfBlock => "Half-block",
            ImageProtocol::None => "None",
        };
        let title = format!(" {} [{}] ", self.label, protocol_str);
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(area);
        block.render(area, buf);

        let hint = Line::from(vec![Span::styled(
            "inline image rendering — Phase 2",
            Style::default().fg(Color::DarkGray),
        )]);
        Paragraph::new(hint).render(inner, buf);
    }
}
