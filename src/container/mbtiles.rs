use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use crate::tile_format::TileFormat;

/// MBTiles writer.
///
/// MBTiles uses TMS tile ordering: y=0 is at the *south*.
/// XYZ → TMS: tms_y = (2^z − 1) − xyz_y
///
/// Tiles are inserted inside a single transaction that is committed on finalize()
/// for dramatically better write throughput vs one transaction per tile.
pub struct MbtilesWriter {
    conn: Connection,
}

impl MbtilesWriter {
    pub fn create(path: &Path, format: TileFormat, min_z: u8, max_z: u8) -> Result<Self> {
        if path.exists() {
            std::fs::remove_file(path)
                .with_context(|| format!("remove existing {:?}", path))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open {:?}", path))?;

        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE metadata (name TEXT NOT NULL, value TEXT);
            CREATE TABLE tiles (
                zoom_level  INTEGER NOT NULL,
                tile_column INTEGER NOT NULL,
                tile_row    INTEGER NOT NULL,
                tile_data   BLOB    NOT NULL,
                PRIMARY KEY (zoom_level, tile_column, tile_row)
            );
        ").context("create MBTiles schema")?;

        let mime = match format {
            TileFormat::Webp => "image/webp",
            TileFormat::Png  => "image/png",
        };

        {
            let mut stmt = conn.prepare(
                "INSERT INTO metadata (name, value) VALUES (?1, ?2)"
            ).context("prepare metadata insert")?;
            for (k, v) in [
                ("name",    "massif"),
                ("format",  mime),
                ("type",    "baselayer"),
                ("version", "1.1"),
                ("minzoom", &min_z.to_string()),
                ("maxzoom", &max_z.to_string()),
            ] {
                stmt.execute(params![k, v]).context("insert metadata")?;
            }
        }

        // Begin the long-running write transaction
        conn.execute_batch("BEGIN").context("begin transaction")?;

        Ok(Self { conn })
    }

    pub fn add_tile(&mut self, z: u8, x: u32, y_xyz: u32, data: &[u8]) -> Result<()> {
        // Flip y from XYZ (north=0) to TMS (south=0)
        let tms_y = (1u32 << z).wrapping_sub(1).wrapping_sub(y_xyz);
        self.conn.execute(
            "INSERT OR REPLACE INTO tiles (zoom_level, tile_column, tile_row, tile_data)
             VALUES (?1, ?2, ?3, ?4)",
            params![z, x, tms_y, data],
        ).context("insert tile")?;
        Ok(())
    }

    pub fn finalize(self) -> Result<()> {
        self.conn.execute_batch("
            COMMIT;
            CREATE UNIQUE INDEX tiles_idx ON tiles (zoom_level, tile_column, tile_row);
        ").context("finalize MBTiles")?;
        Ok(())
    }
}
