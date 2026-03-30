use std::cell::RefCell;
use std::path::Path;

use anyhow::{Context, Result};
use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
use gdal::Dataset;

use crate::encoder::{encode_mapbox, encode_terrarium, Encoding};
use crate::tile_format::TileFormat;
use crate::tile::{merc_to_wgs84, tile_bounds_3857, HALF_CIRC};

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
        .transform_coords(&mut xs, &mut ys, &mut [] as &mut [f64])
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

// ── Per-thread dataset cache ──────────────────────────────────────────────────
// GDAL datasets are not Send, but are safe to reuse on the same thread. Rayon
// uses a persistent thread pool, so caching here means each worker opens the
// dataset once rather than once per tile. Especially impactful for VRT inputs
// where GDAL parses all sub-file references on every open.

struct DatasetCache {
    input_path: String,
    dataset: Dataset,
    gt: [f64; 6],
    src_w: usize,
    src_h: usize,
    nodata_base: f32,
    src_is_wgs84: bool,
    to_src: Option<CoordTransform>,
}

thread_local! {
    static TILE_CACHE: RefCell<Option<DatasetCache>> = RefCell::new(None);
}

fn init_dataset_cache(input_path: &str) -> Result<DatasetCache> {
    let dataset = Dataset::open(input_path).context("open dataset")?;
    let gt = dataset.geo_transform().context("geo_transform")?;
    let (src_w, src_h) = dataset.raster_size();
    let nodata_base = dataset
        .rasterband(1)
        .context("rasterband 1")?
        .no_data_value()
        .unwrap_or(-32_767.0) as f32;

    let mut src_srs = SpatialRef::from_wkt(&dataset.projection()).context("source SRS")?;
    src_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    let src_is_wgs84 = src_srs.is_geographic()
        && src_srs.auth_name().as_deref() == Some("EPSG")
        && src_srs.auth_code().ok() == Some(4326);

    let to_src = if !src_is_wgs84 {
        let mut srs_3857 = SpatialRef::from_epsg(3857).context("EPSG:3857")?;
        srs_3857.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        Some(CoordTransform::new(&srs_3857, &src_srs).context("coord transform 3857→src")?)
    } else {
        None
    };

    Ok(DatasetCache {
        input_path: input_path.to_owned(),
        dataset,
        gt,
        src_w,
        src_h,
        nodata_base,
        src_is_wgs84,
        to_src,
    })
}

/// Process one tile; returns `None` if entirely nodata.
/// Uses a per-thread dataset cache — GDAL datasets are not Send but are safe
/// to reuse on the same thread across tiles.
pub fn process_tile(
    input_path: &str,
    z: u8,
    x: u32,
    y_xyz: u32,
    base_val: f64,
    interval: f64,
    round: u32,
    encoding: Encoding,
    format: TileFormat,
    compress: Option<u8>,
    nodata_override: Option<f32>,
) -> Result<Option<Vec<u8>>> {
    use gdal::raster::ResampleAlg;

    TILE_CACHE.with(|cell| -> Result<Option<Vec<u8>>> {
        // Ensure this thread's cache is warm for the current input path.
        // The inner scope drops the mutable borrow before we take an immutable one below.
        {
            let mut opt = cell.borrow_mut();
            if opt.as_ref().map_or(true, |c| c.input_path != input_path) {
                *opt = Some(init_dataset_cache(input_path)?);
            }
        }

        let cache_ref = cell.borrow();
        let cache = cache_ref.as_ref().unwrap();

        let nodata = nodata_override.unwrap_or(cache.nodata_base);
        let gt = cache.gt;
        let src_w = cache.src_w;
        let src_h = cache.src_h;
        let src_is_wgs84 = cache.src_is_wgs84;

        let band = cache.dataset.rasterband(1).context("rasterband 1")?;

        // ── Tile extent in 3857 ───────────────────────────────────────────────
        let [west_m, south_m, east_m, north_m] = tile_bounds_3857(z, x, y_xyz);

        // ── Transform tile corners + midpoints to source SRS for read window ──
        let mid_x = (west_m + east_m) / 2.0;
        let mid_y = (south_m + north_m) / 2.0;
        let mut cx = [west_m, east_m, west_m, east_m, mid_x, west_m, east_m, mid_x];
        let mut cy = [south_m, south_m, north_m, north_m, mid_y, mid_y, mid_y, south_m];
        if let Some(ref t) = cache.to_src {
            t.transform_coords(&mut cx, &mut cy, &mut [] as &mut [f64])
                .context("transform corners")?;
        } else {
            for i in 0..cx.len() {
                let (lon, lat) = merc_to_wgs84(cx[i], cy[i]);
                cx[i] = lon;
                cy[i] = lat;
            }
        }

        let src_x_min = cx.iter().cloned().fold(f64::MAX, f64::min);
        let src_x_max = cx.iter().cloned().fold(f64::MIN, f64::max);
        let src_y_min = cy.iter().cloned().fold(f64::MAX, f64::min);
        let src_y_max = cy.iter().cloned().fold(f64::MIN, f64::max);

        // ── Convert source-SRS bbox to pixel indices ──────────────────────────
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

        // ── Early exit: if source buffer is entirely nodata, skip tile ─────
        let is_nd = |v: f32| (v - nodata).abs() < 0.5 || v.is_nan();
        if !src_data.iter().any(|&v| !is_nd(v)) {
            return Ok(None);
        }

        // ── Build pixel coordinates and sample + encode ──────────────────────
        const N: usize = 512 * 512;
        let pw = (east_m - west_m) / 512.0;
        let ph = (north_m - south_m) / 512.0;

        let mut rgb = vec![0u8; N * 3];
        let mut any_valid = false;

        if src_is_wgs84 {
            // ── WGS84 fast path: separable lon/lat grid ──────────────────────
            // The pixel grid is regular in Mercator space, and Mercator→WGS84
            // is separable: lon depends only on x, lat only on y. So we need
            // just 512+512 = 1024 coordinate conversions instead of 512×512.
            //
            // We also precompute the full geo→buffer-pixel mapping per row/col,
            // eliminating 262K divisions from the inner loop.

            let scale_x = sx / gt[1]; // combined geo→buffer scale
            let off_x = (gt[0] / gt[1] + rx0 as f64) * sx;
            let scale_y = sy / gt[5];
            let off_y = (gt[3] / gt[5] + ry0 as f64) * sy;

            let deg_per_merc = 180.0 / HALF_CIRC;
            let pi_over_hc = std::f64::consts::PI / HALF_CIRC;

            // Precompute per-column: Mercator x → lon → buffer pixel x
            let mut bpx_col = [0.0f64; 512];
            for col in 0..512usize {
                let x_m = west_m + (col as f64 + 0.5) * pw;
                let lon = x_m * deg_per_merc;
                bpx_col[col] = lon * scale_x - off_x;
            }

            // Precompute per-row: Mercator y → lat → buffer pixel y
            let mut bpy_row = [0.0f64; 512];
            for row in 0..512usize {
                let y_m = north_m - (row as f64 + 0.5) * ph;
                let lat = (2.0 * (y_m * pi_over_hc).exp().atan()
                    - std::f64::consts::FRAC_PI_2)
                    .to_degrees();
                bpy_row[row] = lat * scale_y - off_y;
            }

            // Fused sample + encode — no Vec allocations, no per-pixel trig
            for row in 0..512usize {
                let bpy = bpy_row[row];
                let base = row * 512 * 3;
                for col in 0..512usize {
                    let elev = sample_bilinear(src_data, bw, bh, bpx_col[col], bpy, nodata);
                    let c = match encoding {
                        Encoding::Mapbox => encode_mapbox(elev, base_val, interval, round, nodata),
                        Encoding::Terrarium => encode_terrarium(elev, nodata),
                    };
                    if c != [0, 0, 0] {
                        any_valid = true;
                    }
                    let idx = base + col * 3;
                    rgb[idx] = c[0];
                    rgb[idx + 1] = c[1];
                    rgb[idx + 2] = c[2];
                }
            }
        } else {
            // ── General path: full 262K coordinate transform ─────────────────
            let mut px3 = Vec::with_capacity(N);
            let mut py3 = Vec::with_capacity(N);
            for row in 0..512usize {
                for col in 0..512usize {
                    px3.push(west_m + (col as f64 + 0.5) * pw);
                    py3.push(north_m - (row as f64 + 0.5) * ph);
                }
            }
            cache
                .to_src
                .as_ref()
                .unwrap()
                .transform_coords(&mut px3, &mut py3, &mut [])
                .context("transform pixel grid")?;

            for i in 0..N {
                let bpx = ((px3[i] - gt[0]) / gt[1] - rx0 as f64) * sx;
                let bpy = ((py3[i] - gt[3]) / gt[5] - ry0 as f64) * sy;

                let elev = sample_bilinear(src_data, bw, bh, bpx, bpy, nodata);
                let c = match encoding {
                    Encoding::Mapbox => encode_mapbox(elev, base_val, interval, round, nodata),
                    Encoding::Terrarium => encode_terrarium(elev, nodata),
                };
                if c != [0, 0, 0] {
                    any_valid = true;
                }
                rgb[i * 3..i * 3 + 3].copy_from_slice(&c);
            }
        }

        if !any_valid {
            return Ok(None);
        }

        let tile = match format {
            TileFormat::Webp => crate::tile_format::webp::encode_tile(&rgb, compress)?,
            TileFormat::Png => crate::tile_format::png::encode_tile(&rgb, compress)?,
        };
        Ok(Some(tile))
    })
}
