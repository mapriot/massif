use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use pmtiles::{TileCoord, TileId};
use rayon::prelude::*;

mod container;
mod encoder;
mod frontier;
mod raster;
mod tile;
mod tile_format;

use container::Writer;
use encoder::Encoding;
use frontier::{bounds_by_zoom, initial_frontier, push_children_in_bounds, TileJob, ZoomBounds};
use raster::{dataset_wgs84_bounds, eval_tile, process_tile, TileEval};
use tile::{lat_to_tile_y_xyz, lon_to_tile_x};
use tile_format::TileFormat;

#[derive(Parser, Debug)]
#[command(
    name = "massif",
    version,
    about = "Fast terrain-RGB tile generator — converts elevation rasters to PMTiles or MBTiles"
)]
struct Args {
    /// Input elevation raster — GeoTIFF, VRT, or any GDAL-supported format and CRS
    input: PathBuf,

    /// Output file — .pmtiles or .mbtiles (container inferred from extension)
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
    format: TileFormat,

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

    // ── Terrarium ignores the Mapbox encoding knobs ──────────────────────────
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

    // ── Open output writer (container inferred from file extension) ───────────
    let mut writer = Writer::open(&args.output, args.format, args.min_z, args.max_z)?;

    // PMTiles needs Hilbert-ordered input, so it uses the flat path
    // (enumerate → sort → stream). MBTiles is order-independent, so it uses the
    // sparse frontier, which prunes entire nodata subtrees instead of reading
    // them. Both produce the same set of non-empty tiles.
    let is_pmtiles = args.output.extension().and_then(|e| e.to_str()) == Some("pmtiles");
    let (n_written, n_errors) = if is_pmtiles {
        // PMTiles needs Hilbert-ordered input → flat enumerate + sort + stream.
        run_flat(&args, &input_str, west_lon, south_lat, east_lon, north_lat, &mut writer, true)?
    } else {
        // MBTiles is order-independent. Large builds use the sparse frontier to
        // prune whole nodata subtrees; small ones use the flat path, which avoids
        // the frontier's shallow top-down dependency for no pruning benefit.
        let total = candidate_total(west_lon, south_lat, east_lon, north_lat, args.min_z, args.max_z);
        if total >= FRONTIER_MIN_TOTAL {
            run_frontier(&args, &input_str, west_lon, south_lat, east_lon, north_lat, &mut writer)?
        } else {
            run_flat(&args, &input_str, west_lon, south_lat, east_lon, north_lat, &mut writer, false)?
        }
    };

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

/// Chunk size for memory-bounded parallel processing: each chunk is written
/// before the next begins, so peak RAM ≈ CHUNK_SIZE × avg encoded tile size.
const CHUNK_SIZE: usize = 4096;

fn make_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:45.cyan/blue} {pos:>6}/{len} tiles  {tiles_per_sec}/s  eta {eta}",
        )
        .unwrap()
        .with_key("tiles_per_sec", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            write!(w, "{}", state.per_sec() as u64).unwrap();
        }),
    );
    pb
}

/// Flat generation: enumerate every candidate tile, optionally Hilbert-sort (so
/// the PMTiles streaming writer receives tiles in order), then process in
/// memory-bounded chunks. Returns (tiles written, tiles that errored).
#[allow(clippy::too_many_arguments)]
fn run_flat(
    args: &Args,
    input_str: &str,
    west_lon: f64,
    south_lat: f64,
    east_lon: f64,
    north_lat: f64,
    writer: &mut Writer,
    sort: bool,
) -> Result<(u64, u64)> {
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

    // Pre-sort by Hilbert ID — the PMTiles streaming writer requires it.
    // sort_by_cached_key computes each TileId once (O(n)) instead of per
    // comparison (O(n log n)) — significant for millions of tiles.
    if sort {
        eprintln!("Sorting {} tiles by Hilbert ID…", tiles.len());
        tiles.sort_by_cached_key(|&(z, x, y)| {
            TileId::from(TileCoord::new(z, x, y).expect("valid coord")).value()
        });
    }

    let pb = make_progress_bar(tiles.len() as u64);
    let (bv, iv, rd) = (args.base_val, args.interval, args.round_digits);
    let (encoding, format, compress, nodata) =
        (args.encoding, args.format, args.compress, args.nodata);
    let mut n_written = 0u64;
    let mut n_errors = 0u64;

    for chunk in tiles.chunks(CHUNK_SIZE) {
        // par_iter on a slice preserves order → results match Hilbert order
        let chunk_results: Vec<Result<Option<Vec<u8>>>> = chunk
            .par_iter()
            .map(|&(z, x, y)| {
                let r =
                    process_tile(input_str, z, x, y, bv, iv, rd, encoding, format, compress, nodata);
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
                Ok(None) => {} // empty tile, skip
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
    Ok((n_written, n_errors))
}

/// Shared, read-only context for the parallel frontier descent.
struct FrontierCtx<'a> {
    input: &'a str,
    zb: &'a [ZoomBounds],
    min_z: u8,
    max_z: u8,
    bv: f64,
    iv: f64,
    rd: u32,
    encoding: Encoding,
    format: TileFormat,
    compress: Option<u8>,
    nodata: Option<f32>,
    /// Non-empty tiles are sent here to the single writer thread.
    tx: SyncSender<(TileJob, Vec<u8>)>,
    n_errors: &'a AtomicU64,
    pb: &'a ProgressBar,
}

/// Evaluate one tile, then recurse into its in-bounds children in parallel.
/// Rayon work-stealing keeps every core busy: as soon as a parent finishes, its
/// children become stealable, so low-zoom levels (few but expensive tiles) don't
/// idle the pool the way a per-zoom barrier would. Only prune-safe `NoData`
/// results stop the descent.
fn descend(t: TileJob, ctx: &FrontierCtx) {
    let r = eval_tile(
        ctx.input, t.z, t.x, t.y, ctx.bv, ctx.iv, ctx.rd, ctx.encoding, ctx.format, ctx.compress,
        ctx.nodata,
    );
    ctx.pb.inc(1);

    let expand = match r {
        Ok(TileEval::Rendered(data)) => {
            // A send error means the writer thread has already failed; its error
            // surfaces after join, so we just stop feeding it.
            let _ = ctx.tx.send((t, data));
            true
        }
        // Source had data but this tile encoded empty — not prune-safe, keep going.
        Ok(TileEval::Blank) => true,
        // Whole subtree is nodata — prune.
        Ok(TileEval::NoData) => false,
        Err(e) => {
            eprintln!("Warning: tile {}/{}/{} failed: {:#}", t.z, t.x, t.y, e);
            ctx.n_errors.fetch_add(1, Ordering::Relaxed);
            // Don't let a transient read error drop a whole subtree.
            true
        }
    };

    if expand && t.z < ctx.max_z {
        let child_bounds = &ctx.zb[(t.z - ctx.min_z) as usize + 1];
        let mut children = Vec::with_capacity(4);
        push_children_in_bounds(t, child_bounds, &mut children);
        children.into_par_iter().for_each(|c| descend(c, ctx));
    }
}

/// Below this many candidate tiles, MBTiles uses the flat path instead of the
/// frontier. On a small extent the frontier's top-down descent stalls the pool
/// at shallow zooms (few, expensive tiles) for no real pruning benefit, while
/// the flat path saturates immediately. Above it — continental/global builds —
/// the shallow stall is negligible and pruning nodata subtrees dominates. Tight
/// regional extents (a small country at moderate zoom) fall below the line and
/// keep the exact flat behaviour; anything larger frontiers.
const FRONTIER_MIN_TOTAL: u64 = 4096;

/// Total candidate tiles across `[min_z, max_z]` within bounds — the size signal
/// used to pick flat vs frontier for MBTiles.
fn candidate_total(
    west_lon: f64,
    south_lat: f64,
    east_lon: f64,
    north_lat: f64,
    min_z: u8,
    max_z: u8,
) -> u64 {
    bounds_by_zoom(west_lon, south_lat, east_lon, north_lat, min_z, max_z)
        .iter()
        .map(|b| b.tile_count())
        .sum()
}

/// Sparse-frontier generation for MBTiles: descend the pyramid from `min_z` with
/// rayon work-stealing, expanding only the children of tiles that had data and
/// pruning whole nodata subtrees. Only prune-safe `NoData` results are pruned,
/// so the non-empty tile set is byte-for-byte identical to the flat path. A
/// single writer thread owns the SQLite connection; the bounded channel provides
/// backpressure so memory stays flat. Returns (tiles written, tiles that errored).
#[allow(clippy::too_many_arguments)]
fn run_frontier(
    args: &Args,
    input_str: &str,
    west_lon: f64,
    south_lat: f64,
    east_lon: f64,
    north_lat: f64,
    writer: &mut Writer,
) -> Result<(u64, u64)> {
    let (min_z, max_z) = (args.min_z, args.max_z);
    let zb = bounds_by_zoom(west_lon, south_lat, east_lon, north_lat, min_z, max_z);
    let total_candidates: u64 = zb.iter().map(|b| b.tile_count()).sum();
    eprintln!(
        "Zoom {}-{}:  up to {} candidate tiles, sparse frontier  ({} threads)",
        min_z,
        max_z,
        total_candidates,
        rayon::current_num_threads()
    );

    let pb = make_progress_bar(total_candidates);
    let n_errors = AtomicU64::new(0);

    // Bounded channel: producers block when the writer falls behind, capping the
    // number of encoded tiles held in flight.
    let (tx, rx) = sync_channel::<(TileJob, Vec<u8>)>(256);
    let ctx = FrontierCtx {
        input: input_str,
        zb: &zb,
        min_z,
        max_z,
        bv: args.base_val,
        iv: args.interval,
        rd: args.round_digits,
        encoding: args.encoding,
        format: args.format,
        compress: args.compress,
        nodata: args.nodata,
        tx,
        n_errors: &n_errors,
        pb: &pb,
    };

    let n_written = std::thread::scope(|s| -> Result<u64> {
        // Single writer thread owns the SQLite connection.
        let writer_thread = s.spawn(move || -> Result<u64> {
            let mut n = 0u64;
            for (t, data) in rx {
                writer.add_tile(t.z, t.x, t.y, &data).context("add_tile")?;
                n += 1;
            }
            Ok(n)
        });

        // Parallel descent from the whole min_z row.
        initial_frontier(&zb[0], min_z)
            .into_par_iter()
            .for_each(|t| descend(t, &ctx));
        // Drop the last sender so the writer thread's loop ends.
        drop(ctx);
        writer_thread.join().expect("writer thread panicked")
    })?;

    pb.finish_with_message("done");
    Ok((n_written, n_errors.load(Ordering::Relaxed)))
}
