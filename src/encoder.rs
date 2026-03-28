#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Encoding {
    /// Mapbox terrain-RGB: height = base_val + (R·65536 + G·256 + B) · interval
    Mapbox,
    /// Terrarium (Mapzen): height = (R·256 + G + B/256) − 32768
    Terrarium,
}

/// Encode an elevation value to Mapbox terrain-RGB.
///
/// Matches rio-rgbify:  encoded = ⌊(elev − base_val) / interval⌋  (zeros lower bits)
/// MapLibre decodes:    height  = base_val + (R·65536 + G·256 + B) · interval
#[inline(always)]
pub fn encode_mapbox(elev: f32, base_val: f64, interval: f64, round: u32, nodata: f32) -> [u8; 3] {
    if (elev - nodata).abs() < 0.5 || elev.is_nan() {
        return [0, 0, 0];
    }
    let encoded = ((elev as f64 - base_val) / interval).floor() as i64;
    let mask = !((1i64 << round) - 1);
    let enc = (encoded & mask).clamp(0, 0xFF_FFFF) as u32;
    [((enc >> 16) & 0xFF) as u8, ((enc >> 8) & 0xFF) as u8, (enc & 0xFF) as u8]
}

/// Encode an elevation value to Terrarium terrain-RGB.
///
/// Encodable range: −32768 m to +32767.996 m at ~0.004 m precision.
/// MapLibre decodes: height = (R·256 + G + B/256) − 32768
#[inline(always)]
pub fn encode_terrarium(elev: f32, nodata: f32) -> [u8; 3] {
    if (elev - nodata).abs() < 0.5 || elev.is_nan() {
        return [0, 0, 0];
    }
    let val = (elev as f64 + 32768.0).clamp(0.0, 65535.996);
    let r = (val / 256.0).floor() as u8;
    let g = (val.floor() as u32 % 256) as u8;
    let b = ((val.fract()) * 256.0).floor() as u8;
    [r, g, b]
}
