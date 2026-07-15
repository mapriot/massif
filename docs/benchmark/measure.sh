#!/usr/bin/env bash
# measure.sh — focused A/B benchmark for a single massif configuration.
#
# Runs one massif invocation RUNS times and reports the MEDIAN wall-clock time
# and MEDIAN peak resident set size (RSS), plus tile count and output size.
# Built for gating a change: run it on the old binary, run it on the new one,
# compare. Time AND memory both matter — a faster path that regresses peak RSS
# is not an acceptable win.
#
# Usage:
#   docs/benchmark/measure.sh <LABEL> <INPUT> <OUTPUT.{pmtiles|mbtiles}> [-- <extra massif args>]
#
# Example:
#   docs/benchmark/measure.sh cz_pm_z5-10 czech-republic.tif out.pmtiles \
#       -- --encoding mapbox --format webp --compress 6 --min-z 5 --max-z 10 -j 8
#
# Env vars:
#   RUNS      repetitions, median reported          (default 3)
#   MASSIF    path to the massif binary             (default ./target/release/massif)
#   RESULTS   CSV appended to across invocations     (default docs/benchmark/measure.csv)
#
# Peak RSS: macOS via `/usr/bin/time -l`, Linux via `/usr/bin/time -v`.
set -euo pipefail

LABEL="${1:?Usage: measure.sh <LABEL> <INPUT> <OUTPUT> [-- massif args]}"
INPUT="${2:?missing INPUT raster}"
OUTPUT="${3:?missing OUTPUT file (.pmtiles or .mbtiles)}"
shift 3
if [ "${1:-}" = "--" ]; then shift; fi
EXTRA_ARGS=("$@")

RUNS="${RUNS:-3}"
MASSIF="${MASSIF:-./target/release/massif}"
RESULTS="${RESULTS:-docs/benchmark/measure.csv}"
OS="$(uname)"

container="${OUTPUT##*.}"

[ -f "$INPUT" ]  || { echo "Error: input not found: $INPUT" >&2; exit 1; }
[ -x "$MASSIF" ] || { echo "Error: massif binary not found/executable: $MASSIF" >&2; exit 1; }

# ── time-string → seconds (handles s, m:ss, h:mm:ss) ─────────────────────────
to_seconds() {
  awk -v t="$1" 'BEGIN{
    n=split(t,a,":");
    if(n==1) print a[1]+0;
    else if(n==2) print a[1]*60 + a[2];
    else print a[1]*3600 + a[2]*60 + a[3];
  }'
}

# ── median of a whitespace-separated list of numbers ────────────────────────
median() {
  printf '%s\n' "$@" | sort -n | awk '{v[NR]=$1} END{
    if(NR%2) printf "%.2f", v[(NR+1)/2];
    else printf "%.2f", (v[NR/2]+v[NR/2+1])/2;
  }'
}

# ── one run: sets REAL_SEC, RSS_MB, and leaves stderr in $STDERR_FILE ────────
run_once() {
  STDERR_FILE="$(mktemp)"
  if [ "$OS" = "Darwin" ]; then
    { /usr/bin/time -l "$MASSIF" "${EXTRA_ARGS[@]}" "$INPUT" "$OUTPUT" >/dev/null; } 2>"$STDERR_FILE" || {
      echo "  run failed — massif stderr tail:" >&2; tail -5 "$STDERR_FILE" >&2; exit 1; }
    REAL_SEC="$(awk '{for(i=1;i<=NF;i++) if($i=="real") print $(i-1)}' "$STDERR_FILE" | tail -1)"
    local rss_bytes; rss_bytes="$(awk '/maximum resident set size/{print $1}' "$STDERR_FILE" | tail -1)"
    RSS_MB="$(awk -v b="${rss_bytes:-0}" 'BEGIN{printf "%.1f", b/1048576}')"
  else
    { /usr/bin/time -v "$MASSIF" "${EXTRA_ARGS[@]}" "$INPUT" "$OUTPUT" >/dev/null; } 2>"$STDERR_FILE" || {
      echo "  run failed — massif stderr tail:" >&2; tail -5 "$STDERR_FILE" >&2; exit 1; }
    local elapsed; elapsed="$(awk -F': ' '/Elapsed \(wall clock\)/{print $2}' "$STDERR_FILE" | tail -1)"
    REAL_SEC="$(to_seconds "$elapsed")"
    local rss_kb; rss_kb="$(awk -F': ' '/Maximum resident set size/{print $2}' "$STDERR_FILE" | tail -1)"
    RSS_MB="$(awk -v k="${rss_kb:-0}" 'BEGIN{printf "%.1f", k/1024}')"
  fi
}

echo "════════════════════════════════════════════════════════════════"
echo " measure: $LABEL"
echo " input:   $INPUT   → $OUTPUT ($container)"
echo " args:    ${EXTRA_ARGS[*]}"
echo " runs:    $RUNS (median reported)   os: $OS"
echo "════════════════════════════════════════════════════════════════"

times=(); rsss=(); tile_count=0
for i in $(seq 1 "$RUNS"); do
  rm -f "$OUTPUT"
  run_once
  # tile count from massif's own stderr line ("N non-empty tiles written")
  tc="$(grep -oE '[0-9]+ non-empty tiles written' "$STDERR_FILE" | grep -oE '^[0-9]+' || echo 0)"
  [ "${tc:-0}" -gt 0 ] && tile_count="$tc"
  rm -f "$STDERR_FILE"
  printf "  run %d/%d:  %6ss   %8s MB   tiles=%s\n" "$i" "$RUNS" "$REAL_SEC" "$RSS_MB" "$tile_count"
  times+=("$REAL_SEC"); rsss+=("$RSS_MB")
done

med_time="$(median "${times[@]}")"
med_rss="$(median "${rsss[@]}")"
size_bytes="$(stat -f%z "$OUTPUT" 2>/dev/null || stat -c%s "$OUTPUT" 2>/dev/null || echo 0)"
size_mb="$(awk -v b="$size_bytes" 'BEGIN{printf "%.1f", b/1048576}')"

echo "────────────────────────────────────────────────────────────────"
printf " MEDIAN  time=%ss   peakRSS=%s MB   tiles=%s   size=%s MB\n" \
  "$med_time" "$med_rss" "$tile_count" "$size_mb"
echo "────────────────────────────────────────────────────────────────"

# ── append to CSV (create header once) ──────────────────────────────────────
if [ ! -f "$RESULTS" ]; then
  echo "label,container,args,runs,median_time_sec,median_peak_rss_mb,tile_count,size_mb" > "$RESULTS"
fi
echo "\"$LABEL\",$container,\"${EXTRA_ARGS[*]}\",$RUNS,$med_time,$med_rss,$tile_count,$size_mb" >> "$RESULTS"
echo " appended → $RESULTS"
