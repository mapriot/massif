use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use image_webp::{ColorType, WebPEncoder};
use indicatif::{ProgressBar, ProgressStyle};
use pmtiles::{PmTilesWriter, TileCoord, TileId, TileType};
use rayon::prelude::*;

// Half circumference at equator in metres (π × WGS84 R)
const HALF_CIRC: f64 = 20_037_508.342_789_244;

#[derive(Parser, Debug)]
#[command(
    name = "massif",
    about = "Generate Mapbox terrain-RGB PMTiles from a Float32 elevation raster (any input CRS)"
)]
struct Args {
    /// Input Float32 GeoTIFF or VRT (any GDAL-supported CRS, typically EPSG:4326 or UTM)
    input: PathBuf,

    /// Output PMTiles file path
    output: PathBuf,

    /// Base elevation offset — Mapbox decode: height = base_val + (R·65536+G·256+B) · interval
    #[arg(short = 'b', long, default_value = "-10000", allow_hyphen_values = true)]
    base_val: f64,

    /// Elevation interval / precision in metres
    #[arg(short = 'i', long, default_value = "0.1")]
    interval: f64,

    /// Zero out the lowest N bits of the encoded integer (rio-rgbify -r)
    #[arg(short = 'r', long, default_value = "3")]
    round_digits: u32,

    /// Minimum zoom level to generate
    #[arg(long, default_value = "5")]
    min_z: u8,

    /// Maximum zoom level to generate
    #[arg(long, default_value = "12")]
    max_z: u8,

    /// Compression level 1–9 (omit for fastest; 6 is a good default).
    /// Higher = smaller file, slower encoding. Format-agnostic — maps to the
    /// best available compressor for the output format (currently libwebp lossless effort).
    #[arg(long, value_name = "LEVEL", value_parser = clap::value_parser!(u8).range(1..=9))]
    compress: Option<u8>,

    /// Worker thread count (default: all CPUs)
    #[arg(short = 'j', long)]
    workers: Option<usize>,
}

// ─── Tile math (XYZ / Web Mercator) ──────────────────────────────────────────

/// Return [west, south, east, north] in EPSG:3857 metres for tile (z, x, y_xyz).
/// XYZ convention: y=0 is at the north.
fn tile_bounds_3857(z: u8, x: u32, y_xyz: u32) -> [f64; 4] {
    let n = (1u64 << z) as f64;
    let size = 2.0 * HALF_CIRC / n;
    let west = -HALF_CIRC + x as f64 * size;
    let east = west + size;
    let north = HALF_CIRC - y_xyz as f64 * size;
    let south = north - size;
    [west, south, east, north]
}

/// WGS84 longitude → tile column X at zoom z.
fn lon_to_tile_x(lon: f64, z: u8) -> u32 {
    let n = (1u64 << z) as f64;
    let x = ((lon + 180.0) / 360.0 * n).floor() as i64;
    x.clamp(0, n as i64 - 1) as u32
}

/// WGS84 latitude → tile row Y (XYZ, y=0 at north) at zoom z.
fn lat_to_tile_y_xyz(lat: f64, z: u8) -> u32 {
    let lat = lat.clamp(-85.051_129, 85.051_129);
    let n = (1u64 << z) as f64;
    let lat_r = lat.to_radians();
    let y = ((1.0 - (lat_r.tan() + 1.0 / lat_r.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor() as i64;
    y.clamp(0, n as i64 - 1) as u32
}

// ─── Raster helpers ───────────────────────────────────────────────────────────

/// Bilinear sample from a flat f32 buffer.  Returns `nodata` if out of bounds.
fn sample_bilinear(
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

/// Encode an elevation value to Mapbox terrain-RGB.
///
/// Matches rio-rgbify:  encoded = ⌊(elev − base_val) / interval⌋  (zeros lower bits)
/// MapLibre decodes:    height  = base_val + (R·65536 + G·256 + B) · interval
#[inline(always)]
fn encode_elevation(elev: f32, base_val: f64, interval: f64, round: u32, nodata: f32) -> [u8; 3] {
    if (elev - nodata).abs() < 0.5 || elev.is_nan() {
        return [0, 0, 0];
    }
    let encoded = ((elev as f64 - base_val) / interval).floor() as i64;
    let mask = !((1i64 << round) - 1);
    let enc = (encoded & mask).clamp(0, 0xFF_FFFF) as u32;
    [((enc >> 16) & 0xFF) as u8, ((enc >> 8) & 0xFF) as u8, (enc & 0xFF) as u8]
}

// ─── Per-tile processing ──────────────────────────────────────────────────────

/// Process one tile; returns `None` if entirely nodata.
/// Opens its own GDAL handle — required because GDAL datasets are not Send.
fn process_tile(
    input_path: &str,
    z: u8,
    x: u32,
    y_xyz: u32,
    base_val: f64,
    interval: f64,
    round: u32,
    compress: Option<u8>,
) -> Result<Option<Vec<u8>>> {
    use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
    use gdal::Dataset;
    use gdal::raster::ResampleAlg;

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
        // Map source-SRS coordinate → buffer pixel coordinate
        let bpx = ((px3[i] - gt[0]) / gt[1] - rx0 as f64) * sx;
        let bpy = ((py3[i] - gt[3]) / gt[5] - ry0 as f64) * sy;

        let elev = sample_bilinear(src_data, bw, bh, bpx, bpy, nodata);
        let c = encode_elevation(elev, base_val, interval, round, nodata);
        if c != [0, 0, 0] {
            any_valid = true;
        }
        rgb[i * 3..i * 3 + 3].copy_from_slice(&c);
    }

    if !any_valid {
        return Ok(None);
    }

    // ── WebP encode ───────────────────────────────────────────────────────────
    // No --compress:  image-webp pure-Rust lossless — fastest path.
    // --compress 1-9: libwebp lossless; maps 1-9 → effort 0-100.
    //                 Lossless is mandatory — lossy corrupts elevation values.
    let webp: Vec<u8> = if let Some(level) = compress {
        let effort = (level - 1) as f32 * 100.0 / 8.0; // 1→0, 5→50, 9→100
        webp::Encoder::from_rgb(&rgb, 512, 512)
            .encode_simple(true, effort) // lossless=true, quality=effort
            .unwrap()
            .to_vec()
    } else {
        let mut buf = Vec::new();
        WebPEncoder::new(&mut buf)
            .encode(&rgb, 512, 512, ColorType::Rgb8)
            .context("webp encode")?;
        buf
    };

    Ok(Some(webp))
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(w) = args.workers {
        rayon::ThreadPoolBuilder::new()
            .num_threads(w)
            .build_global()
            .context("build rayon thread pool")?;
    }

    let input_str = args
        .input
        .to_str()
        .context("input path is not valid UTF-8")?
        .to_owned();

    // ── Dataset metadata → WGS84 bounds for tile list ─────────────────────────
    let (west_lon, south_lat, east_lon, north_lat) = {
        use gdal::spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef};
        use gdal::Dataset;

        let ds = Dataset::open(&args.input).context("open dataset")?;
        let gt = ds.geo_transform().context("geo_transform")?;
        let (w, h) = ds.raster_size();

        // Four corners in source SRS
        let ox = gt[0];
        let oy = gt[3];
        let ex = ox + gt[1] * w as f64;
        let ey = oy + gt[5] * h as f64;

        let mut xs = [ox, ex, ox, ex];
        let mut ys = [oy, oy, ey, ey];

        let mut src_srs =
            SpatialRef::from_wkt(&ds.projection()).context("source SRS for bounds")?;
        src_srs.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        let mut wgs84 = SpatialRef::from_epsg(4326).context("EPSG:4326")?;
        wgs84.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
        let to_wgs84 = CoordTransform::new(&src_srs, &wgs84).context("coord transform")?;
        to_wgs84
            .transform_coords(&mut xs, &mut ys, &mut [])
            .context("transform corners to WGS84")?;

        let w_lon = xs.iter().cloned().fold(f64::MAX, f64::min);
        let e_lon = xs.iter().cloned().fold(f64::MIN, f64::max);
        let s_lat = ys.iter().cloned().fold(f64::MAX, f64::min);
        let n_lat = ys.iter().cloned().fold(f64::MIN, f64::max);

        eprintln!(
            "Input: {}×{} px  WGS84 bounds [{:.4}W {:.4}S {:.4}E {:.4}N]",
            w, h, w_lon, s_lat, e_lon, n_lat
        );
        (w_lon, s_lat, e_lon, n_lat)
    };

    // ── Build tile list ───────────────────────────────────────────────────────
    let mut tiles: Vec<(u8, u32, u32)> = Vec::new();
    for z in args.min_z..=args.max_z {
        let x0 = lon_to_tile_x(west_lon, z);
        let x1 = lon_to_tile_x(east_lon, z);
        let y0 = lat_to_tile_y_xyz(north_lat, z); // smaller y = north
        let y1 = lat_to_tile_y_xyz(south_lat, z);
        for x in x0..=x1 {
            for y in y0..=y1 {
                tiles.push((z, x, y));
            }
        }
    }
    eprintln!(
        "Zoom {}-{}:  {} candidate tiles  ({} threads)",
        args.min_z,
        args.max_z,
        tiles.len(),
        rayon::current_num_threads()
    );

    // ── Pre-sort tiles by Hilbert ID so PMTiles is written in optimal order ────
    // Rayon's par_iter preserves input order in collect(), so chunks processed
    // in parallel will also arrive in Hilbert order — no post-sort needed.
    tiles.sort_unstable_by_key(|&(z, x, y)| {
        TileId::from(TileCoord::new(z, x, y).expect("valid coord")).value()
    });

    // ── Open PMTiles writer before processing (stream tiles, no RAM spike) ────
    let f = File::create(&args.output)
        .with_context(|| format!("create output {:?}", args.output))?;

    let mut writer = PmTilesWriter::new(TileType::Webp)
        .min_zoom(args.min_z)
        .max_zoom(args.max_z)
        .create(f)
        .context("create PMTiles writer")?;

    // ── Parallel tile generation — chunked to bound peak memory ──────────────
    // Each chunk is processed in parallel and written before the next begins.
    // Peak RAM ≈ CHUNK_SIZE × avg_tile_size (instead of all tiles at once).
    const CHUNK_SIZE: usize = 4096;

    let pb = ProgressBar::new(tiles.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:45.cyan/blue} {pos:>6}/{len} tiles  {per_sec}  eta {eta}",
        )
        .unwrap(),
    );

    let bv = args.base_val;
    let iv = args.interval;
    let rd = args.round_digits;
    let lq = args.compress;
    let mut n_written: u64 = 0;

    for chunk in tiles.chunks(CHUNK_SIZE) {
        // par_iter on a slice preserves order → results match chunk order (= Hilbert order)
        let chunk_results: Vec<Option<Vec<u8>>> = chunk
            .par_iter()
            .map(|&(z, x, y)| {
                let r = process_tile(&input_str, z, x, y, bv, iv, rd, lq);
                pb.inc(1);
                r.ok().flatten()
            })
            .collect();

        for (i, maybe_webp) in chunk_results.iter().enumerate() {
            if let Some(webp) = maybe_webp {
                let (z, x, y) = chunk[i];
                let coord = TileCoord::new(z, x, y).context("TileCoord")?;
                writer.add_tile(coord, webp).context("add_tile")?;
                n_written += 1;
            }
        }
        // chunk_results dropped here — memory freed between chunks
    }

    pb.finish_with_message("done");
    eprintln!("{} non-empty tiles", n_written);

    writer.finalize().context("finalize")?;

    let sz = std::fs::metadata(&args.output)?.len();
    eprintln!(
        "Written {:?}  ({:.1} MB)",
        args.output,
        sz as f64 / 1_048_576.0
    );
    Ok(())
}
