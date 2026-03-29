# Benchmarks

Exhaustive parameter sweep across all combinations of format, compression, encoding, round digits, and container type.

## Methodology

Each run produces a full tile set from zoom 5–10 (small) or 5–12 (large). We measure wall-clock time, output file size, tile count, and per-tile sizes. The benchmark script is at [`benchmark/bench.sh`](benchmark/bench.sh).

**Parameter matrix:**
- **Format:** WebP, PNG
- **Compression:** none, 1, 3, 6, 9
- **Encoding:** Mapbox, Terrarium
- **Round digits (Mapbox):** 0, 1, 3, 5, 7
- **Container:** PMTiles, MBTiles

36 runs per dataset.

---

## Small dataset

**Input:** sample.tif (small test raster)
**Zoom:** 5–10, 536 tiles
**Hardware:** Apple M4 Pro, 48 GB RAM, 14 threads

### Format and compression (Mapbox, r=3)

| Format | Compress | MBTiles | PMTiles | Avg tile | Time |
|---|---|---|---|---|---|
| WebP | none | 59.5 MB | 59.1 MB | 116 KB | 2s |
| WebP | 1 | 45.8 MB | 45.4 MB | 90 KB | 3s |
| WebP | 3 | 41.6 MB | 41.2 MB | 81 KB | 4s |
| WebP | 6 | 36.6 MB | 36.2 MB | 71 KB | 5s |
| WebP | 9 | 36.2 MB | 35.8 MB | 71 KB | 9s |
| PNG | none | 148.2 MB | 147.7 MB | 290 KB | 2s |
| PNG | 1 | 148.2 MB | 147.7 MB | 290 KB | 2s |
| PNG | 3 | 148.2 MB | 147.7 MB | 290 KB | 1s |
| PNG | 6 | 62.0 MB | 61.6 MB | 121 KB | 2s |
| PNG | 9 | 57.8 MB | 57.4 MB | 113 KB | 5s |

> Note: PNG compress 1–3 produced identical output to no compression in this run (pre-fix). After the fix in v0.1.0, compress 1–3 now maps to `Default` compression and will produce smaller files.

### Encoding comparison

| Encoding | Format | Compress | MBTiles | PMTiles | Avg tile |
|---|---|---|---|---|---|
| Mapbox (r=3) | WebP | none | 59.5 MB | 59.1 MB | 116 KB |
| Terrarium | WebP | none | 186.9 MB | 186.4 MB | 365 KB |
| Mapbox (r=3) | WebP | 6 | 36.6 MB | 36.2 MB | 71 KB |
| Terrarium | WebP | 6 | 124.6 MB | 124.1 MB | 243 KB |
| Mapbox (r=3) | PNG | none | 148.2 MB | 147.7 MB | 290 KB |
| Terrarium | PNG | none | 214.4 MB | 213.8 MB | 418 KB |
| Mapbox (r=3) | PNG | 6 | 62.0 MB | 61.6 MB | 121 KB |
| Terrarium | PNG | 6 | 182.7 MB | 182.2 MB | 357 KB |

### Round digits impact (Mapbox, WebP, compress 6)

| `-r` | MBTiles | PMTiles | Avg tile | vs r=0 |
|---|---|---|---|---|
| 0 | 63.2 MB | 62.7 MB | 123 KB | — |
| 1 | 53.0 MB | 52.6 MB | 103 KB | −16% |
| 3 | 36.6 MB | 36.2 MB | 71 KB | −42% |
| 5 | 24.5 MB | 24.1 MB | 47 KB | −61% |
| 7 | 12.7 MB | 12.3 MB | 24 KB | −80% |

---

## Large dataset

**Input:** 7.7 GB Float32 GeoTIFF (Indonesia)
**Zoom:** 5–12, 67,807 tiles
**Hardware:** Apple M4 Pro, 48 GB RAM, 14 threads

### Format and compression (Mapbox, r=3)

| Format | Compress | PMTiles | Time | Avg tile | vs WebP none |
|---|---|---|---|---|---|
| WebP | none | 4,560 MB | 2:25 | 70.5 KB | — |
| WebP | 1 | 3,462 MB | 4:44 | 53.5 KB | −24% |
| WebP | 3 | 3,235 MB | 5:18 | 50.0 KB | −29% |
| **WebP** | **6** | **2,844 MB** | **6:29** | **44.0 KB** | **−38%** |
| WebP | 9 | 2,828 MB | 11:55 | 43.7 KB | −38% |
| PNG | none | 9,776 MB | 2:26 | 151 KB | +114% |
| PNG | 1 | 4,664 MB | 4:16 | 72.1 KB | +2% |
| PNG | 3 | 4,664 MB | 4:06 | 72.1 KB | +2% |
| PNG | 6 | 4,393 MB | 8:46 | 67.9 KB | −4% |
| PNG | 9 | 4,393 MB | 8:41 | 67.9 KB | −4% |

PMTiles sizes shown (MBTiles adds ~1% overhead from SQLite).

**Key observations:**
- WebP compress 6 is the clear winner: 2,844 MB in 6:29
- WebP compress 9 saves only 16 MB (0.6%) over compress 6 but takes almost twice as long
- PNG compress 1–3 are identical (both map to `Default` compression level)
- PNG compress 6–9 are identical (both map to `Best`)
- Uncompressed PNG is 2.1× larger than uncompressed WebP
- Compressed PNG (level 6) is comparable to uncompressed WebP in size, but slower

### Encoding comparison

| Encoding | Format | Compress | PMTiles | Time | Avg tile |
|---|---|---|---|---|---|
| Mapbox (r=3) | WebP | none | 4,560 MB | 2:25 | 70.5 KB |
| Terrarium | WebP | none | 12,606 MB | 2:29 | 195 KB |
| Mapbox (r=3) | WebP | 6 | 2,844 MB | 6:29 | 44.0 KB |
| Terrarium | WebP | 6 | 8,861 MB | 7:45 | 137 KB |
| Mapbox (r=3) | PNG | none | 9,776 MB | 2:26 | 151 KB |
| Terrarium | PNG | none | 14,194 MB | 2:32 | 219 KB |
| Mapbox (r=3) | PNG | 6 | 4,393 MB | 8:46 | 67.9 KB |
| Terrarium | PNG | 6 | 12,373 MB | 3:58 | 191 KB |

Terrarium produces **3.1× larger** output than Mapbox (r=3) with WebP compress 6. The difference is less dramatic with PNG but still substantial (2.8×).

### Round digits impact (Mapbox, WebP, compress 6)

| `-r` | PMTiles | Time | Avg tile | vs r=0 |
|---|---|---|---|---|
| 0 | 4,964 MB | 7:21 | 76.8 KB | — |
| 1 | 4,239 MB | 6:57 | 65.6 KB | −15% |
| 3 | 2,844 MB | 6:29 | 44.0 KB | −43% |
| 5 | 1,849 MB | 5:52 | 28.6 KB | −63% |
| 7 | 885 MB | 4:31 | 13.7 KB | −82% |

Round digits is the single largest lever for output file size. Higher `-r` values also compress faster (less entropy = less work for the encoder).

**Latitude caveat:** r=3 produces visible artifacts at high northern latitudes (northern Norway, Svalbard, Greenland, etc) where subtle elevation gradients get quantized away. For polar or sub-polar regions, use r=1 or r=0.

---

## Summary

Results are consistent between the small (536 tile) and large (67,807 tile) datasets. The ratios hold regardless of dataset size:

| Finding | Small | Large |
|---|---|---|
| WebP compress 6 vs none | −38% | −38% |
| WebP compress 9 vs 6 | −1% | −0.6% |
| PNG vs WebP (no compress) | +2.5× | +2.1× |
| Terrarium vs Mapbox (WebP c6) | +3.4× | +3.1× |
| r=3 vs r=0 (WebP c6) | −42% | −43% |
| r=5 vs r=0 (WebP c6) | −61% | −63% |
| MBTiles vs PMTiles overhead | <1% | ~1% |

### Recommended settings

| Use case | Flags | Expected size | Speed |
|---|---|---|---|
| Production (default) | `--compress 6` | smallest practical | ~2.5× baseline |
| Fast iteration / preview | *(no flags)* | ~1.6× production | fastest |
| Maximum size reduction | `--compress 6 -r 5` | ~63% smaller than r=0 | ~2.5× baseline |
| High-latitude / precision | `--compress 6 -r 0` | ~1.7× default | ~2.8× baseline |
| PNG (compatibility) | `--format png --compress 1` | ~1.6× WebP c6 | ~1.8× baseline |
