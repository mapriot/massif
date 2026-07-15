use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use pmtiles::{PmTilesStreamWriter, PmTilesWriter, TileCoord, TileType};

use crate::tile_format::TileFormat;

pub struct PmtilesWriter {
    inner: PmTilesStreamWriter<File>,
}

impl PmtilesWriter {
    pub fn create(path: &Path, format: TileFormat, min_z: u8, max_z: u8) -> Result<Self> {
        let tile_type = match format {
            TileFormat::Webp => TileType::Webp,
            TileFormat::Png => TileType::Png,
        };
        let f = File::create(path)
            .with_context(|| format!("create {:?}", path))?;
        let inner = PmTilesWriter::new(tile_type)
            .min_zoom(min_z)
            .max_zoom(max_z)
            // Without this the header's center_zoom defaults to 0, which sits
            // outside our [min_z, max_z] range; point it at the middle zoom.
            .center_zoom(min_z + (max_z - min_z) / 2)
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
