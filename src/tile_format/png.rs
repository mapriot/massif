use anyhow::{Context, Result};

/// Encode a 512×512 RGB tile as PNG.
///
/// `compress` 1–9 maps to png::Compression levels:
///   1–3 → Fast, 4–6 → Default, 7–9 → Best
/// Omitting `compress` uses no compression (prioritises speed, consistent with
/// the no-compress WebP path).
pub fn encode_tile(rgb: &[u8], compress: Option<u8>) -> Result<Vec<u8>> {
    let level = match compress {
        None => png::Compression::Fast,
        Some(1..=3) => png::Compression::Default,
        Some(4..=6) => png::Compression::Best,
        Some(_) => png::Compression::Best,
    };

    let mut buf = Vec::new();
    let mut encoder = png::Encoder::new(&mut buf, 512, 512);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(level);
    encoder
        .write_header()
        .context("png write header")?
        .write_image_data(rgb)
        .context("png write data")?;
    Ok(buf)
}
