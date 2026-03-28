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
