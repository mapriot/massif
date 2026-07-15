# massif

Fast terrain-RGB tile generator from elevation rasters.

Converts GeoTIFF, VRT, or any GDAL-supported elevation raster into Mapbox or Terrarium terrain-RGB tiles, packaged as [PMTiles](https://protomaps.com/docs/pmtiles) or [MBTiles](https://wiki.openstreetmap.org/wiki/MBTiles). Ready to use with MapLibre GL for hillshading and 3D terrain.

Built as a fast Rust replacement for [rio-rgbify](https://github.com/mapbox/rio-rgbify). Uses all CPU cores via [rayon](https://github.com/rayon-rs/rayon), prunes empty ocean/nodata regions instead of reading them, shows real-time progress, and outputs to modern tile containers — no Python overhead, no guessing when it'll finish.

## Installation

Three ways to get massif, easiest first.

### Docker — no GDAL or Rust needed

Only Docker is required. Build the image once, then run it:

```bash
docker build -t massif .
docker run --rm -v "$PWD:/data" massif --compress 6 /data/input.tif /data/output.pmtiles
```

Bind-mount the directory holding your rasters (and where output goes) at `/data`; paths are resolved inside the container. For a VRT, mount the directory containing both the `.vrt` and every `.tif` it references.

### From crates.io

Needs GDAL installed — see [prerequisites](#gdal-prerequisites) below.

```bash
cargo install massif
```

### From source

```bash
git clone https://github.com/mapriot/massif
cd massif
cargo build --release   # binary at target/release/massif
```

On macOS with Homebrew GDAL you may need `PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig" cargo build --release`.

### GDAL prerequisites

Native builds (crates.io / source) need GDAL on your system:

| Platform | Command |
|---|---|
| macOS | `brew install gdal` |
| Ubuntu / Debian | `sudo apt install libgdal-dev gdal-bin` |
| Fedora / RHEL | `sudo dnf install gdal-devel` |
| Windows | [OSGeo4W](https://trac.osgeo.org/osgeo4w/) or [Conda](https://anaconda.org/conda-forge/gdal) — ensure `gdal-config` is on your PATH *(untested)* |

**Supported GDAL versions: 3.5–3.12** — the versions the `gdal` Rust crate ships bindings for. GDAL **3.13 and newer are not yet supported** and will fail to build. If your package manager installs 3.13+ (current Homebrew `gdal` does), install a supported build via conda-forge — `conda install -c conda-forge 'gdal>=3.5,<3.13'` — or just use Docker, which pins a supported GDAL for you.

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

### Options

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

### Large, ocean-heavy builds

For big extents with lots of empty area — coastlines, open ocean, gaps between countries in a VRT — massif's **MBTiles** path automatically uses a **sparse frontier**: it walks the tile pyramid and prunes whole nodata subtrees instead of reading every candidate tile. It's automatic (nothing to enable), scales with how empty the extent is, and produces byte-identical output.

PMTiles needs Hilbert-ordered tiles, so it uses the flat path. To get a PMTiles *with* the frontier's speed, generate MBTiles and convert it with the [`pmtiles`](https://github.com/protomaps/go-pmtiles) CLI:

```bash
massif --compress 6 input.tif output.mbtiles
pmtiles convert output.mbtiles output.pmtiles
```

The convert step is a fast bulk repack (seconds) that produces the same Hilbert-clustered PMTiles. On an Indonesia z5–10 build this two-step path is ~2× faster end-to-end than generating PMTiles directly (≈42 s vs ≈90 s), at lower peak memory. For small or land-dense extents, writing `.pmtiles` directly is simplest and just as fast.

## Input

Any raster GDAL can read — GeoTIFF (`.tif`), Virtual Raster (`.vrt`), HGT, IMG, and more. Any pixel data type works (Float32, Float64, Int16, UInt16, etc.); GDAL converts to Float32 internally. The input can be in any CRS — massif reprojects each tile to Web Mercator on the fly, and skips the transform entirely for EPSG:4326 (about 2.5× faster), so run `gdalwarp -t_srs EPSG:4326` first if you can.

Common elevation data sources:
- [ALOS World 3D](https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm)
- [SRTM](https://www.usgs.gov/centers/eros/science/usgs-eros-archive-digital-elevation-shuttle-radar-topography-mission-srtm)
- [Copernicus DEM](https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM) (GLO-30, GLO-90)

**Overviews (recommended for single large TIFs).** Precomputed downsampled pyramids let massif read low-zoom tiles cheaply instead of resampling the full-resolution data each time — 20–40% faster:

```bash
# Single TIF — writes a sidecar .ovr file, does not modify the input
gdaladdo -ro -r average input.tif 2 4 8 16 32 64 128 256

# VRT — same approach, creates merged.vrt.ovr
gdaladdo -ro -r average merged.vrt 2 4 8 16 32 64 128 256
```

massif (via GDAL) picks up the `.ovr` sidecar automatically. The tradeoff is storage — the `.ovr` can be as large as the source data. If disk is tight, skip overviews; massif handles it, just slower.

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

## Performance

massif saturates all cores and, on large empty-heavy extents, prunes nodata regions instead of reading them. Highlights:

- **66 GB VRT — Europe + Oceania, 70 TIFs, z5–12:** 4h00m with `--compress 6` on a 20-thread Xeon under production load. rio-rgbify did not finish in 48h on the same machine and dataset.
- **Sparse frontier (0.2.0):** a multi-country VRT (Czech + Turkey + Indonesia, z5–10) builds in **3m33s vs 5m49s** on the flat path (20-thread Xeon) — ~1.6× faster, byte-identical output, lower peak memory. The win grows with how empty the bounding box is, up to ~2.2× on an ocean-heavy single file.

Quick tuning guide:

| Setting | Impact | Notes |
|---|---|---|
| EPSG:4326 input | **~2.5× faster** (no compress) | massif skips GDAL transforms entirely; use `gdalwarp -t_srs EPSG:4326` |
| GDAL overviews | **−20–40%** time | Effective for single TIFs; `.ovr` can match source file size |
| WebP vs PNG | WebP is **~2× smaller** | Use PNG only if the client can't display WebP |
| `--compress 6` | **−38%** size vs none | Best size/speed tradeoff; gains flatten past 5 |
| `-r 3` (default) | **−43%** size vs r=0 | Biggest lever for file size; invisible for hillshading at most latitudes |
| Terrarium vs Mapbox | Terrarium is **~3.1× larger** | No round-digits equivalent; prefer Mapbox |

Full methodology, per-machine real-world timings, and all 36 parameter combinations: [docs/benchmarks.md](docs/benchmarks.md).

## License

MIT — see [LICENSE](LICENSE)
