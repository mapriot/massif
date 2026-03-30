# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [0.1.1] - 2026-03-30

### Performance

- Exploit separable Mercator→WGS84 projection for EPSG:4326 inputs: 512+512 coordinate conversions per tile instead of 262,144
- Precompute geo→buffer-pixel mapping per row/col, removing all division and trig from the inner sampling loop
- Eliminate two 2MB Vec allocations per tile on the WGS84 fast path (stack arrays instead)
- Early nodata scan on source buffer skips empty edge tiles before any coordinate work
- Cache GDAL dataset per rayon thread — each worker opens the file once instead of per tile (major win for VRT inputs)
- Skip GDAL coordinate transforms entirely for EPSG:4326 inputs (direct inline math)
- Skip Hilbert sort for MBTiles output (SQLite insertion order doesn't matter)
- Use `sort_by_cached_key` for PMTiles Hilbert sort — computes tile IDs once instead of per comparison

## [0.1.0] - 2026-03-29

Initial release.

### Features

- Convert elevation rasters (GeoTIFF, VRT, any GDAL format, any pixel type) to terrain-RGB tiles
- Mapbox and Terrarium encoding schemes
- WebP and PNG tile formats
- PMTiles v3 and MBTiles output containers (inferred from file extension)
- Dual WebP encoder: fast pure-Rust path (no `--compress`) and libwebp path (`--compress 1-9`)
- Parallel processing via rayon with configurable thread count
- Real-time progress bar with tiles/sec and ETA
- Hilbert-sorted tile output for optimal PMTiles performance
- Chunked processing to bound peak memory usage
- Bilinear resampling with nodata-aware fallback
- Configurable Mapbox encoding parameters (`--base-val`, `--interval`, `--round-digits`)
- Nodata override (`--nodata`) for rasters with missing or incorrect metadata
- Any input CRS — automatic reprojection to Web Mercator via GDAL

[0.1.1]: https://github.com/mapriot/massif/releases/tag/v0.1.1
[0.1.0]: https://github.com/mapriot/massif/releases/tag/v0.1.0
