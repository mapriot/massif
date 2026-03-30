/// Half circumference at equator in metres (π × WGS84 R)
pub const HALF_CIRC: f64 = 20_037_508.342_789_244;

/// Convert EPSG:3857 (Web Mercator) metres to WGS84 (longitude, latitude) in degrees.
#[inline(always)]
pub fn merc_to_wgs84(x_m: f64, y_m: f64) -> (f64, f64) {
    let lon = x_m / HALF_CIRC * 180.0;
    let lat = (2.0 * (y_m * std::f64::consts::PI / HALF_CIRC).exp().atan()
        - std::f64::consts::FRAC_PI_2)
        .to_degrees();
    (lon, lat)
}

/// Return [west, south, east, north] in EPSG:3857 metres for tile (z, x, y_xyz).
/// XYZ convention: y=0 is at the north.
pub fn tile_bounds_3857(z: u8, x: u32, y_xyz: u32) -> [f64; 4] {
    let n = (1u64 << z) as f64;
    let size = 2.0 * HALF_CIRC / n;
    let west = -HALF_CIRC + x as f64 * size;
    let east = west + size;
    let north = HALF_CIRC - y_xyz as f64 * size;
    let south = north - size;
    [west, south, east, north]
}

/// WGS84 longitude → tile column X at zoom z.
pub fn lon_to_tile_x(lon: f64, z: u8) -> u32 {
    let n = (1u64 << z) as f64;
    let x = ((lon + 180.0) / 360.0 * n).floor() as i64;
    x.clamp(0, n as i64 - 1) as u32
}

/// WGS84 latitude → tile row Y (XYZ, y=0 at north) at zoom z.
pub fn lat_to_tile_y_xyz(lat: f64, z: u8) -> u32 {
    let lat = lat.clamp(-85.051_129, 85.051_129);
    let n = (1u64 << z) as f64;
    let lat_r = lat.to_radians();
    let y = ((1.0 - (lat_r.tan() + 1.0 / lat_r.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor() as i64;
    y.clamp(0, n as i64 - 1) as u32
}
