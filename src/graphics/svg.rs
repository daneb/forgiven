/// Convert SVG bytes to a PNG byte vector using resvg + tiny-skia.
///
/// Phase 2 implementation: used by the Mermaid-to-Sixel pipeline to convert
/// diagram SVG output into a PNG before encoding for terminal display.
#[allow(dead_code)]
pub fn svg_to_png(svg_bytes: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    let _ = (svg_bytes, width, height);
    todo!("Phase 2: resvg SVG-to-PNG conversion")
}
