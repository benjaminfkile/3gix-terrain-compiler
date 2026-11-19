//! The terrain module against the fixture world: the summit, the join
//! across a tile edge, the mass, determinism, other frames, the frame
//! record, and the tiles each cell asks for.

use gx_core::frame_record::{self, FrameRecord};
use gx_core::key::{CellKey, ChunkKey};
use gx_core::matter::{self, Section, State};
use gx_core::module_io::{
    decode_source_ids, encode_inputs, encode_request, encode_tiles, ModuleRequest,
    Tile as ModuleTile,
};
use gx_core::registry;
use gx_core::units::Meters;
use gx_module_guest::{run_compile_v2, run_sources_v2};
use std::collections::BTreeMap;
use terrain_module::{Frame, TerrainModule};

use gx_core::key::CellKey as Key;
use terrain_module::compile::{self, CompileError, LAND_ALBEDO, SEAFLOOR_ALBEDO};
use terrain_tiles::{Presence, Tile};

// The fixture world fills every tile, so these tests read it as a source set
// whose fine tiles all exist.
// The frame is the system module's record of it.
fn sources(key: &ChunkKey) -> Vec<u64> {
    compile::sources_with(&fixture::frame(), key, &Presence::all())
}
fn section(key: &Key, tiles: &BTreeMap<u64, Tile>) -> Result<Section, CompileError> {
    compile::section_with(&fixture::frame(), key, tiles, &Presence::all())
}
fn compile(key: &ChunkKey, tiles: &BTreeMap<u64, Tile>) -> Result<Vec<u8>, CompileError> {
    compile::compile_with(&fixture::frame(), key, tiles, &Presence::all())
}
fn cell_at(depth: u8, p: [f64; 3]) -> Option<CellKey> {
    plan::cell_at(&fixture::frame(), depth, p)
}
use terrain_module::fixture::FRAME_ID;
use terrain_module::fixture::{self, DEG, SUMMIT_ELEVATION};
use terrain_module::frame::RESOLUTION;
use terrain_module::geodesy::{frame_point, geodetic, Geodetic};
use terrain_module::plan::{self, Plan};
use terrain_tiles::TileKey;

/// The registry mass of the frame (the system module's data).
const FRAME_MASS: f64 = 5.972168399787584e24;

fn cell(key: &str) -> CellKey {
    match key.parse::<ChunkKey>().unwrap() {
        ChunkKey::Cell(c) => c,
        _ => panic!("a cell key"),
    }
}

fn build(key: &CellKey) -> Section {
    let tiles = fixture::tiles(&sources(&ChunkKey::Cell(*key)));
    section(key, &tiles).unwrap()
}

fn sample_edge(s: &Section) -> f64 {
    s.edge().value() / f64::from(s.resolution())
}

fn density(s: &Section, x: u32, y: u32, z: u32) -> f64 {
    let n = u32::from(s.resolution());
    let i = (x + n * (y + n * z)) as usize;
    s.samples().unwrap().density(i).value()
}

#[test]
fn summit_is_solid_to_its_elevation_and_vacuum_above() {
    let (lat, lon) = fixture::summit();
    let top = frame_point(Geodetic {
        lat,
        lon,
        h: f64::from(SUMMIT_ELEVATION),
    });
    // The cell holding a point 300 m below the summit, so it holds rock
    // and the air above the summit.
    let below = frame_point(Geodetic {
        lat,
        lon,
        h: f64::from(SUMMIT_ELEVATION) - 300.0,
    });
    let key = cell_at(16, below).unwrap();
    assert_eq!(cell_at(16, top), Some(key), "the summit is in the cell");
    let s = build(&key);
    let n = u32::from(RESOLUTION);
    let e = sample_edge(&s);
    assert!(e < 25.0);
    let o = s.origin();
    // Near the prime meridian and the equator the frame's x axis is the
    // vertical, so the samples at the summit's y and z form a column.
    let iy = ((top[1] - o.y) / e).floor() as u32;
    let iz = ((top[2] - o.z) / e).floor() as u32;
    let solid: Vec<bool> = (0..n).map(|x| density(&s, x, iy, iz) > 0.0).collect();
    let k_top = solid.iter().rposition(|&v| v).expect("rock in the column");
    assert!(solid[..=k_top].iter().all(|&v| v), "solid up to the top");
    let x_top = o.x + e * (k_top + 1) as f64;
    assert!(
        (x_top - top[0]).abs() <= e,
        "rock ends at {x_top}, the summit is at {}",
        top[0]
    );
    // Nothing in the whole cell lies more than a sample above the summit.
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                if o.x + e * f64::from(x) > top[0] + e {
                    assert_eq!(density(&s, x, y, z), 0.0, "{x} {y} {z}");
                }
            }
        }
    }
    // The top sample of the column is land.
    let i = (k_top as u32 + n * (iy + n * iz)) as usize;
    let a = s.samples().unwrap().get(i).unwrap().albedo;
    assert!((a[0].value() - LAND_ALBEDO[0]).abs() < 1e-6);
}

/// The surface position along `x` of the line at `y`, `z`, where the
/// height equals the fixture elevation, by fixed point iteration.
fn surface_x(y: f64, z: f64, x0: f64) -> f64 {
    let mut x = x0;
    for _ in 0..20 {
        let g = geodetic(x, y, z);
        x -= g.h - fixture::elevation(g.lat, g.lon);
    }
    x
}

/// The height of rock in the column of samples at `y`, `z`, measured from
/// the cell's lower `x` face: the solid fraction summed along the column
/// times the sample edge. The density near the surface is 2600 kg/m^3.
fn fill(s: &Section, y: u32, z: u32) -> f64 {
    let n = u32::from(s.resolution());
    let e = sample_edge(s);
    (0..n).map(|x| density(s, x, y, z) / 2600.0).sum::<f64>() * e
}

#[test]
fn cells_across_a_tile_edge_join_without_a_step() {
    // The face y = 0 is longitude 0, the edge between level 1 tile
    // columns 359 and 360 (depth 14 reads level 1).
    let west = cell("4-14-10242-8191-8209");
    let east = cell("4-14-10242-8192-8209");
    let col = |k: &CellKey| -> Vec<u32> {
        sources(&ChunkKey::Cell(*k))
            .iter()
            .map(|&id| TileKey::from_id(id).unwrap().col)
            .collect()
    };
    assert!(col(&west).contains(&359) && col(&east).contains(&360));
    let (a, b) = (build(&west), build(&east));
    assert_eq!(a.origin().y + a.edge().value(), 0.0);
    assert_eq!(b.origin().y, 0.0);
    let n = u32::from(RESOLUTION);
    let e = sample_edge(&a);
    let mut worst_step: f64 = 0.0;
    for z in 0..n {
        let zc = a.origin().z + e * (f64::from(z) + 0.5);
        let fa = fill(&a, n - 1, z);
        let fb = fill(&b, 0, z);
        // Each side agrees with the fixture surface at its plane.
        let xa = surface_x(-e / 2.0, zc, a.origin().x) - a.origin().x;
        let xb = surface_x(e / 2.0, zc, b.origin().x) - b.origin().x;
        assert!((fa - xa).abs() <= e / 2.0, "west z {z}: {fa} vs {xa}");
        assert!((fb - xb).abs() <= e / 2.0, "east z {z}: {fb} vs {xb}");
        // Across the face the rock changes by no more than the surface
        // does between the two planes, plus one sub-sample.
        assert!((fa - fb).abs() <= (xa - xb).abs() + e / 4.0 + 1e-9, "z {z}");
        worst_step = worst_step.max((fa - fb).abs());
    }
    assert!(worst_step < e / 2.0, "{worst_step}");
}

#[test]
fn matter_adds_up_to_the_frame_mass() {
    let depth = 3;
    let cells = 1u32 << depth;
    let mut mass = 0.0;
    for z in 0..cells {
        for y in 0..cells {
            for x in 0..cells {
                let key = CellKey {
                    frame_id: FRAME_ID,
                    depth,
                    x,
                    y,
                    z,
                };
                mass += build(&key).mass().value();
            }
        }
    }
    let rel = (mass - FRAME_MASS) / FRAME_MASS;
    assert!(rel.abs() < 0.01, "matter {mass} kg, frame {FRAME_MASS} kg");
}

#[test]
fn compiling_twice_gives_the_same_bytes() {
    for key in [
        "4-0-0-0-0",
        "4-11-1280-1027-1025",
        "4-16-40947-33125-32768",
        "registry",
    ] {
        let k: ChunkKey = key.parse().unwrap();
        let tiles = fixture::tiles(&sources(&k));
        let one = compile(&k, &tiles).unwrap();
        let two = compile(&k, &fixture::tiles(&sources(&k))).unwrap();
        assert_eq!(one, two, "{key}");
    }
}

#[test]
fn registry_is_empty_and_other_frames_are_empty() {
    let r = compile(&ChunkKey::Registry, &Default::default()).unwrap();
    let reg = registry::decode(&r).unwrap();
    assert!(reg.is_empty() && reg.epoch().value() == 0.0);
    assert!(sources(&ChunkKey::Registry).is_empty());
    // Cells of other frames, declared by the system module or not,
    // including the cells that hold those frames' centers.
    for frame_id in [0u64, 3, 5, 10, 77] {
        for (depth, i) in [(0u8, 0u32), (2, 1), (2, 2), (6, 32)] {
            let key = CellKey {
                frame_id,
                depth,
                x: i,
                y: i,
                z: i,
            };
            let k = ChunkKey::Cell(key);
            assert!(sources(&k).is_empty());
            let bytes = compile(&k, &Default::default()).unwrap();
            assert!(matter::decode(&key, &bytes).unwrap().is_empty(), "{k}");
        }
    }
    // Frame keys get the empty record: the module declares no frame.
    for id in [FRAME_ID, 0, 77] {
        let k = ChunkKey::Frame(id);
        assert!(sources(&k).is_empty());
        let bytes = compile(&k, &Default::default()).unwrap();
        assert_eq!(frame_record::decode(&bytes), Ok(FrameRecord::empty(id)));
    }
}

/// The module's own entry points (module ABI v2) for `key` with these frame
/// records: the section, or the failure reason.
fn run_v2(key: &str, records: &[FrameRecord]) -> Result<Vec<u8>, String> {
    let request = encode_request(&ModuleRequest {
        origin_utc_ms: 0,
        span_seconds: 0,
        span_index: 0,
        key: key.parse().unwrap(),
    });
    let encoded: Vec<Vec<u8>> = records.iter().map(frame_record::encode).collect();
    let refs: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    let frames = encode_inputs(&refs).unwrap();
    let ids = decode_source_ids(&run_sources_v2::<TerrainModule>(&request, &frames)?).unwrap();
    let tile_bytes: Vec<(u64, Vec<u8>)> = ids
        .iter()
        .map(|&id| (id, fixture::tile(id).encode()))
        .collect();
    let tiles: Vec<ModuleTile> = tile_bytes
        .iter()
        .map(|(id, bytes)| ModuleTile { id: *id, bytes })
        .collect();
    let tiles = encode_tiles(&tiles).unwrap();
    run_compile_v2::<TerrainModule>(&request, &tiles, &frames)
}

#[test]
fn the_record_root_extent_sets_the_cell_geometry() {
    let key = "4-3-4-4-4";
    let cell = cell(key);
    let mut half = fixture::record();
    half.root_extent = Meters::new(fixture::record().root_extent.value() / 2.0);
    let a = run_v2(key, &[fixture::record()]).unwrap();
    let b = run_v2(key, &[half]).unwrap();
    let (sa, sb) = (
        matter::decode(&cell, &a).unwrap(),
        matter::decode(&cell, &b).unwrap(),
    );
    let extent = fixture::record().root_extent.value();
    assert_eq!(sa.edge().value(), extent / 8.0);
    assert_eq!(sb.edge().value(), extent / 16.0);
    assert_eq!(sa.origin().x, 0.0);
    assert_eq!(sb.origin().x, 0.0);
    // The same key at half the extent is a cube of half the edge, so it
    // holds less of the planet: still solid at the center, less mass.
    assert!(sb.mass().value() > 0.0 && sb.mass().value() < sa.mass().value());
    // The plan follows the record too: a cell at the record's extent.
    let frame = Frame::from_record(&half).unwrap();
    let g = plan::geometry(&frame, &cell);
    assert_eq!(g.edge.value(), sb.edge().value());
}

#[test]
fn the_record_chooses_the_frame() {
    // The same record under another frame id: the module fills that frame
    // with the same matter, and the frame it no longer names is empty.
    let mut other = fixture::record();
    other.frame_id = 12;
    let own = run_v2("4-3-4-4-4", &[fixture::record()]).unwrap();
    let moved = run_v2("12-3-4-4-4", &[other]).unwrap();
    let (a, b) = (
        matter::decode(&cell("4-3-4-4-4"), &own).unwrap(),
        matter::decode(&cell("12-3-4-4-4"), &moved).unwrap(),
    );
    assert!(!a.is_empty());
    assert_eq!(a.mass().value(), b.mass().value());
    assert_eq!(a.edge().value(), b.edge().value());
    let left = run_v2("4-3-4-4-4", &[other]).unwrap();
    assert!(matter::decode(&cell("4-3-4-4-4"), &left)
        .unwrap()
        .is_empty());
    // Surface tiles follow the record the same way.
    let tile = run_v2("s-12-0-0-0-0", &[other]).unwrap();
    let own_tile = run_v2("s-4-0-0-0-0", &[fixture::record()]).unwrap();
    assert_eq!(tile.len(), own_tile.len());
    assert!(tile.len() > 64, "a tile with a surface");
    let empty_tile = run_v2("s-4-0-0-0-0", &[other]).unwrap();
    assert_eq!(empty_tile.len(), 64, "the header alone");
}

#[test]
fn an_empty_record_fails() {
    let e = run_v2("4-3-4-4-4", &[FrameRecord::empty(FRAME_ID)]).unwrap_err();
    assert!(e.contains("empty"), "{e}");
    // A frame key carries no records and gets the empty record.
    let bytes = run_v2("frame-4", &[]).unwrap();
    assert_eq!(bytes, frame_record::encode(&FrameRecord::empty(4)));
}

#[test]
fn cells_beyond_the_shell_need_no_tiles() {
    // Above 9 km: empty. Below 11 km: the profile alone, solid throughout.
    let above = frame_point(Geodetic {
        lat: 10.0 * DEG,
        lon: 20.0 * DEG,
        h: 30_000.0,
    });
    let below = frame_point(Geodetic {
        lat: -40.0 * DEG,
        lon: 120.0 * DEG,
        h: -40_000.0,
    });
    let up = cell_at(12, above).unwrap();
    let down = cell_at(12, below).unwrap();
    assert_eq!(plan::plan(&fixture::frame(), &up), Plan::Empty);
    assert_eq!(plan::plan(&fixture::frame(), &down), Plan::Interior);
    assert!(build(&up).is_empty());
    let s = build(&down);
    let v = s.samples().unwrap();
    for i in 0..v.len() {
        let x = v.get(i).unwrap();
        assert_eq!(x.state, State::Solid);
        assert!((x.albedo[2].value() - SEAFLOOR_ALBEDO[2]).abs() < 1e-6);
        assert!(x.temperature.value() > 288.0);
    }
    // Deeper than the layer's max depth: empty.
    let deep = cell_at(
        22,
        frame_point(Geodetic {
            lat: 0.3 * DEG,
            lon: -0.3 * DEG,
            h: 300.0,
        }),
    )
    .unwrap();
    assert!(sources(&ChunkKey::Cell(deep)).is_empty());
    assert!(build(&deep).is_empty());
}

/// The tiles a cell lists are always enough: cells straddling the
/// antimeridian, the poles, the equator, and tile corners at many depths
/// compile with exactly their listed tiles. The guest kit passes nothing
/// else, so a missing tile would be an error here.
#[test]
fn listed_tiles_cover_every_read() {
    let places = [
        (0.0, 180.0, 0.0),
        (0.0, -180.0, -4000.0),
        (89.99, 33.0, -4000.0),
        (-89.999, 0.0, -4000.0),
        (90.0, 0.0, -4000.0),
        (45.0, 45.0, -4000.0),
        (-1.0, -1.0, -4000.0),
        (0.0, 0.0, 300.0),
        (0.79, 0.79, 0.0),
        (0.0, 2.5, -10_900.0),
    ];
    for &(lat, lon, h) in &places {
        for depth in [7u8, 10, 12, 15, 19] {
            let p = frame_point(Geodetic {
                lat: lat * DEG,
                lon: lon * DEG,
                h,
            });
            let key = cell_at(depth, p).unwrap();
            let ids = sources(&ChunkKey::Cell(key));
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(ids, sorted, "ascending and unique");
            assert!(!ids.is_empty(), "{key:?} holds the surface");
            let tiles = fixture::tiles(&ids);
            section(&key, &tiles).unwrap_or_else(|e| panic!("{lat} {lon} {depth}: {e}"));
        }
    }
    // The cell at the pole needs every tile of its rows.
    let pole = cell_at(
        10,
        frame_point(Geodetic {
            lat: 90.0 * DEG,
            lon: 0.0,
            h: -4000.0,
        }),
    )
    .unwrap();
    // Depth 10 reads level 5: 45 tile columns around the globe.
    assert!(sources(&ChunkKey::Cell(pole)).len() >= 45);
}

/// Samples are 777 m tall in the coast cell, more than the 300 m plateau,
/// so a sample under the land mixes land and seafloor albedo by mass; one
/// offshore is seafloor alone.
#[test]
fn coast_cell_has_land_and_seafloor() {
    let s = build(&cell("4-11-1280-1027-1025"));
    let v = s.samples().unwrap();
    let (mut land, mut sea) = (false, false);
    for i in 0..v.len() {
        let x = v.get(i).unwrap();
        if x.state == State::Solid {
            let a = x.albedo[0].value();
            assert!((SEAFLOOR_ALBEDO[0] - 1e-6..=LAND_ALBEDO[0] + 1e-6).contains(&a));
            land |= a > SEAFLOOR_ALBEDO[0] + 0.01;
            sea |= (a - SEAFLOOR_ALBEDO[0]).abs() < 1e-6;
        }
    }
    assert!(land && sea);
}

#[test]
fn the_coarse_frame_shows_its_surface_not_its_hot_interior() {
    // The depth-0 section, the one a renderer lights with: the interior's
    // thousands of kelvin stay inside, and the outer samples radiate at the
    // surface temperature, so the frame sends out no light at 1000 K or
    // above (gx_core::emission).
    let key = cell(&format!("{FRAME_ID}-0-0-0-0"));
    let s = build(&key);
    let samples = s.samples().unwrap();
    let hottest = samples
        .iter()
        .map(|x| x.temperature.value())
        .fold(0.0, f64::max);
    assert!(hottest > 1000.0, "the interior is hot: {hottest}");
    assert_eq!(
        gx_core::emission::summarize(&s, gx_core::units::Kelvin::new(1000.0)),
        None
    );
}

#[test]
fn cells_just_west_of_the_antimeridian_list_the_tiles_they_read() {
    // Cells whose longitudes end exactly at -180 degrees: the reader wraps
    // to the first tile column, the lister used to wrap to the last tile's
    // padding columns only.
    for k in ["4-5-12-15-18", "4-6-28-31-38", "4-6-28-31-39"] {
        let key = cell(k);
        let tiles = fixture::tiles(&sources(&ChunkKey::Cell(key)));
        assert!(section(&key, &tiles).is_ok(), "{k}");
    }
}

/// The surface is the level set `occupancy = 0.5` (`matter-format.md`
/// section 3.3), and the ramp puts it on the elevation surface between
/// sample centers. Near latitude 0 and longitude 0 the frame's `x` axis is
/// the vertical, so every line of sample centers along `x` is a vertical
/// line. Through a cell on the summit's flank at three depths, wherever
/// occupancy crosses 0.5 along such a line, linear between sample centers,
/// the crossing lies within a tenth of a sample spacing of the point where
/// the line meets the bicubic elevation surface the cell was built from.
#[test]
fn occupancy_one_half_lies_on_the_elevation_surface() {
    use terrain_module::plan::GRID;
    use terrain_module::surface::Surface;

    let (sl, so) = fixture::summit();
    let (lat, lon) = (sl + 0.015 * DEG, so - 0.01 * DEG);
    let h = fixture::elevation(lat, lon);
    let all = Presence::all();
    let mut worst: f64 = 0.0;
    for depth in [10u8, 14, 17] {
        let key = cell_at(depth, frame_point(Geodetic { lat, lon, h })).unwrap();
        let k = ChunkKey::Cell(key);
        let tiles = fixture::tiles(&sources(&k));
        let s = section(&key, &tiles).unwrap();
        let level = match plan::plan_with(&fixture::frame(), &key, &all) {
            Plan::Surface { level, .. } => level,
            other => panic!("{other:?}"),
        };
        let mut surface = Surface::new(GRID, level, &tiles, &all);
        let v = s.samples().unwrap();
        let n = usize::from(s.resolution());
        let e = sample_edge(&s);
        let o = s.origin();
        let mut crossings = 0;
        let mut worst_here: f64 = 0.0;
        for z in 0..n {
            for y in 0..n {
                let (yc, zc) = (o.y + e * (y as f64 + 0.5), o.z + e * (z as f64 + 0.5));
                let occ: Vec<f64> = (0..n)
                    .map(|x| v.occupancy(x + n * (y + n * z)).value())
                    .collect();
                for x in 0..n - 1 {
                    let (a, b) = (occ[x], occ[x + 1]);
                    if (a - 0.5) * (b - 0.5) > 0.0 || a == b {
                        continue;
                    }
                    let x_half = o.x + e * (x as f64 + 0.5) + e * (a - 0.5) / (a - b);
                    // Where the line meets the bicubic surface: the height
                    // above it is 0.
                    let mut xs = x_half;
                    for _ in 0..50 {
                        let g = geodetic(xs, yc, zc);
                        xs -= g.h - surface.clamped_elevation(g.lat, g.lon).unwrap();
                    }
                    let g = geodetic(xs, yc, zc);
                    let miss = g.h - surface.clamped_elevation(g.lat, g.lon).unwrap();
                    assert!(miss.abs() < 1e-6, "depth {depth}: no solution {miss}");
                    let offset = (x_half - xs).abs() / e;
                    assert!(
                        offset <= 0.1,
                        "depth {depth} y {y} z {z}: {offset} spacings"
                    );
                    worst_here = worst_here.max(offset);
                    crossings += 1;
                }
            }
        }
        // The surface crosses the cell (on the flank it may leave through
        // the top or the bottom, so not every line meets it).
        assert!(crossings >= n, "depth {depth}: {crossings} crossings");
        eprintln!("depth {depth}: {crossings} crossings, worst offset {worst_here} spacings");
        worst = worst.max(worst_here);
    }
    eprintln!("worst offset of the 0.5 crossing: {worst} sample spacings");
}
