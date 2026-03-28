use std::path::Path;

use anyhow::{Context, Result};

use crate::encoder::encode_mapbox;
use crate::tile_format::Format;
use crate::tile::tile_bounds_3857;

/// Bilinear sample from a flat f32 buffer. Returns `nodata` if out of bounds.
pub fn sample_bilinear(
    data: &[f32],
    width: usize,
    height: usize,
    px: f64,
    py: f64,
    nodata: f32,
) -> f32 {
    if px < 0.0 || py < 0.0 || px >= width as f64 || py >= height as f64 {
        return nodata;
    }
    let x0 = px.floor() as usize;
    let y0 = py.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = px - x0 as f64;
    let fy = py - y0 as f64;

    let v = [
        data[y0 * width + x0],
        data[y0 * width + x1],
        data[y1 * width + x0],
        data[y1 * width + x1],
    ];

    let is_nd = |v: f32| (v - nodata).abs() < 0.5 || v.is_nan();
    if v.iter().any(|&s| is_nd(s)) {
        // Nearest-neighbour fallback
        let nx = if fx < 0.5 { x0 } else { x1 };
        let ny = if fy < 0.5 { y0 } else { y1 };
        return data[ny * width + nx];
    }

    (v[0] as f64 * (1.0 - fx) * (1.0 - fy)
        + v[1] as f64 * fx * (1.0 - fy)
        + v[2] as f64 * (1.0 - fx) * fy
        + v[3] as f64 * fx * fy) as f32
}

/// Read the WGS84 bounding box of a GDAL dataset.
/// Returns (west_lon, south_lat, east_lon, north_lat).
pub fn dataset_wgs84_bounds(path: &Path) -> Result<(f64, f64, f64, f64)> {
    use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
    use gdal::Dataset;

    let ds = Dataset::open(path).context("open dataset")?;
    let gt = ds.geo_transform().context("geo_transform")?;
    let (w, h) = ds.raster_size();

    let ox = gt[0];
    let oy = gt[3];
    let ex = ox + gt[1] * w as f64;
    let ey = oy + gt[5] * h as f64;

    let mut xs = [ox, ex, ox, ex];
    let mut ys = [oy, oy, ey, ey];

    let mut src_srs = SpatialRef::from_wkt(&ds.projection()).context("source SRS for bounds")?;
    src_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let mut wgs84 = SpatialRef::from_epsg(4326).context("EPSG:4326")?;
    wgs84.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    let to_wgs84 = CoordTransform::new(&src_srs, &wgs84).context("coord transform")?;
    to_wgs84
        .transform_coords(&mut xs, &mut ys, &mut [])
        .context("transform corners to WGS84")?;

    let west = xs.iter().cloned().fold(f64::MAX, f64::min);
    let east = xs.iter().cloned().fold(f64::MIN, f64::max);
    let south = ys.iter().cloned().fold(f64::MAX, f64::min);
    let north = ys.iter().cloned().fold(f64::MIN, f64::max);

    eprintln!(
        "Input: {}×{} px  WGS84 bounds [{:.4}W {:.4}S {:.4}E {:.4}N]",
        w, h, west, south, east, north
    );
    Ok((west, south, east, north))
}

/// Process one tile; returns `None` if entirely nodata.
/// Opens its own GDAL handle — required because GDAL datasets are not Send.
pub fn process_tile(
    input_path: &str,
    z: u8,
    x: u32,
    y_xyz: u32,
    base_val: f64,
    interval: f64,
    round: u32,
    format: Format,
    compress: Option<u8>,
) -> Result<Option<Vec<u8>>> {
    use gdal::raster::ResampleAlg;
    use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
    use gdal::Dataset;

    let dataset = Dataset::open(input_path).context("open dataset")?;
    let gt = dataset.geo_transform().context("geo_transform")?;
    let (src_w, src_h) = dataset.raster_size();
    let band = dataset.rasterband(1).context("rasterband 1")?;
    let nodata = band.no_data_value().unwrap_or(-32_767.0) as f32;

    // ── Coordinate transform: EPSG:3857 → source SRS ──────────────────────────
    let mut srs_3857 = SpatialRef::from_epsg(3857).context("EPSG:3857")?;
    srs_3857.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    let mut src_srs = SpatialRef::from_wkt(&dataset.projection()).context("source SRS")?;
    src_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    let to_src = CoordTransform::new(&srs_3857, &src_srs).context("coord transform 3857→src")?;

    // ── Tile extent in 3857 ───────────────────────────────────────────────────
    let [west_m, south_m, east_m, north_m] = tile_bounds_3857(z, x, y_xyz);

    // ── Transform tile corners + midpoints to source SRS for read window ──────
    let mid_x = (west_m + east_m) / 2.0;
    let mid_y = (south_m + north_m) / 2.0;
    let mut cx = [west_m, east_m, west_m, east_m, mid_x, west_m, east_m, mid_x];
    let mut cy = [south_m, south_m, north_m, north_m, mid_y, mid_y, mid_y, south_m];
    to_src
        .transform_coords(&mut cx, &mut cy, &mut [])
        .context("transform corners")?;

    let src_x_min = cx.iter().cloned().fold(f64::MAX, f64::min);
    let src_x_max = cx.iter().cloned().fold(f64::MIN, f64::max);
    let src_y_min = cy.iter().cloned().fold(f64::MAX, f64::min);
    let src_y_max = cy.iter().cloned().fold(f64::MIN, f64::max);

    // ── Convert source-SRS bbox to pixel indices ──────────────────────────────
    // gt: [origin_x, px_width, 0, origin_y, 0, px_height(negative)]
    let px_min = (src_x_min - gt[0]) / gt[1];
    let px_max = (src_x_max - gt[0]) / gt[1];
    // gt[5] < 0, so larger src_y → smaller py
    let py_min = (src_y_max - gt[3]) / gt[5];
    let py_max = (src_y_min - gt[3]) / gt[5];

    // Expand 1 px for bilinear border; clamp to source bounds
    let rx0 = (px_min.floor() as i64 - 1).clamp(0, src_w as i64 - 1) as usize;
    let ry0 = (py_min.floor() as i64 - 1).clamp(0, src_h as i64 - 1) as usize;
    let rx1 = (px_max.ceil() as i64 + 2).clamp(rx0 as i64 + 1, src_w as i64) as usize;
    let ry1 = (py_max.ceil() as i64 + 2).clamp(ry0 as i64 + 1, src_h as i64) as usize;

    let rw = rx1 - rx0;
    let rh = ry1 - ry0;

    // Cap buffer to 2048 — GDAL bilinear-resamples if buf_size < window_size
    const MAX_BUF: usize = 2048;
    let bw = rw.min(MAX_BUF);
    let bh = rh.min(MAX_BUF);

    let buf = band
        .read_as::<f32>(
            (rx0 as isize, ry0 as isize),
            (rw, rh),
            (bw, bh),
            Some(ResampleAlg::Bilinear),
        )
        .context("read_as")?;
    let src_data = buf.data();

    let sx = bw as f64 / rw as f64; // source → buffer scale
    let sy = bh as f64 / rh as f64;

    // ── Transform all 512×512 output pixel centres from 3857 → source SRS ────
    let n = 512 * 512;
    let pw = (east_m - west_m) / 512.0;
    let ph = (north_m - south_m) / 512.0;

    let mut px3 = Vec::with_capacity(n);
    let mut py3 = Vec::with_capacity(n);
    for row in 0..512usize {
        for col in 0..512usize {
            px3.push(west_m + (col as f64 + 0.5) * pw);
            py3.push(north_m - (row as f64 + 0.5) * ph);
        }
    }
    to_src
        .transform_coords(&mut px3, &mut py3, &mut [])
        .context("transform pixel grid")?;

    // ── Sample + encode ───────────────────────────────────────────────────────
    let mut rgb = vec![0u8; n * 3];
    let mut any_valid = false;

    for i in 0..n {
        let bpx = ((px3[i] - gt[0]) / gt[1] - rx0 as f64) * sx;
        let bpy = ((py3[i] - gt[3]) / gt[5] - ry0 as f64) * sy;

        let elev = sample_bilinear(src_data, bw, bh, bpx, bpy, nodata);
        let c = encode_mapbox(elev, base_val, interval, round, nodata);
        if c != [0, 0, 0] {
            any_valid = true;
        }
        rgb[i * 3..i * 3 + 3].copy_from_slice(&c);
    }

    if !any_valid {
        return Ok(None);
    }

    let tile = match format {
        Format::Webp => crate::tile_format::webp::encode_tile(&rgb, compress)?,
        Format::Png => crate::tile_format::png::encode_tile(&rgb, compress)?,
    };
    Ok(Some(tile))
}
