//! Cutting a grid into every tile of every level.
//!
//! The source set of `client-compute.md` section 5.3: tiles in the format
//! of [`terrain_tiles::tile`], the manifest of section 5.1 (`manifest.bin`,
//! the core library's encoder), and a `summary.json` with counts, bytes, and
//! the elevation range.
//!
//! # Streaming
//!
//! The grid is read one band of [`TILE_SIZE`] data rows at a time, north to
//! south. Each band becomes one row of level 0 tiles, which are written at
//! once. Every two rows of level `k` tiles (or the last one alone) become one
//! row of level `k + 1` tiles by the rule in `terrain_tiles::grid`, and are
//! then dropped. At most two full-width rows of tiles per level are held at
//! a time.
//!
//! # Determinism and resuming
//!
//! Every tile is a pure function of the grid, and the manifest and summary
//! are pure functions of the tiles, so every run on every machine writes the
//! same bytes. Threads only compress tiles of one row side by side; the
//! results are gathered in column order. A tile whose file already exists
//! with the SHA-256 of the bytes it should have is not written again, so an
//! interrupted run is resumed by running it again. Every file is written to a
//! temporary name first and renamed into place, so an interrupted write
//! never leaves a partial file under a tile's name.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gx_core::manifest::{self, Entry, Manifest};
use gx_core::sha256::{sha256, to_hex, DIGEST_LEN};
use terrain_tiles::grid::{mean4, TILE_SIZE};
use terrain_tiles::{Grid, Tile, TileKey};

use crate::source::{RowSource, SourceError};

/// The manifest file in the output directory.
pub const MANIFEST_FILE: &str = "manifest.bin";
/// The summary file in the output directory.
pub const SUMMARY_FILE: &str = "summary.json";
/// The directory of tiles in the output directory.
pub const TILES_DIR: &str = "tiles";

/// The path of a tile's file under the output directory `out`:
/// `tiles/{level}/{row}/{col}.3gte`.
pub fn tile_path(out: &Path, key: TileKey) -> PathBuf {
    out.join(TILES_DIR)
        .join(key.level.to_string())
        .join(key.row.to_string())
        .join(format!("{}.3gte", key.col))
}

/// Why a cut failed.
#[derive(Debug)]
pub enum CutError {
    /// The grid could not be read.
    Source(SourceError),
    /// A file could not be read or written.
    Io(PathBuf, io::Error),
}

impl fmt::Display for CutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CutError::Source(e) => write!(f, "{e}"),
            CutError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for CutError {}

impl From<SourceError> for CutError {
    fn from(e: SourceError) -> Self {
        CutError::Source(e)
    }
}

/// How to cut.
#[derive(Copy, Clone, Debug)]
pub struct CutOptions {
    /// Threads that compress tiles. The output does not depend on it.
    pub threads: usize,
    /// Print a line per finished level 0 row of tiles.
    pub progress: bool,
}

impl Default for CutOptions {
    fn default() -> Self {
        CutOptions {
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            progress: false,
        }
    }
}

/// Per level counts of a cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelSummary {
    /// The level.
    pub level: u8,
    /// Rows of tiles.
    pub tile_rows: u32,
    /// Columns of tiles.
    pub tile_cols: u32,
    /// Tiles.
    pub tiles: u64,
    /// Bytes of all tiles.
    pub bytes: u64,
}

/// What a cut produced. Everything but `written` and `skipped` is also in
/// `summary.json` and is a pure function of the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutReport {
    /// The grid.
    pub grid: Grid,
    /// Per level counts, finest first.
    pub levels: Vec<LevelSummary>,
    /// Tiles in all levels.
    pub tiles: u64,
    /// Distinct tile hashes: the most files an upload sends.
    pub distinct: u64,
    /// Bytes of all tiles.
    pub bytes: u64,
    /// Bytes of the manifest.
    pub manifest_bytes: u64,
    /// SHA-256 of the manifest.
    pub manifest_hash: [u8; DIGEST_LEN],
    /// Lowest elevation in the data, meters.
    pub min_elevation: i16,
    /// Highest elevation in the data, meters.
    pub max_elevation: i16,
    /// Tiles written by this run.
    pub written: u64,
    /// Tiles already present with the right hash.
    pub skipped: u64,
}

impl CutReport {
    /// The contents of `summary.json`: every field but `written` and
    /// `skipped`, in a fixed order with a fixed layout.
    pub fn summary_json(&self) -> String {
        let mut s = String::new();
        s.push_str("{\n");
        s.push_str(&format!(
            "  \"grid\": {{ \"rows\": {}, \"cols\": {} }},\n",
            self.grid.rows(),
            self.grid.cols()
        ));
        s.push_str("  \"levels\": [\n");
        for (i, l) in self.levels.iter().enumerate() {
            s.push_str(&format!(
                "    {{ \"level\": {}, \"tileRows\": {}, \"tileCols\": {}, \"tiles\": {}, \"bytes\": {} }}{}\n",
                l.level,
                l.tile_rows,
                l.tile_cols,
                l.tiles,
                l.bytes,
                if i + 1 < self.levels.len() { "," } else { "" }
            ));
        }
        s.push_str("  ],\n");
        s.push_str(&format!("  \"tiles\": {},\n", self.tiles));
        s.push_str(&format!("  \"distinctTiles\": {},\n", self.distinct));
        s.push_str(&format!("  \"tileBytes\": {},\n", self.bytes));
        s.push_str(&format!("  \"manifestBytes\": {},\n", self.manifest_bytes));
        s.push_str(&format!(
            "  \"manifestHash\": \"{}\",\n",
            to_hex(&self.manifest_hash)
        ));
        s.push_str(&format!(
            "  \"elevationMinMeters\": {},\n",
            self.min_elevation
        ));
        s.push_str(&format!(
            "  \"elevationMaxMeters\": {}\n",
            self.max_elevation
        ));
        s.push_str("}\n");
        s
    }
}

/// One full-width row of tiles of one level: [`TILE_SIZE`] lattice rows of
/// `width` samples.
struct Band {
    level: u8,
    index: u32,
    width: usize,
    samples: Vec<i16>,
}

impl Band {
    fn row(&self, i: u32) -> &[i16] {
        let a = i as usize * self.width;
        &self.samples[a..a + self.width]
    }
}

struct Cutter<'a> {
    grid: Grid,
    out: &'a Path,
    opts: CutOptions,
    pending: Vec<Vec<Band>>,
    entries: Vec<Entry>,
    levels: Vec<LevelSummary>,
    written: u64,
    skipped: u64,
}

/// Cuts every tile of every level of `grid` from `source` into `out`, then
/// writes `manifest.bin` and `summary.json` (module documentation).
pub fn cut(
    source: &mut dyn RowSource,
    grid: Grid,
    out: &Path,
    opts: CutOptions,
) -> Result<CutReport, CutError> {
    fs::create_dir_all(out).map_err(|e| CutError::Io(out.to_path_buf(), e))?;
    let mut c = Cutter {
        grid,
        out,
        opts,
        pending: grid.levels().map(|_| Vec::new()).collect(),
        entries: Vec::new(),
        levels: grid
            .levels()
            .map(|level| LevelSummary {
                level,
                tile_rows: grid.tile_rows(level),
                tile_cols: grid.tile_cols(level),
                tiles: 0,
                bytes: 0,
            })
            .collect(),
        written: 0,
        skipped: 0,
    };
    let (mut lo, mut hi) = (i16::MAX, i16::MIN);
    let width = grid.lattice_cols(0) as usize;
    let cols = grid.cols() as usize;
    for b in 0..grid.tile_rows(0) {
        let start = b * TILE_SIZE;
        let count = TILE_SIZE.min(grid.rows() - start);
        let data = source.read_rows(start, count)?;
        if data.len() != count as usize * cols {
            return Err(CutError::Source(SourceError(format!(
                "rows {start} to {}: {} samples, expected {}",
                start + count,
                data.len(),
                count as usize * cols
            ))));
        }
        for &v in &data {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let mut samples = Vec::with_capacity(TILE_SIZE as usize * width);
        for i in 0..TILE_SIZE {
            let r = (grid.data_row(start + i) - start) as usize;
            let row = &data[r * cols..(r + 1) * cols];
            for c in 0..width as u32 {
                samples.push(row[grid.data_col(c) as usize]);
            }
        }
        c.push(Band {
            level: 0,
            index: b,
            width,
            samples,
        })?;
        if opts.progress {
            println!(
                "cut: level 0 row {} of {}, {} tiles so far",
                b + 1,
                grid.tile_rows(0),
                c.entries.len()
            );
        }
    }
    c.finish(lo, hi)
}

impl Cutter<'_> {
    /// Writes a band's tiles and feeds it to the next level.
    fn push(&mut self, band: Band) -> Result<(), CutError> {
        self.emit(&band)?;
        let k = band.level;
        if k == self.grid.top_level() {
            return Ok(());
        }
        let last = band.index + 1 == self.grid.tile_rows(k);
        let slot = &mut self.pending[k as usize];
        slot.push(band);
        if slot.len() == 2 || last {
            let finer = std::mem::take(slot);
            let parent = self.coarsen(k + 1, &finer);
            self.push(parent)?;
        }
        Ok(())
    }

    /// Band `finer[0].index / 2` of `level` from one or two bands of
    /// `level - 1` (`terrain_tiles::grid`, values).
    fn coarsen(&self, level: u8, finer: &[Band]) -> Band {
        let g = self.grid;
        let index = finer[0].index / 2;
        let width = g.lattice_cols(level) as usize;
        let cols: Vec<[usize; 2]> = (0..width as u32)
            .map(|c| {
                [
                    g.finer_col(level, c, 0) as usize,
                    g.finer_col(level, c, 1) as usize,
                ]
            })
            .collect();
        let mut samples = Vec::with_capacity(TILE_SIZE as usize * width);
        for i in 0..TILE_SIZE {
            let r = index * TILE_SIZE + i;
            let [a, b] = [0, 1].map(|ii| {
                let src = g.finer_row(level, r, ii);
                let band = finer
                    .iter()
                    .find(|f| f.index == src / TILE_SIZE)
                    .expect("a coarse row reads only the two bands below it");
                band.row(src % TILE_SIZE)
            });
            for &[c0, c1] in &cols {
                samples.push(mean4(a[c0], a[c1], b[c0], b[c1]));
            }
        }
        Band {
            level,
            index,
            width,
            samples,
        }
    }

    /// Encodes, hashes, and writes every tile of a band, threads side by
    /// side, and records the manifest entries in column order.
    fn emit(&mut self, band: &Band) -> Result<(), CutError> {
        let tile_cols = self.grid.tile_cols(band.level);
        let threads = self.opts.threads.clamp(1, tile_cols as usize);
        let per = (tile_cols as usize).div_ceil(threads);
        let out = self.out;
        let results: Vec<Result<(Entry, bool), CutError>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    s.spawn(move || {
                        let lo = (t * per) as u32;
                        let hi = (((t + 1) * per) as u32).min(tile_cols);
                        (lo..hi)
                            .map(|col| write_tile(out, band, col))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("a tile thread does not panic"))
                .collect()
        });
        let summary = &mut self.levels[band.level as usize];
        for r in results {
            let (entry, wrote) = r?;
            summary.tiles += 1;
            summary.bytes += entry.length;
            if wrote {
                self.written += 1;
            } else {
                self.skipped += 1;
            }
            self.entries.push(entry);
        }
        Ok(())
    }

    /// Writes the manifest and the summary.
    fn finish(self, min_elevation: i16, max_elevation: i16) -> Result<CutReport, CutError> {
        let mut hashes: Vec<[u8; DIGEST_LEN]> = self.entries.iter().map(|e| e.sha256).collect();
        hashes.sort_unstable();
        hashes.dedup();
        let tiles = self.entries.len() as u64;
        let bytes = self.entries.iter().map(|e| e.length).sum();
        let m = Manifest::new(self.entries).expect("tile ids are unique and fit a u32 count");
        let mbytes = manifest::encode(&m);
        write_atomic(&self.out.join(MANIFEST_FILE), &mbytes)?;
        let report = CutReport {
            grid: self.grid,
            levels: self.levels,
            tiles,
            distinct: hashes.len() as u64,
            bytes,
            manifest_bytes: mbytes.len() as u64,
            manifest_hash: manifest::manifest_hash(&mbytes),
            min_elevation,
            max_elevation,
            written: self.written,
            skipped: self.skipped,
        };
        write_atomic(
            &self.out.join(SUMMARY_FILE),
            report.summary_json().as_bytes(),
        )?;
        Ok(report)
    }
}

/// Builds, encodes, and writes tile column `col` of a band. Returns its
/// manifest entry and whether the file was written (`false` when it already
/// held these bytes).
fn write_tile(out: &Path, band: &Band, col: u32) -> Result<(Entry, bool), CutError> {
    let key = TileKey {
        level: band.level,
        row: band.index,
        col,
    };
    let x0 = (col * TILE_SIZE) as usize;
    let mut samples = Vec::with_capacity((TILE_SIZE * TILE_SIZE) as usize);
    for i in 0..TILE_SIZE {
        samples.extend_from_slice(&band.row(i)[x0..x0 + TILE_SIZE as usize]);
    }
    let tile = Tile::new(key, samples).expect("a full tile with a valid key");
    let bytes = tile.encode();
    let hash = sha256(&bytes);
    let entry = Entry {
        id: key.id(),
        sha256: hash,
        length: bytes.len() as u64,
    };
    let path = tile_path(out, key);
    if let Ok(existing) = fs::read(&path) {
        if sha256(&existing) == hash {
            return Ok((entry, false));
        }
    }
    write_atomic(&path, &bytes)?;
    Ok((entry, true))
}

/// Writes `bytes` to `path` through a temporary file in the same directory.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CutError> {
    let io = |e| CutError::Io(path.to_path_buf(), e);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes).map_err(io)?;
    fs::rename(&tmp, path).map_err(io)
}
