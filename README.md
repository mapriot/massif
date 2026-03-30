# massif

Fast terrain-RGB tile generator from elevation rasters.

Converts GeoTIFF, VRT, or any GDAL-supported elevation raster into Mapbox or Terrarium terrain-RGB tiles, packaged as [PMTiles](https://protomaps.com/docs/pmtiles) or [MBTiles](https://wiki.openstreetmap.org/wiki/MBTiles). Ready to use with MapLibre GL for hillshading and 3D terrain.

Built as a fast Rust replacement for [rio-rgbify](https://github.com/mapbox/rio-rgbify). Uses all CPU cores via [rayon](https://github.com/rayon-rs/rayon), shows real-time progress, and outputs to modern tile containers — no Python overhead, no guessing when it'll finish.

## Installation

### Prerequisites

GDAL must be installed on your system.

| Platform | Command |
|---|---|
| macOS | `brew install gdal` |
| Ubuntu / Debian | `sudo apt install libgdal-dev gdal-bin` |
| Fedora / RHEL | `sudo dnf install gdal-devel` |
| Windows | [OSGeo4W](https://trac.osgeo.org/osgeo4w/) or [Conda](https://anaconda.org/conda-forge/gdal) — ensure `gdal-config` is on your PATH *(untested)* |

### Install massif

**From crates.io**
```bash
cargo install massif
```

**From source**
```bash
git clone https://github.com/mapriot/massif
cd massif
cargo build --release
# Binary is at target/release/massif
```

On macOS with Homebrew GDAL you may need:
```bash
PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig" cargo build --release
```

## Usage

```
massif [OPTIONS] <INPUT> <OUTPUT>
```

`INPUT` is any GDAL-supported elevation raster (GeoTIFF, VRT, HGT, etc., any CRS).
`OUTPUT` is `.pmtiles` or `.mbtiles` — the container format is inferred from the extension.

### Quick start

```bash
# Fastest — preview / iteration (WebP, no extra compression)
massif input.tif output.pmtiles

# Production — good balance of size and speed
massif --compress 6 input.tif output.pmtiles

# MBTiles output — same flags, different extension
massif --compress 6 input.tif output.mbtiles

# Terrarium encoding
massif --encoding terrarium --compress 6 input.tif output.pmtiles

# PNG tiles
massif --format png --compress 6 input.tif output.pmtiles

# Maximum compression for smallest files (diminishing returns past r=5)
massif --compress 6 -r 5 input.tif output.pmtiles
```

### All options

| Flag | Default | Description |
|---|---|---|
| `--encoding` | `mapbox` | RGB encoding: `mapbox` or `terrarium` |
| `--format` | `webp` | Tile image format: `webp` or `png` |
| `--compress` | *(omitted)* | Compression effort 1–9; omit for fastest |
| `--min-z` | `5` | Minimum zoom level |
| `--max-z` | `12` | Maximum zoom level |
| `--nodata` | *(from raster)* | Override nodata value (e.g. `0`, `-9999`, `-32768`) |
| `-j, --workers` | all CPUs | Thread count |

**Mapbox encoding only:**

| Flag | Default | Description |
|---|---|---|
| `-b, --base-val` | `-10000` | Base elevation offset |
| `-i, --interval` | `0.1` | Elevation precision in metres |
| `-r, --round-digits` | `3` | Zero out lowest N bits of encoded value (reduces entropy) |

## Input preparation

**GDAL overviews** precompute downsampled versions of your raster so massif can read low-zoom tiles cheaply instead of resampling the full-resolution data each time. This reduces processing time by 20–40%.

```bash
# Single TIF — writes a sidecar .ovr file, does not modify the input
gdaladdo -ro -r average input.tif 2 4 8 16 32 64 128 256

# VRT — same approach, creates merged.vrt.ovr
gdaladdo -ro -r average merged.vrt 2 4 8 16 32 64 128 256
```

Massif (via GDAL) picks up the `.ovr` sidecar automatically. The tradeoff is storage: the `.ovr` file can be as large as the source data itself. If disk space is constrained, skip overviews and run without — massif handles it, just slower.

## Performance

### Single large TIF — 7.2 GB (Indonesia, zoom 5–12, ~142K tiles)

| Machine | Overviews | Command | Time | Output |
|---|---|---|---|---|
| Apple M4 Pro, 14 threads | no | `massif` | 2:30 | 4,560 MB |
| Apple M4 Pro, 14 threads | yes | `massif` | 1:28 | 4,560 MB |
| Apple M4 Pro, 14 threads | no | `massif --compress 6` | 6:29 | 2,844 MB |
| Apple M4 Pro, 14 threads | yes | `massif --compress 6` | 5:35 | 2,844 MB |
| Xeon Silver 4210, 20 threads | no | `massif` | 7:20 | 4,560 MB |
| Xeon Silver 4210, 20 threads | yes | `massif` | 5:42 | 4,560 MB |
| Xeon Silver 4210, 20 threads | no | `massif --compress 6` | 16:21 | 2,844 MB |
| Xeon Silver 4210, 20 threads | yes | `massif --compress 6` | 12:44 | 2,844 MB |
| Xeon Silver 4210, 20 threads | no | `rio-rgbify` | 25:51 | ~2,810 MB |

### VRT of 70 TIFs — 66 GB total (Europe + Oceania, zoom 5–12)

| Machine | Command | Time | Output |
|---|---|---|---|
| Xeon Silver 4210, 20 threads | `massif` | **15h 47m** | 48,062 MB |
| Xeon Silver 4210, 20 threads | `rio-rgbify` | DNF after 48h | — |

rio-rgbify did not finish after 48 hours on the same machine and dataset. All massif tiles are 512×512 lossless WebP images. The Xeon results were measured on a server under normal production load — actual times on an idle machine would be lower.

| Setting | Impact | Notes |
|---|---|---|
| GDAL overviews | **−20–40%** time | Effective for single TIFs; `.ovr` can match source file size |
| WebP vs PNG | WebP is **2× smaller** | Use PNG only if client doesn't support WebP |
| `--compress 6` | **−38%** size vs no compression | Best size/speed tradeoff; gains flatten past 5 |
| `-r 3` (default) | **−43%** size vs r=0 | Biggest lever for file size; invisible for hillshading at most latitudes |
| Terrarium vs Mapbox | Terrarium is **3.1× larger** | No round-digits equivalent; use Mapbox when possible |

For full benchmark methodology, all 36 parameter combinations, and recommended settings by use case, see [docs/benchmarks.md](docs/benchmarks.md).

## Encoding schemes

### Mapbox (default)

```
encoded = floor((elevation - base_val) / interval)
R = (encoded >> 16) & 0xFF
G = (encoded >> 8)  & 0xFF
B =  encoded        & 0xFF
```

MapLibre decodes as:
```
height = base_val + (R × 65536 + G × 256 + B) × interval
```

With the defaults (`-b -10000 -i 0.1`), the encodable range is −10,000 m to +1,677,721.5 m at 0.1 m precision. The `-r` flag zeroes the lowest N bits of the encoded integer — this reduces entropy for better compression with negligible quality loss for hillshading. Note: `-r 3` may produce visible artifacts at high latitudes (e.g. northern Norway, Svalbard, Greenland) where elevation gradients are subtle; use `-r 1` or `-r 0` for polar regions.

### Terrarium

```
val = elevation + 32768
R = floor(val / 256)
G = floor(val) mod 256
B = floor(frac(val) × 256)
```

MapLibre decodes as:
```
height = (R × 256 + G + B / 256) − 32768
```

Range: −32,768 m to +32,767.996 m at ~0.004 m precision. Used by Mapzen and many open elevation datasets. No configurable parameters — `-b`, `-i`, and `-r` are ignored with a warning.

## Using with MapLibre GL

```json
{
  "sources": {
    "terrain": {
      "type": "raster-dem",
      "url": "pmtiles://https://example.com/terrain.pmtiles",
      "encoding": "mapbox",
      "tileSize": 512
    }
  },
  "terrain": {
    "source": "terrain",
    "exaggeration": 1.5
  },
  "layers": [
    {
      "id": "hillshading",
      "type": "hillshade",
      "source": "terrain"
    }
  ]
}
```

For Terrarium output, set `"encoding": "terrarium"` in the source.

## Input formats

Any raster supported by GDAL — GeoTIFF (`.tif`), Virtual Raster (`.vrt`), HGT, IMG, and more. Any pixel data type works (Float32, Float64, Int16, UInt16, etc.) — GDAL converts to Float32 internally. The input can be in any CRS; massif reprojects each tile to Web Mercator on the fly.

Common elevation data sources:
- [ALOS World 3D](https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm)
- [SRTM](https://www.usgs.gov/centers/eros/science/usgs-eros-archive-digital-elevation-shuttle-radar-topography-mission-srtm)
- [Copernicus DEM](https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM) (GLO-30, GLO-90)

## License

MIT — see [LICENSE](LICENSE)
