use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use pmtiles::{TileCoord, TileId};
use rayon::prelude::*;

mod container;
mod encoder;
mod raster;
mod tile;
mod tile_format;

use container::Writer;
use encoder::Encoding;
use raster::{dataset_wgs84_bounds, process_tile};
use tile::{lat_to_tile_y_xyz, lon_to_tile_x};
use tile_format::Format;

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

    /// RGB encoding scheme [default: mapbox]
    #[arg(long, value_enum, default_value = "mapbox")]
    encoding: Encoding,

    /// Output tile format [default: webp]
    #[arg(long, value_enum, default_value = "webp")]
    format: Format,

    /// Compression level 1–9 (omit for fastest; 6 is a good default).
    /// Higher = smaller file, slower encoding. Format-agnostic — maps to the
    /// best available compressor for the output format.
    #[arg(long, value_name = "LEVEL", value_parser = clap::value_parser!(u8).range(1..=9))]
    compress: Option<u8>,

    /// Override the nodata value from the raster metadata.
    /// Useful when the file has no embedded nodata or it is wrong (common values: 0, -9999, -32768).
    #[arg(long, allow_hyphen_values = true)]
    nodata: Option<f32>,

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

    // ── Open output writer (container inferred from file extension) ───────────
    let mut writer = Writer::open(&args.output, args.format, args.min_z, args.max_z)?;

    // ── Parallel tile generation — chunked to bound peak memory ──────────────
    // Each chunk is processed in parallel and written before the next begins.
    // Peak RAM ≈ CHUNK_SIZE × avg_tile_size (instead of all tiles at once).
    const CHUNK_SIZE: usize = 4096;

    let pb = ProgressBar::new(tiles.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:45.cyan/blue} {pos:>6}/{len} tiles  {tiles_per_sec}/s  eta {eta}",
        )
        .unwrap()
        .with_key("tiles_per_sec", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            write!(w, "{}", state.per_sec() as u64).unwrap();
        }),
    );

    if args.encoding == Encoding::Terrarium {
        if args.base_val != -10000.0 {
            eprintln!("Warning: --base-val is ignored for --encoding terrarium");
        }
        if args.interval != 0.1 {
            eprintln!("Warning: --interval is ignored for --encoding terrarium");
        }
        if args.round_digits != 3 {
            eprintln!("Warning: --round-digits is ignored for --encoding terrarium");
        }
    }

    let bv = args.base_val;
    let iv = args.interval;
    let rd = args.round_digits;
    let encoding = args.encoding;
    let format = args.format;
    let compress = args.compress;
    let nodata = args.nodata;
    let mut n_written: u64 = 0;
    let mut n_errors: u64 = 0;

    for chunk in tiles.chunks(CHUNK_SIZE) {
        // par_iter on a slice preserves order → results match chunk order (= Hilbert order)
        let chunk_results: Vec<Result<Option<Vec<u8>>>> = chunk
            .par_iter()
            .map(|&(z, x, y)| {
                let r = process_tile(&input_str, z, x, y, bv, iv, rd, encoding, format, compress, nodata);
                pb.inc(1);
                r
            })
            .collect();

        for (i, result) in chunk_results.into_iter().enumerate() {
            match result {
                Ok(Some(tile)) => {
                    let (z, x, y) = chunk[i];
                    writer.add_tile(z, x, y, &tile).context("add_tile")?;
                    n_written += 1;
                }
                Ok(None) => {} // entirely nodata tile, skip
                Err(e) => {
                    let (z, x, y) = chunk[i];
                    eprintln!("Warning: tile {}/{}/{} failed: {:#}", z, x, y, e);
                    n_errors += 1;
                }
            }
        }
        // chunk_results dropped here — memory freed between chunks
    }

    pb.finish_with_message("done");
    eprintln!("{} non-empty tiles written", n_written);
    if n_errors > 0 {
        eprintln!("Warning: {} tiles failed and were skipped", n_errors);
    }

    writer.finalize().context("finalize")?;

    let sz = std::fs::metadata(&args.output)?.len();
    eprintln!(
        "Written {:?}  ({:.1} MB)",
        args.output,
        sz as f64 / 1_048_576.0
    );
    Ok(())
}
