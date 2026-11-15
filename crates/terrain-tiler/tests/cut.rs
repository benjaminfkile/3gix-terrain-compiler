//! Cutting a synthetic grid file written in the dataset's HDF5 layout:
//! every tile against an independent in-memory pyramid, golden hashes,
//! resuming, both row orders, and the file checks.

use std::fs;
use std::path::{Path, PathBuf};

use gx_core::manifest;
use gx_core::sha256::{sha256, to_hex};
use terrain_tiler::cut::{cut, tile_path, CutOptions, MANIFEST_FILE, SUMMARY_FILE};
use terrain_tiler::source::{GridFile, MemorySource, RowOrder};
use terrain_tiler::synth;
use terrain_tiles::grid::TILE_SIZE;
use terrain_tiles::{Grid, Tile, TileKey};

/// 540 by 1080 samples (20 arcminutes): levels 0 to 2, a partial last
/// tile row on every level, and a lattice wider than the globe on levels 0
/// and 1, so both the pole clamp and the wrap are exercised.
const ROWS: u32 = 540;

/// The manifest hash of the synthetic grid's source set.
const GOLDEN_MANIFEST: &str = "6f48b1c5b389433bb353aed0a9b4147f8c4548dba77bc60d0362fac2323a29e6";

/// Golden SHA-256 of a few tiles: a corner, a wrapped edge, a tile past
/// the south pole, and the top level.
const GOLDEN_TILES: [(u8, u32, u32, &str); 4] = [
    (
        0,
        0,
        0,
        "2afb3d284273ed41d7476b604841394a2f494b9f860502c6e6f73ab284535724",
    ),
    (
        0,
        1,
        4,
        "003242bae3da851f15c560c6968b720c998683a795f7312aa8caa3786157cd1a",
    ),
    (
        1,
        1,
        2,
        "f9394dd9960f0a8762838116b06ed1bcc6e240f72889a5cb539ad71ab736abac",
    ),
    (
        2,
        0,
        1,
        "6eb2744c98b534a39c08d912ba3e383574a6e8c81253f513d1c7a3c6bb4121af",
    ),
];

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn grid() -> Grid {
    Grid::new(ROWS, 2 * ROWS).unwrap()
}

fn opts() -> CutOptions {
    CutOptions {
        threads: 3,
        progress: false,
    }
}

/// The pyramid written out from its definition, one whole lattice per
/// level, independent of the tiler's bands.
fn reference(g: Grid) -> Vec<(u32, Vec<i16>)> {
    let (rows, cols) = (g.rows(), g.cols());
    let mut levels = Vec::new();
    let w0 = g.lattice_cols(0);
    let mut prev: Vec<i16> = (0..g.lattice_rows(0))
        .flat_map(|r| (0..w0).map(move |c| (r, c)))
        .map(|(r, c)| synth::elevation(&g, r.min(rows - 1), c % cols))
        .collect();
    levels.push((w0, prev.clone()));
    for k in 1..=g.top_level() {
        let pw = g.lattice_cols(k - 1);
        let prows = rows.div_ceil(1 << (k - 1));
        let pcols = cols >> (k - 1);
        let w = g.lattice_cols(k);
        let mut cur = Vec::new();
        for r in 0..g.lattice_rows(k) {
            for c in 0..w {
                let mut s = 0i32;
                for i in 0..2 {
                    for j in 0..2 {
                        let rr = (2 * r + i).min(prows - 1);
                        let cc = (2 * c + j) % pcols;
                        s += i32::from(prev[(rr * pw + cc) as usize]);
                    }
                }
                let m = if s >= 0 { (s + 2) / 4 } else { -((-s + 2) / 4) };
                cur.push(m as i16);
            }
        }
        levels.push((w, cur.clone()));
        prev = cur;
    }
    levels
}

fn read_tile(out: &Path, key: TileKey) -> Vec<u8> {
    fs::read(tile_path(out, key)).unwrap()
}

#[test]
fn cuts_every_tile_by_the_definition() {
    let dir = scratch("cut-definition");
    let g = grid();
    let file = dir.join("grid.nc");
    synth::write(&file, g, RowOrder::SouthFirst).unwrap();
    let out = dir.join("out");
    let mut src = GridFile::open(&file, g).unwrap();
    assert_eq!(src.order(), RowOrder::SouthFirst);
    let report = cut(&mut src, g, &out, opts()).unwrap();

    assert_eq!(g.top_level(), 2);
    assert_eq!(report.tiles, 15 + 6 + 2);
    assert_eq!(report.written, report.tiles);
    let reference = reference(g);
    let mbytes = fs::read(out.join(MANIFEST_FILE)).unwrap();
    let m = manifest::decode(&mbytes).unwrap();
    assert_eq!(m.entries().len() as u64, report.tiles);
    for e in m.entries() {
        let key = TileKey::from_id(e.id).unwrap();
        let bytes = read_tile(&out, key);
        assert_eq!((sha256(&bytes), bytes.len() as u64), (e.sha256, e.length));
        let tile = Tile::decode_id(e.id, &bytes).unwrap();
        let (w, lattice) = &reference[key.level as usize];
        for i in 0..TILE_SIZE {
            for j in 0..TILE_SIZE {
                let r = key.row * TILE_SIZE + i;
                let c = key.col * TILE_SIZE + j;
                assert_eq!(
                    tile.get(i, j),
                    lattice[(r * w + c) as usize],
                    "{key} sample {i} {j}"
                );
            }
        }
    }
    let (lo, hi) = (0..ROWS)
        .flat_map(|r| (0..2 * ROWS).map(move |c| synth::elevation(&g, r, c)))
        .fold((i16::MAX, i16::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    assert_eq!((report.min_elevation, report.max_elevation), (lo, hi));
    let summary = fs::read_to_string(out.join(SUMMARY_FILE)).unwrap();
    assert_eq!(summary, report.summary_json());
    assert!(summary.contains(&format!("\"elevationMinMeters\": {lo},")));
    assert!(summary.contains(&format!(
        "\"manifestHash\": \"{}\"",
        to_hex(&report.manifest_hash)
    )));
}

#[test]
fn golden_hashes() {
    let dir = scratch("cut-golden");
    let g = grid();
    let file = dir.join("grid.nc");
    synth::write(&file, g, RowOrder::SouthFirst).unwrap();
    let out = dir.join("out");
    let report = cut(&mut GridFile::open(&file, g).unwrap(), g, &out, opts()).unwrap();
    let mut got = vec![format!("manifest {}", to_hex(&report.manifest_hash))];
    let mut want = vec![format!("manifest {GOLDEN_MANIFEST}")];
    for (level, row, col, hash) in GOLDEN_TILES {
        let bytes = read_tile(&out, TileKey { level, row, col });
        got.push(format!("{level} {row} {col} {}", to_hex(&sha256(&bytes))));
        want.push(format!("{level} {row} {col} {hash}"));
    }
    assert_eq!(got, want);
}

#[test]
fn resumes_and_repairs() {
    let dir = scratch("cut-resume");
    let g = grid();
    let file = dir.join("grid.nc");
    synth::write(&file, g, RowOrder::SouthFirst).unwrap();
    let out = dir.join("out");
    let first = cut(&mut GridFile::open(&file, g).unwrap(), g, &out, opts()).unwrap();
    let again = cut(&mut GridFile::open(&file, g).unwrap(), g, &out, opts()).unwrap();
    assert_eq!((again.written, again.skipped), (0, first.tiles));
    assert_eq!(again.manifest_hash, first.manifest_hash);

    // A damaged tile and a missing one are written again; nothing else is.
    let damaged = tile_path(
        &out,
        TileKey {
            level: 1,
            row: 0,
            col: 1,
        },
    );
    fs::write(&damaged, b"partial").unwrap();
    fs::remove_file(tile_path(
        &out,
        TileKey {
            level: 0,
            row: 2,
            col: 3,
        },
    ))
    .unwrap();
    let mut one_thread = opts();
    one_thread.threads = 1;
    let fixed = cut(&mut GridFile::open(&file, g).unwrap(), g, &out, one_thread).unwrap();
    assert_eq!((fixed.written, fixed.skipped), (2, first.tiles - 2));
    assert_eq!(fixed.manifest_hash, first.manifest_hash);
    assert_eq!(
        Tile::decode(&fs::read(&damaged).unwrap()).unwrap().key(),
        TileKey {
            level: 1,
            row: 0,
            col: 1
        }
    );
}

#[test]
fn row_order_and_source_do_not_change_the_bytes() {
    let dir = scratch("cut-order");
    let g = grid();
    let south = dir.join("south.nc");
    let north = dir.join("north.nc");
    synth::write(&south, g, RowOrder::SouthFirst).unwrap();
    synth::write(&north, g, RowOrder::NorthFirst).unwrap();
    let a = cut(
        &mut GridFile::open(&south, g).unwrap(),
        g,
        &dir.join("a"),
        opts(),
    )
    .unwrap();
    let mut nf = GridFile::open(&north, g).unwrap();
    assert_eq!(nf.order(), RowOrder::NorthFirst);
    let b = cut(&mut nf, g, &dir.join("b"), opts()).unwrap();
    let mut mem = MemorySource {
        grid: g,
        samples: (0..ROWS)
            .flat_map(|r| (0..2 * ROWS).map(move |c| synth::elevation(&g, r, c)))
            .collect(),
    };
    let c = cut(&mut mem, g, &dir.join("c"), opts()).unwrap();
    assert_eq!(a.manifest_hash, b.manifest_hash);
    assert_eq!(a.manifest_hash, c.manifest_hash);
    assert_eq!(
        fs::read(dir.join("a").join(MANIFEST_FILE)).unwrap(),
        fs::read(dir.join("c").join(MANIFEST_FILE)).unwrap()
    );
}

#[test]
fn checks_the_file_before_cutting() {
    let dir = scratch("cut-checks");
    let g = grid();
    let file = dir.join("grid.nc");
    synth::write(&file, g, RowOrder::SouthFirst).unwrap();
    // The dataset's own shape is required unless another grid is named.
    let e = GridFile::open(&file, Grid::GEBCO).err().unwrap();
    assert!(e.to_string().contains("shape"), "{e}");

    // A grid whose coordinates do not match its shape.
    let bad = dir.join("bad.nc");
    {
        let f = hdf5::File::create(&bad).unwrap();
        let lat: Vec<f64> = (0..ROWS).map(|i| f64::from(i) * 0.1).collect();
        let lon: Vec<f64> = (0..2 * ROWS)
            .map(|j| -180.0 + (f64::from(j) + 0.5) / 3.0)
            .collect();
        f.new_dataset_builder()
            .with_data(&ndarray::Array1::from(lat))
            .create("lat")
            .unwrap();
        f.new_dataset_builder()
            .with_data(&ndarray::Array1::from(lon))
            .create("lon")
            .unwrap();
        f.new_dataset::<i16>()
            .shape((ROWS as usize, 2 * ROWS as usize))
            .create("elevation")
            .unwrap();
    }
    let e = GridFile::open(&bad, g).err().unwrap();
    assert!(e.to_string().starts_with("lat:"), "{e}");

    // The wrong element type.
    let wide = dir.join("wide.nc");
    {
        let f = hdf5::File::create(&wide).unwrap();
        f.new_dataset::<i32>()
            .shape((ROWS as usize, 2 * ROWS as usize))
            .create("elevation")
            .unwrap();
    }
    let e = GridFile::open(&wide, g).err().unwrap();
    assert!(e.to_string().contains("16-bit"), "{e}");
}
