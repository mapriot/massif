use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use pmtiles::{PmTilesStreamWriter, PmTilesWriter, TileCoord, TileType};

use crate::tile_format::Format;

pub struct PmtilesWriter {
    inner: PmTilesStreamWriter<File>,
}

impl PmtilesWriter {
    pub fn create(path: &Path, format: Format, min_z: u8, max_z: u8) -> Result<Self> {
        let tile_type = match format {
            Format::Webp => TileType::Webp,
            Format::Png => TileType::Png,
        };
        let f = File::create(path)
            .with_context(|| format!("create {:?}", path))?;
        let inner = PmTilesWriter::new(tile_type)
            .min_zoom(min_z)
            .max_zoom(max_z)
            .create(f)
            .context("create PMTiles writer")?;
        Ok(Self { inner })
    }

    pub fn add_tile(&mut self, z: u8, x: u32, y_xyz: u32, data: &[u8]) -> Result<()> {
        let coord = TileCoord::new(z, x, y_xyz).context("TileCoord")?;
        self.inner.add_tile(coord, data).context("add_tile")
    }

    pub fn finalize(self) -> Result<()> {
        self.inner.finalize().context("finalize PMTiles")
    }
}
