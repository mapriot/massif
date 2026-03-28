use anyhow::{Context, Result};
use image_webp::{ColorType, WebPEncoder};

/// Encode a 512×512 RGB tile as WebP.
///
/// No `compress`: image-webp pure-Rust lossless — fastest path.
/// `compress` 1–9: libwebp lossless; maps 1–9 → effort 0–100.
///
/// Lossless encoding is mandatory for terrain-RGB — lossy modes corrupt
/// individual channel values, producing elevation errors of thousands of metres.
pub fn encode_tile(rgb: &[u8], compress: Option<u8>) -> Result<Vec<u8>> {
    if let Some(level) = compress {
        let effort = (level - 1) as f32 * 100.0 / 8.0; // 1→0.0, 5→50.0, 9→100.0
        Ok(webp::Encoder::from_rgb(rgb, 512, 512)
            .encode_simple(true, effort) // lossless=true, quality=effort
            .unwrap()
            .to_vec())
    } else {
        let mut buf = Vec::new();
        WebPEncoder::new(&mut buf)
            .encode(rgb, 512, 512, ColorType::Rgb8)
            .context("webp encode")?;
        Ok(buf)
    }
}
