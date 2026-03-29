pub mod mbtiles;
pub mod pmtiles;

use std::path::Path;

use anyhow::{bail, Result};

use crate::tile_format::TileFormat;
use mbtiles::MbtilesWriter;
use pmtiles::PmtilesWriter;

/// Output container, inferred from the output file extension.
/// `.pmtiles` → PMTiles, `.mbtiles` → MBTiles.
pub enum Writer {
    Pmtiles(PmtilesWriter),
    Mbtiles(MbtilesWriter),
}

impl Writer {
    pub fn open(path: &Path, format: TileFormat, min_z: u8, max_z: u8) -> Result<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("pmtiles") => Ok(Self::Pmtiles(
                PmtilesWriter::create(path, format, min_z, max_z)?,
            )),
            Some("mbtiles") => Ok(Self::Mbtiles(
                MbtilesWriter::create(path, format, min_z, max_z)?,
            )),
            other => bail!(
                "Unknown output extension {:?} — use .pmtiles or .mbtiles",
                other
            ),
        }
    }

    pub fn add_tile(&mut self, z: u8, x: u32, y_xyz: u32, data: &[u8]) -> Result<()> {
        match self {
            Self::Pmtiles(w) => w.add_tile(z, x, y_xyz, data),
            Self::Mbtiles(w) => w.add_tile(z, x, y_xyz, data),
        }
    }

    pub fn finalize(self) -> Result<()> {
        match self {
            Self::Pmtiles(w) => w.finalize(),
            Self::Mbtiles(w) => w.finalize(),
        }
    }
}
