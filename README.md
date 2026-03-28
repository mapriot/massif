# massif

Fast terrain-RGB PMTiles generator from Float32 elevation rasters.

Converts GeoTIFF, VRT, or any GDAL-supported elevation raster (DEM, DSM, DTM) into [Mapbox terrain-RGB](https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-rgb-v1/) encoded [PMTiles](https://protomaps.com/docs/pmtiles), ready to use with MapLibre GL for hillshading and 3D terrain.

Built as a fast Rust replacement for [rio-rgbify](https://github.com/mapbox/rio-rgbify). Uses all CPU cores with zero per-tile Python overhead.

## Performance

Tested on a 7.7 GB Float32 GeoTIFF (Indonesia, zoom 5–12, ~142K tiles, 14 threads, flags: `-b -10000 -i 0.1 -r 3 --min-z 5 --max-z 12 -j 14`):

| Command | Time | Output size |
|---------|------|-------------|
| `massif` (no `--compress`) | 2:30 | 4 560 MB |
| `massif --compress 6` | ~6:30 | ~2 844 MB |
| `massif --compress 9` | 12:35 | 2 828 MB |
| `rio-rgbify` | 10:21 | 2 947 MB |

`--compress 6` is the recommended default for production: ~38% smaller than no compression, still 1.5× faster than rio-rgbify.

## Installation

### Prerequisites

massif requires GDAL to be installed on your system.

**macOS**
```bash
brew install gdal
```

**Ubuntu / Debian**
```bash
sudo apt install libgdal-dev gdal-bin
```

**Fedora / RHEL**
```bash
sudo dnf install gdal-devel
```

**Windows**

Install GDAL via [OSGeo4W](https://trac.osgeo.org/osgeo4w/) or [Conda](https://anaconda.org/conda-forge/gdal), then ensure `gdal-config` is on your PATH.

### Install massif

**From crates.io** (once published)
```bash
cargo install massif
```

**From source**
```bash
git clone https://github.com/mapriot/massif
cd massif
cargo build --release
# Binary is at target/release/massif
sudo cp target/release/massif /usr/local/bin/
```

On macOS you may need to set `PKG_CONFIG_PATH` if GDAL was installed via Homebrew:
```bash
PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig" cargo build --release
```

## Usage

```
massif [OPTIONS] <INPUT> <OUTPUT>

Arguments:
  <INPUT>   Float32 elevation raster — GeoTIFF, VRT, or any GDAL-supported format
  <OUTPUT>  Output PMTiles file path

Options:
  -b, --base-val <FLOAT>       Base elevation offset [default: -10000]
  -i, --interval <FLOAT>       Elevation interval / precision in metres [default: 0.1]
  -r, --round-digits <INT>     Zero out the lowest N bits of the encoded value [default: 3]
      --min-z <INT>            Minimum zoom level [default: 5]
      --max-z <INT>            Maximum zoom level [default: 12]
      --format <FORMAT>        Tile image format: webp, png [default: webp]
      --compress <LEVEL>       Compression level 1–9 (omit for fastest)
  -j, --workers <INT>          Thread count [default: all CPUs]
  -h, --help                   Print help
```

### Recommended production command

```bash
massif -b -10000 -i 0.1 -r 3 --min-z 5 --max-z 12 --compress 6 input.tif output.pmtiles
```

### Examples

Fastest output — no extra compression (good for iteration/preview):
```bash
massif input.tif output.pmtiles
```

Production — balanced size and speed:
```bash
massif -b -10000 -i 0.1 -r 3 --min-z 5 --max-z 12 --compress 6 input.tif output.pmtiles
```

PNG tiles (larger than WebP, but universally supported):
```bash
massif -b -10000 -i 0.1 -r 3 --min-z 5 --max-z 12 --format png input.tif output.pmtiles
```

Maximum compression (diminishing returns past 6):
```bash
massif -b -10000 -i 0.1 -r 3 --min-z 5 --max-z 12 --compress 9 input.tif output.pmtiles
```

## Compression levels

`--compress` is a format-agnostic 1–9 scale (like gzip/zstd). Currently maps to libwebp lossless compression effort.

| Level | WebP effort | Size vs no flag | Time vs no flag |
|-------|-------------|-----------------|-----------------|
| 1 | 0 | −24% | ~1.8× |
| 5 | 50 | −38% | ~2.4× |
| **6** | **63** | **−38%** | **~2.6×** ← recommended |
| 7 | 75 | −38% | ~2.8× |
| 9 | 100 | −38% | ~5× |

The size curve flattens sharply after level 5 — levels 5–9 all produce nearly identical file sizes, but time keeps growing. Level 6 sits just past the knee of the curve.

Omitting `--compress` uses a different encoder (pure-Rust image-webp) that is significantly faster but produces larger files. Use it when speed matters more than storage: local testing, ephemeral previews, or when you'll re-encode later.

## Input formats

Any raster format supported by GDAL works as input — GeoTIFF (`.tif`), GDAL Virtual Raster (`.vrt`), HGT, IMG, and more. The input can be in any CRS; massif uses GDAL's coordinate transform to reproject each tile to Web Mercator (EPSG:3857) on the fly.

Common sources:
- [Copernicus DEM](https://spacedata.copernicus.eu/collections/copernicus-digital-elevation-model) (GLO-30, GLO-90)
- [SRTM](https://www.usgs.gov/centers/eros/science/usgs-eros-archive-digital-elevation-shuttle-radar-topography-mission-srtm)
- [ALOS World 3D](https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm)
- Any Float32 GeoTIFF with elevation values in metres

## Encoding

massif uses the [Mapbox terrain-RGB encoding](https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-rgb-v1/):

```
encoded = floor((elevation - base_val) / interval)
R = (encoded >> 16) & 0xFF
G = (encoded >> 8)  & 0xFF
B =  encoded        & 0xFF
```

MapLibre decodes this as:
```
height = base_val + (R × 65536 + G × 256 + B) × interval
```

With the defaults (`base_val=-10000`, `interval=0.1`), the encodable range is −10 000 m to +1 677 721.5 m at 0.1 m precision. The `--round-digits` flag zeroes out the lowest N bits of the encoded integer, reducing entropy and improving compression without significant quality loss for hillshading.

Nodata pixels (and any pixels outside the input extent) are encoded as `R=0, G=0, B=0`. Tiles that are entirely nodata are omitted from the output.

Output tiles are 512×512 pixels in PMTiles v3 format. Default tile format is WebP lossless; use `--format png` for PNG.

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

## Roadmap

- [ ] Terrarium encoding (`--encoding terrarium`)
- [ ] MBTiles output (inferred from `.mbtiles` extension)
- [ ] Pre-built binaries via GitHub Releases

## License

MIT — see [LICENSE](LICENSE)
