//! The module's conformance vectors: fixed requests, the fixture tiles each
//! needs, and the SHA-256 of the section each must produce.
//!
//! Implements the determinism check of `space-model.md` section 4 item 6
//! for this module. `conformance/` holds, per vector, `NAME.request.bin`
//! (the request bytes of `client-compute.md` section 4), `NAME.tiles.bin`
//! (the tile bytes passed to `gx_compile`: the fixture tiles of
//! [`crate::fixture`] for exactly the ids `gx_sources` returns), and
//! `NAME.sha256` (lowercase hex of the expected section), plus `index.json`
//! saying what each covers. The core determinism harness runs the built
//! module over them natively and under Node and requires these hashes from
//! both. [`write`] regenerates the directory with the module's own entry
//! points ([`gx_module_guest::run_sources`] and
//! [`gx_module_guest::run_compile`]).
//!
//! Requests carry origin 0 and span 0, as the hub sends them to a layer
//! with `usesOrigin` false and `spanSeconds` 0.

use std::fmt::Write as _;
use std::path::Path;

use gx_core::module_io::{decode_source_ids, encode_request, encode_tiles, ModuleRequest, Tile};
use gx_core::sha256::{sha256, to_hex};
use gx_module_guest::{run_compile, run_sources};
use terrain_tiles::TileKey;

use crate::{fixture, TerrainModule};

/// One conformance vector.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    /// File name stem.
    pub name: &'static str,
    /// The chunk key.
    pub key: &'static str,
    /// What the vector covers.
    pub why: &'static str,
}

/// The twelve vectors.
pub const VECTORS: [Vector; 12] = [
    v("01-registry", "registry", "the empty registry"),
    v(
        "02-whole-d0",
        "4-0-0-0-0",
        "the whole frame at depth 0, both tiles of the top level",
    ),
    v(
        "03-octant-d3",
        "4-3-4-4-4",
        "an octant from the center to the surface, depth 3",
    ),
    v(
        "04-island-d6",
        "4-6-40-32-32",
        "the surface where the frame's x axis leaves it, depth 6, level 5",
    ),
    v(
        "05-shelf-d9",
        "4-9-320-255-256",
        "island and seafloor across the prime meridian, depth 9, level 2",
    ),
    v(
        "06-interior-d8",
        "4-8-128-128-128",
        "an interior cell at the center, the profile alone, no sources",
    ),
    v(
        "07-coast-d11",
        "4-11-1280-1027-1025",
        "a coastline cell: land and seafloor albedo, depth 11, level 0",
    ),
    v(
        "08-tile-edge-d14",
        "4-14-10242-8191-8209",
        "a cell against the edge between two level 0 tile columns, depth 14",
    ),
    v(
        "09-peak-d16",
        "4-16-40979-32775-32774",
        "the cell holding the fixture summit at 8000 m, depth 16",
    ),
    v(
        "10-trench-d16",
        "4-16-40947-33125-32768",
        "the floor of the fixture trench at -10900 m, depth 16",
    ),
    v(
        "11-surface-d21",
        "4-21-1311018-1047201-1049940",
        "the island's surface at the deepest depth, samples 0.76 m apart",
    ),
    v(
        "12-above-d12",
        "4-12-2565-2045-2050",
        "an empty cell more than 9 km above the surface, no sources",
    ),
];

const fn v(name: &'static str, key: &'static str, why: &'static str) -> Vector {
    Vector { name, key, why }
}

impl Vector {
    /// The request bytes.
    pub fn request(&self) -> Vec<u8> {
        encode_request(&ModuleRequest {
            origin_utc_ms: 0,
            span_seconds: 0,
            span_index: 0,
            key: self.key.parse().expect("conformance keys parse"),
        })
    }

    /// The tile ids the module asks for, through its own `gx_sources`
    /// logic.
    pub fn sources(&self) -> Result<Vec<u64>, String> {
        let ids = run_sources::<TerrainModule>(&self.request())?;
        decode_source_ids(&ids).map_err(|e| e.to_string())
    }

    /// The tile bytes: the fixture tiles of [`Vector::sources`].
    pub fn tiles(&self) -> Result<Vec<u8>, String> {
        let ids = self.sources()?;
        let encoded: Vec<(u64, Vec<u8>)> = ids
            .iter()
            .map(|&id| (id, fixture::tile(id).encode()))
            .collect();
        let tiles: Vec<Tile> = encoded
            .iter()
            .map(|(id, bytes)| Tile { id: *id, bytes })
            .collect();
        encode_tiles(&tiles).map_err(|e| e.to_string())
    }

    /// The section the module returns, computed natively.
    pub fn section(&self) -> Result<Vec<u8>, String> {
        run_compile::<TerrainModule>(&self.request(), &self.tiles()?)
    }
}

/// Every file of the conformance directory, `(file name, bytes)`, in name
/// order.
pub fn files() -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut index = String::from("{\n  \"cases\": {\n");
    for (i, vec) in VECTORS.iter().enumerate() {
        let tiles = vec.tiles()?;
        let hash = to_hex(&sha256(&run_compile::<TerrainModule>(
            &vec.request(),
            &tiles,
        )?));
        out.push((format!("{}.request.bin", vec.name), vec.request()));
        out.push((
            format!("{}.sha256", vec.name),
            format!("{hash}\n").into_bytes(),
        ));
        out.push((format!("{}.tiles.bin", vec.name), tiles));
        let sources: Vec<String> = vec
            .sources()?
            .iter()
            .map(|&id| {
                let k = TileKey::from_id(id).expect("module ids are tile ids");
                format!("\"{}/{}/{}\"", k.level, k.row, k.col)
            })
            .collect();
        let comma = if i + 1 < VECTORS.len() { "," } else { "" };
        let _ = write!(
            index,
            "    \"{}\": {{\n      \"key\": \"{}\",\n      \"origin_utc_ms\": 0,\n      \"sha256\": \"{}\",\n      \"sources\": [{}],\n      \"why\": \"{}\"\n    }}{comma}\n",
            vec.name,
            vec.key,
            hash,
            sources.join(", "),
            vec.why
        );
    }
    index.push_str("  }\n}\n");
    out.push(("index.json".to_string(), index.into_bytes()));
    out.sort();
    Ok(out)
}

/// Writes every file of [`files`] into `dir`, creating it if needed.
pub fn write(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, bytes) in files()? {
        let p = dir.join(name);
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}
