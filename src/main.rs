use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use pmtiles::{PmTilesWriter, TileCoord, TileId, TileType};
use rayon::prelude::*;

mod encoder;
mod output;
mod raster;
mod tile;

use raster::{dataset_wgs84_bounds, process_tile};
use tile::{lat_to_tile_y_xyz, lon_to_tile_x};

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
    let (west_lon, south_lat, east_lon, north_lat) = dataset_wgs84_bounds(&args.input)?;

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
    let compress = args.compress;
    let mut n_written: u64 = 0;

    for chunk in tiles.chunks(CHUNK_SIZE) {
        // par_iter on a slice preserves order → results match chunk order (= Hilbert order)
        let chunk_results: Vec<Option<Vec<u8>>> = chunk
            .par_iter()
            .map(|&(z, x, y)| {
                let r = process_tile(&input_str, z, x, y, bv, iv, rd, compress);
                pb.inc(1);
                r.ok().flatten()
            })
            .collect();

        for (i, maybe_tile) in chunk_results.iter().enumerate() {
            if let Some(tile) = maybe_tile {
                let (z, x, y) = chunk[i];
                let coord = TileCoord::new(z, x, y).context("TileCoord")?;
                writer.add_tile(coord, tile).context("add_tile")?;
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
