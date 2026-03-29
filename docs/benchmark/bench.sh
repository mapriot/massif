#!/usr/bin/env bash
# bench.sh — Exhaustive massif parameter benchmark
#
# Run from the repo root:
#   docs/benchmark/bench.sh <INPUT_RASTER> [MIN_Z] [MAX_Z] [WORKERS]
#
# Example:
#   docs/benchmark/bench.sh indonesia.tif 5 10 14
#
# Outputs:
#   docs/benchmark/out/      — all generated tile files
#   docs/benchmark/out/results.csv — machine-readable results
#   docs/benchmark/out/results.txt — human-readable summary
#
# For each run we collect: time, file size, tile count, and a sample tile size.
# Sample tile is extracted from mbtiles via sqlite3; for pmtiles runs we record
# average tile size (file_size / tile_count).

set -euo pipefail

INPUT="${1:?Usage: ./bench.sh <INPUT_RASTER> [MIN_Z] [MAX_Z] [WORKERS]}"
MIN_Z="${2:-5}"
MAX_Z="${3:-10}"
WORKERS="${4:-$(sysctl -n hw.logicalcpu 2>/dev/null || nproc 2>/dev/null || echo 8)}"

MASSIF="./target/release/massif"
OUTDIR="docs/benchmark/out"

# ── Check prerequisites ──────────────────────────────────────────────────────
if [ ! -f "$INPUT" ]; then
    echo "Error: input raster not found: $INPUT" >&2
    exit 1
fi

if [ ! -x "$MASSIF" ]; then
    echo "Building massif in release mode..." >&2
    cargo build --release
fi

mkdir -p "$OUTDIR"

# ── CSV header ───────────────────────────────────────────────────────────────
CSV="$OUTDIR/results.csv"
TXT="$OUTDIR/results.txt"
echo "run,container,format,encoding,compress,round_digits,time_sec,file_size_bytes,file_size_mb,tile_count,sample_tile_bytes,avg_tile_bytes" > "$CSV"

printf "%-50s %8s %10s %8s %12s %10s\n" "Run" "Time(s)" "Size(MB)" "Tiles" "SampleTile" "AvgTile" > "$TXT"
printf "%-50s %8s %10s %8s %12s %10s\n" "$(printf '%.0s-' {1..50})" "--------" "----------" "--------" "------------" "----------" >> "$TXT"

RUN_NUM=0

# ── Run a single benchmark ───────────────────────────────────────────────────
run_bench() {
    local label="$1"
    local container="$2"   # pmtiles or mbtiles
    local format="$3"      # webp or png
    local encoding="$4"    # mapbox or terrarium
    local compress="$5"    # "" for none, or 1-9
    local round_digits="$6" # 0-8 (only matters for mapbox)

    RUN_NUM=$((RUN_NUM + 1))
    local outfile="$OUTDIR/${label}.${container}"

    # Build command
    local cmd=("$MASSIF")
    cmd+=(--encoding "$encoding")
    cmd+=(--format "$format")
    cmd+=(--min-z "$MIN_Z")
    cmd+=(--max-z "$MAX_Z")
    cmd+=(-j "$WORKERS")

    if [ -n "$compress" ]; then
        cmd+=(--compress "$compress")
    fi

    if [ "$encoding" = "mapbox" ]; then
        cmd+=(-r "$round_digits")
    fi

    cmd+=("$INPUT" "$outfile")

    # Remove old output
    rm -f "$outfile"

    echo ""
    echo "━━━ Run $RUN_NUM: $label ━━━"
    echo "  ${cmd[*]}"

    # Time the run
    local start_time=$SECONDS
    "${cmd[@]}" 2>&1 | tail -3
    local elapsed=$((SECONDS - start_time))

    # File size
    local file_size
    file_size=$(stat -f%z "$outfile" 2>/dev/null || stat -c%s "$outfile" 2>/dev/null || echo 0)
    local file_size_mb
    file_size_mb=$(echo "scale=1; $file_size / 1048576" | bc)

    # Tile count — extract from massif output or count in container
    local tile_count=0
    local sample_tile_bytes=0
    local avg_tile_bytes=0

    if [ "$container" = "mbtiles" ]; then
        # Get tile count and a sample tile size from sqlite
        tile_count=$(sqlite3 "$outfile" "SELECT COUNT(*) FROM tiles;" 2>/dev/null || echo 0)
        # Pick the median tile by zoom (middle zoom level, first tile found)
        local mid_z=$(( (MIN_Z + MAX_Z) / 2 ))
        sample_tile_bytes=$(sqlite3 "$outfile" \
            "SELECT length(tile_data) FROM tiles WHERE zoom_level=$mid_z ORDER BY tile_column, tile_row LIMIT 1;" \
            2>/dev/null || echo 0)
        if [ "$tile_count" -gt 0 ]; then
            avg_tile_bytes=$(echo "scale=0; $file_size / $tile_count" | bc)
        fi
    elif [ "$container" = "pmtiles" ]; then
        # For pmtiles, estimate from file size (header overhead ~127 bytes + index)
        # We'll get exact count from massif's stderr output (captured above)
        # Use the paired mbtiles run if available, otherwise estimate
        local paired_mbtiles="$OUTDIR/${label}.mbtiles"
        if [ -f "$paired_mbtiles" ]; then
            tile_count=$(sqlite3 "$paired_mbtiles" "SELECT COUNT(*) FROM tiles;" 2>/dev/null || echo 0)
        fi
        if [ "$tile_count" -gt 0 ]; then
            avg_tile_bytes=$(echo "scale=0; $file_size / $tile_count" | bc)
        fi
        sample_tile_bytes="$avg_tile_bytes"  # best we can do without pmtiles CLI
    fi

    # Record
    echo "$RUN_NUM,$container,$format,$encoding,${compress:-none},$round_digits,$elapsed,$file_size,$file_size_mb,$tile_count,$sample_tile_bytes,$avg_tile_bytes" >> "$CSV"
    printf "%-50s %8d %10s %8s %12s %10s\n" "$label" "$elapsed" "${file_size_mb}MB" "$tile_count" "${sample_tile_bytes}B" "${avg_tile_bytes}B" >> "$TXT"

    echo "  -> ${elapsed}s  ${file_size_mb} MB  tiles=$tile_count  sample=${sample_tile_bytes}B  avg=${avg_tile_bytes}B"
}

echo "========================================================================"
echo " massif benchmark"
echo " Input:   $INPUT"
echo " Zoom:    $MIN_Z-$MAX_Z"
echo " Workers: $WORKERS"
echo " Output:  $OUTDIR/"
echo "========================================================================"

# ── Phase 1: Format × Compress × Container (Mapbox default, r=3) ────────────
echo ""
echo "== PHASE 1: Core matrix (mapbox, r=3) =="
echo "   Format × Compress × Container"

for fmt in webp png; do
    for compress in "" 1 3 6 9; do
        compress_label="${compress:-none}"
        # Run mbtiles first (so pmtiles can reference tile count)
        run_bench "mapbox_${fmt}_c${compress_label}_r3" "mbtiles" "$fmt" "mapbox" "$compress" "3"
        run_bench "mapbox_${fmt}_c${compress_label}_r3" "pmtiles" "$fmt" "mapbox" "$compress" "3"
    done
done

# ── Phase 2: Encoding comparison (terrarium vs mapbox) ───────────────────────
echo ""
echo "== PHASE 2: Terrarium encoding =="

for fmt in webp png; do
    for compress in "" 6; do
        compress_label="${compress:-none}"
        run_bench "terrarium_${fmt}_c${compress_label}" "mbtiles" "$fmt" "terrarium" "$compress" "3"
        run_bench "terrarium_${fmt}_c${compress_label}" "pmtiles" "$fmt" "terrarium" "$compress" "3"
    done
done

# ── Phase 3: Round digits impact (mapbox, webp, compress 6, both containers) ─
echo ""
echo "== PHASE 3: Round digits (mapbox, webp, compress 6) =="

for r in 0 1 5 7; do
    # r=3 already covered in phase 1
    run_bench "mapbox_webp_c6_r${r}" "mbtiles" "webp" "mapbox" "6" "$r"
    run_bench "mapbox_webp_c6_r${r}" "pmtiles" "webp" "mapbox" "6" "$r"
done

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "========================================================================"
echo " Benchmark complete! $RUN_NUM runs total."
echo ""
echo " Results:"
echo "   $CSV  (machine-readable)"
echo "   $TXT  (human-readable)"
echo ""
echo " Output files in $OUTDIR/"
echo "========================================================================"

cat "$TXT"
