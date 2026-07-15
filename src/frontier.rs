//! Sparse-frontier tile enumeration.
//!
//! Instead of enumerating every candidate tile in the bounds at every zoom, we
//! walk the pyramid breadth-first from `min_z`: process a zoom's frontier, and
//! seed the next zoom only with the children of tiles that had data. Whole
//! nodata subtrees (open ocean, gaps in coverage) are pruned before they are
//! ever read.
//!
//! Pruning is driven exclusively by [`crate::raster::TileEval::NoData`], which
//! is prune-safe because a tile's source read window fully contains every
//! descendant's window. This guarantees the set of non-empty tiles produced is
//! identical to the flat path — see the module test and the correctness note in
//! `TileEval`.

use crate::tile::{lat_to_tile_y_xyz, lon_to_tile_x};

/// A tile coordinate awaiting evaluation. Kept tiny (9 bytes) so a full zoom's
/// frontier is cheap to hold in memory even for continental extents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileJob {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// Inclusive tile-index bounds for one zoom level.
#[derive(Clone, Copy, Debug)]
pub struct ZoomBounds {
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
}

impl ZoomBounds {
    #[inline]
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    #[inline]
    pub fn tile_count(&self) -> u64 {
        (self.x1 - self.x0 + 1) as u64 * (self.y1 - self.y0 + 1) as u64
    }
}

/// Tile-index bounds at zoom `z` for the given geographic extent.
pub fn zoom_bounds(west_lon: f64, south_lat: f64, east_lon: f64, north_lat: f64, z: u8) -> ZoomBounds {
    ZoomBounds {
        x0: lon_to_tile_x(west_lon, z),
        x1: lon_to_tile_x(east_lon, z),
        y0: lat_to_tile_y_xyz(north_lat, z), // smaller y = north
        y1: lat_to_tile_y_xyz(south_lat, z),
    }
}

/// Per-zoom bounds for `min_z..=max_z`, indexed by `z - min_z`.
pub fn bounds_by_zoom(
    west_lon: f64,
    south_lat: f64,
    east_lon: f64,
    north_lat: f64,
    min_z: u8,
    max_z: u8,
) -> Vec<ZoomBounds> {
    (min_z..=max_z)
        .map(|z| zoom_bounds(west_lon, south_lat, east_lon, north_lat, z))
        .collect()
}

/// All tiles at `min_z` within bounds — the initial frontier.
pub fn initial_frontier(bounds: &ZoomBounds, min_z: u8) -> Vec<TileJob> {
    let mut frontier = Vec::with_capacity(bounds.tile_count() as usize);
    for x in bounds.x0..=bounds.x1 {
        for y in bounds.y0..=bounds.y1 {
            frontier.push(TileJob { z: min_z, x, y });
        }
    }
    frontier
}

/// Append the in-bounds children of `tile` (at `tile.z + 1`) to `out`.
/// `child_bounds` are the tile-index bounds for the child zoom.
pub fn push_children_in_bounds(tile: TileJob, child_bounds: &ZoomBounds, out: &mut Vec<TileJob>) {
    let z = tile.z + 1;
    let x = tile.x * 2;
    let y = tile.y * 2;
    for (cx, cy) in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
        if child_bounds.contains(cx, cy) {
            out.push(TileJob { z, x: cx, y: cy });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_are_clipped_to_bounds() {
        let cb = ZoomBounds { x0: 2, x1: 3, y0: 4, y1: 5 };
        let mut out = Vec::new();
        // parent (z, 1, 2) → children at (2..=3, 4..=5) — all four in bounds
        push_children_in_bounds(TileJob { z: 4, x: 1, y: 2 }, &cb, &mut out);
        assert_eq!(out.len(), 4);

        // parent whose children fall partly outside bounds
        let cb2 = ZoomBounds { x0: 2, x1: 2, y0: 4, y1: 4 };
        let mut out2 = Vec::new();
        push_children_in_bounds(TileJob { z: 4, x: 1, y: 2 }, &cb2, &mut out2);
        assert_eq!(out2, vec![TileJob { z: 5, x: 2, y: 4 }]);
    }
}
