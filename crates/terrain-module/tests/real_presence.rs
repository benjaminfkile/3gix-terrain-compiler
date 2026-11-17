//! The tiles a cell lists cover every tile it reads, over the real
//! source set's presence map (the one the module embeds): fine tiles
//! missing under the oceans fall back to coarser levels whose stencils
//! reach past the cell. The tiles are zero-filled; which tiles a read
//! touches does not depend on their values.

use std::collections::BTreeMap;

use gx_core::key::{CellKey, ChunkKey};
use terrain_module::compile;
use terrain_module::fixture::DEG;
use terrain_module::geodesy::{frame_point, Geodetic};
use terrain_module::plan::{cell_at, GRID};
use terrain_tiles::{Tile, TileKey};

fn check(key: CellKey) {
    let ids = compile::sources(&ChunkKey::Cell(key));
    let tiles: BTreeMap<u64, Tile> = ids
        .iter()
        .map(|&id| {
            let k = TileKey::from_id(id).unwrap();
            (id, Tile::new(k, vec![0; GRID.tile_samples()]).unwrap())
        })
        .collect();
    compile::section(&key, &tiles).unwrap_or_else(|e| panic!("{}: {e}", ChunkKey::Cell(key)));
}

#[test]
fn a_cell_near_the_pole_across_a_level_4_tile_edge() {
    match "4-17-65414-65288-81877".parse::<ChunkKey>().unwrap() {
        ChunkKey::Cell(c) => check(c),
        ChunkKey::Registry | ChunkKey::Surface(_) => unreachable!(),
    }
}

#[test]
fn cells_around_the_globe_read_only_listed_tiles() {
    for i in 0..13 {
        let lat = -89.7 + f64::from(i) * 14.95;
        for j in 0..16 {
            let lon = -179.3 + f64::from(j) * 22.43;
            for depth in [12u8, 15, 17] {
                let p = frame_point(Geodetic {
                    lat: lat * DEG,
                    lon: lon * DEG,
                    h: 0.0,
                });
                check(cell_at(depth, p).unwrap());
            }
        }
    }
}
