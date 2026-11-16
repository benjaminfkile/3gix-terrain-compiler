//! Cutting the GLO-30 pyramid ([`Grid::GLO30`]): Copernicus GLO-30 on land,
//! GEBCO under the oceans.
//!
//! The raw GLO-30 set is about 26,450 one degree tiles of `f32` heights
//! (GeoTIFF, pixel is point, the north row at the tile's north edge, 3600
//! rows and a width that shrinks toward the poles), each with a water body
//! mask (`_WBM`, 0 land, 1 ocean, 2 lake, 3 river). Tiles exist only where
//! there is land. It is far too large to hold, so it is cut band by band:
//! [`cut_band`] cuts one band of four degrees of latitude (one row of
//! level 4 tiles) from the raw tiles of that band on local disk, and
//! [`finish`] builds the coarse levels, the presence map, the manifest, and
//! the summary once every band is done. A driver downloads a band, cuts it,
//! and deletes it before the next (`scripts/glo30.sh`).
//!
//! # Values
//!
//! A level 0 sample (15/16 arcsecond, cell centered as in
//! [`terrain_tiles::grid`]) is
//!
//! - where a GLO-30 tile covers it and its nearest GLO-30 pixel is not
//!   ocean: the bilinear interpolation of the four GLO-30 pixels around it
//!   (across tile edges where the neighbor tile is in the band; clamped to
//!   the tile's edge pixels otherwise), rounded to whole meters;
//! - elsewhere: the bilinear interpolation of the four GEBCO samples around
//!   it.
//!
//! A level 0 tile exists when one of its samples came from GLO-30
//! ([`terrain_tiles::presence`]). Levels 1 to 3 are built from the one
//! below by the mean of four, over every sample (computed whether its tile
//! is kept or not), and a tile is kept when one of its children is. A level
//! 4 sample is the mean of its four level 3 samples when their level 3
//! tile exists, and otherwise the GEBCO sample at the same place (level 4
//! has GEBCO's spacing exactly). Levels 5 and up are built from level 4 by
//! the mean of four ([`terrain_tiles::grid`]).
//!
//! Heights are meters above the datasets' vertical datums (GLO-30: EGM2008;
//! GEBCO: mean sea level), as the GEBCO pyramid's were.
//!
//! # Resuming and determinism
//!
//! Every tile is a pure function of the raw data. A band writes its tiles,
//! then `bands/{band}.entries` (its manifest entries, the level 0 tiles
//! it kept, and its elevation range), and that file marks it done. Running
//! a done band again rewrites nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use gx_core::manifest::{self, Entry, Manifest};
use gx_core::sha256::{sha256, to_hex, DIGEST_LEN};
use terrain_tiles::grid::{mean4, FINE_LEVELS};
use terrain_tiles::{Grid, Presence, Tile, TileKey};
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::cut::{tile_path, write_atomic, MANIFEST_FILE, SUMMARY_FILE};
use crate::source::{GridFile, RowSource};

/// The grid this module cuts.
pub const GRID: Grid = Grid::GLO30;

/// Bands: rows of level 4 tiles, four degrees each, from the north.
pub const BANDS: u32 = 45;

/// The presence map file in the output directory.
pub const PRESENCE_FILE: &str = "presence.bin";

/// Level 0 samples per degree.
const PER_DEGREE: f64 = 3840.0;

/// GEBCO samples per degree.
const GEBCO_PER_DEGREE: f64 = 240.0;

/// A GLO-30 height at or below this is no data.
const NO_DATA_BELOW: f32 = -10000.0;

/// Why a band or the finish failed.
#[derive(Debug)]
pub struct Glo30Error(pub String);

impl fmt::Display for Glo30Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Glo30Error {}

fn err(e: impl fmt::Display) -> Glo30Error {
    Glo30Error(e.to_string())
}

/// The base name of the GLO-30 tile whose south edge is latitude `lat` and
/// west edge longitude `lon` (degrees), for example
/// `Copernicus_DSM_COG_10_N46_00_E007_00`.
pub fn tile_name(lat: i32, lon: i32) -> String {
    format!(
        "Copernicus_DSM_COG_10_{}{:02}_00_{}{:03}_00",
        if lat < 0 { 'S' } else { 'N' },
        lat.unsigned_abs(),
        if lon < 0 { 'W' } else { 'E' },
        lon.unsigned_abs()
    )
}

/// The south edges (degrees) of the GLO-30 tile rows of band `band`, north
/// first.
pub fn band_latitudes(band: u32) -> [i32; 4] {
    let north = 90 - 4 * band as i32;
    [north - 1, north - 2, north - 3, north - 4]
}

/// One GLO-30 tile with its water body mask.
struct SourceTile {
    north: f64,
    west: f64,
    width: usize,
    height: usize,
    dem: Vec<f32>,
    wbm: Vec<u8>,
}

fn read_tiff(path: &Path) -> Result<(usize, usize, [f64; 2], DecodingResult), Glo30Error> {
    let f = fs::File::open(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    let mut d =
        Decoder::new(BufReader::new(f)).map_err(|e| err(format!("{}: {e}", path.display())))?;
    let (w, h) = d.dimensions().map_err(err)?;
    let tie = d
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .map_err(|e| err(format!("{}: tiepoint: {e}", path.display())))?;
    if tie.len() < 6 || tie[0] != 0.0 || tie[1] != 0.0 {
        return Err(err(format!(
            "{}: unexpected tiepoint {tie:?}",
            path.display()
        )));
    }
    let image = d
        .read_image()
        .map_err(|e| err(format!("{}: {e}", path.display())))?;
    Ok((w as usize, h as usize, [tie[3], tie[4]], image))
}

impl SourceTile {
    fn load(dir: &Path, lat: i32, lon: i32) -> Result<Option<SourceTile>, Glo30Error> {
        let name = tile_name(lat, lon);
        let dem_path = dir.join(format!("{name}_DEM.tif"));
        if !dem_path.exists() {
            return Ok(None);
        }
        let (w, h, [west, north], dem) = read_tiff(&dem_path)?;
        let DecodingResult::F32(dem) = dem else {
            return Err(err(format!("{}: not f32", dem_path.display())));
        };
        // Some files store the corner a rounding step off a whole degree.
        let (lon_w, lat_n) = (f64::from(lon), f64::from(lat + 1));
        if (west - lon_w).abs() > 1e-6 || (north - lat_n).abs() > 1e-6 {
            return Err(err(format!(
                "{}: tiepoint {west}, {north} is not the tile's corner",
                dem_path.display()
            )));
        }
        let (west, north) = (lon_w, lat_n);
        let wbm_path = dir.join(format!("{name}_WBM.tif"));
        let (ww, wh, _, wbm) = read_tiff(&wbm_path)?;
        let DecodingResult::U8(wbm) = wbm else {
            return Err(err(format!("{}: not u8", wbm_path.display())));
        };
        if (ww, wh) != (w, h) || dem.len() != w * h || wbm.len() != w * h {
            return Err(err(format!("{name}: DEM and WBM shapes differ")));
        }
        Ok(Some(SourceTile {
            north,
            west,
            width: w,
            height: h,
            dem,
            wbm,
        }))
    }
}

/// The GLO-30 tiles of one band, by `(south edge, west edge)` degrees.
struct Sources {
    tiles: BTreeMap<(i32, i32), SourceTile>,
    /// Window tiles that have no raw file (all ocean).
    missing: BTreeSet<(i32, i32)>,
}

impl Sources {
    /// Keeps exactly the band's tiles from longitude `west` to `east`
    /// (degrees, both included, wrapped), loading those not yet held from
    /// `raw`.
    fn load_window(
        &mut self,
        raw: &Path,
        lats: &[i32],
        west: i32,
        east: i32,
        threads: usize,
    ) -> Result<(), Glo30Error> {
        let wrap = |lon: i32| (lon + 180).rem_euclid(360) - 180;
        let lons: BTreeSet<i32> = (west..=east).map(wrap).collect();
        self.tiles.retain(|k, _| lons.contains(&k.1));
        self.missing.retain(|k| lons.contains(&k.1));
        let names: Vec<(i32, i32)> = lats
            .iter()
            .flat_map(|&lat| lons.iter().map(move |&lon| (lat, lon)))
            .filter(|k| !self.tiles.contains_key(k) && !self.missing.contains(k))
            .collect();
        let loaded = parallel(&names, threads, |&(lat, lon)| {
            SourceTile::load(raw, lat, lon)
        });
        for (&key, t) in names.iter().zip(loaded) {
            match t? {
                Some(t) => {
                    self.tiles.insert(key, t);
                }
                None => {
                    self.missing.insert(key);
                }
            }
        }
        Ok(())
    }

    fn get(&self, lat: i32, lon: i32) -> Option<&SourceTile> {
        let lon = (lon + 180).rem_euclid(360) - 180;
        self.tiles.get(&(lat, lon))
    }

    /// The tile and row holding pixel row `row` (may be `height`, the first
    /// row of the tile to the south) of tile `(lat, lon)`, clamped to the
    /// tile when the southern one is not loaded.
    fn row(&self, lat: i32, lon: i32, t: &SourceTile, row: usize) -> (i32, usize) {
        if row < t.height {
            return (lat, row);
        }
        match self.get(lat - 1, lon) {
            Some(_) => (lat - 1, row - t.height),
            None => (lat, t.height - 1),
        }
    }

    /// The height and ocean flag of the pixel at `col` (may be `width`, the
    /// first column of the tile to the east) of row `row` of tile `(lat,
    /// lon)`; clamped when the eastern tile is not loaded.
    fn pixel(&self, lat: i32, lon: i32, row: usize, col: usize) -> (f32, u8) {
        let t = self.get(lat, lon).expect("a loaded tile");
        if col < t.width {
            let i = row * t.width + col;
            return (t.dem[i], t.wbm[i]);
        }
        match self.get(lat, lon + 1) {
            Some(e) => {
                let i = row * e.width + (col - t.width);
                (e.dem[i], e.wbm[i])
            }
            None => {
                let i = row * t.width + t.width - 1;
                (t.dem[i], t.wbm[i])
            }
        }
    }

    /// The bilinear GLO-30 height at `lat`, `lon` (degrees), or `None` when
    /// no tile covers it, its nearest pixel is ocean, or a pixel has no
    /// data.
    fn height(&self, lat: f64, lon: f64) -> Option<f64> {
        let (la, lo) = (lat.floor() as i32, lon.floor() as i32);
        let t = self.get(la, lo)?;
        let y = (t.north - lat) * t.height as f64;
        let x = (lon - t.west) * t.width as f64;
        // The nearest pixel decides land or water.
        let (nr, nc) = (y.round() as usize, x.round() as usize);
        let (nla, nrow) = self.row(la, lo, t, nr);
        let near = self.get(nla, lo).expect("a loaded tile");
        let nx = if nla == la {
            nc
        } else {
            ((lon - near.west) * near.width as f64).round() as usize
        };
        let (_, water) = self.pixel(nla, lo, nrow, nx);
        if water == 1 {
            return None;
        }
        let (y0, fy) = (y.floor(), y - y.floor());
        let mut rows = [0.0f64; 2];
        for (k, value) in rows.iter_mut().enumerate() {
            let (rla, rrow) = self.row(la, lo, t, y0 as usize + k);
            let rt = self.get(rla, lo).expect("a loaded tile");
            let rx = (lon - rt.west) * rt.width as f64;
            let (x0, fx) = (rx.floor(), rx - rx.floor());
            let (a, _) = self.pixel(rla, lo, rrow, x0 as usize);
            let (b, _) = self.pixel(rla, lo, rrow, x0 as usize + 1);
            if a <= NO_DATA_BELOW || b <= NO_DATA_BELOW {
                return None;
            }
            *value = f64::from(a) + fx * (f64::from(b) - f64::from(a));
        }
        Some(rows[0] + fy * (rows[1] - rows[0]))
    }
}

/// The GEBCO rows of one band, with a row of margin on each side.
struct Gebco {
    first_row: u32,
    rows: u32,
    cols: u32,
    data: Vec<i16>,
}

impl Gebco {
    fn read(file: &mut GridFile, band: u32) -> Result<Gebco, Glo30Error> {
        let g = Grid::GEBCO;
        let first = (band * 960).saturating_sub(1);
        let last = ((band + 1) * 960).min(g.rows() - 1);
        let rows = last - first + 1;
        let data = file.read_rows(first, rows).map_err(err)?;
        Ok(Gebco {
            first_row: first,
            rows,
            cols: g.cols(),
            data,
        })
    }

    /// The GEBCO sample at data row `r` (clamped to the band's rows) and
    /// column `c` (wrapped).
    fn at(&self, r: i64, c: i64) -> f64 {
        let r = (r - i64::from(self.first_row)).clamp(0, i64::from(self.rows) - 1) as usize;
        let c = c.rem_euclid(i64::from(self.cols)) as usize;
        f64::from(self.data[r * self.cols as usize + c])
    }

    /// The bilinear GEBCO height at `lat`, `lon` (degrees).
    fn height(&self, lat: f64, lon: f64) -> f64 {
        let y = (90.0 - lat) * GEBCO_PER_DEGREE - 0.5;
        let x = (lon + 180.0) * GEBCO_PER_DEGREE - 0.5;
        let (y0, x0) = (y.floor(), x.floor());
        let (fy, fx) = (y - y0, x - x0);
        let (r, c) = (y0 as i64, x0 as i64);
        let top = self.at(r, c) + fx * (self.at(r, c + 1) - self.at(r, c));
        let bottom = self.at(r + 1, c) + fx * (self.at(r + 1, c + 1) - self.at(r + 1, c));
        top + fy * (bottom - top)
    }
}

/// Rounds to whole meters, half away from zero, into `i16`.
fn meters(v: f64) -> i16 {
    v.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

/// One level 0 tile: its samples and whether any came from GLO-30.
fn level0_tile(sources: &Sources, gebco: &Gebco, row: u32, col: u32) -> (Vec<i16>, bool) {
    let n = GRID.tile_size();
    let mut samples = Vec::with_capacity(GRID.tile_samples());
    let mut land = false;
    for i in 0..n {
        let lat = 90.0 - (f64::from(row * n + i) + 0.5) / PER_DEGREE;
        for j in 0..n {
            let lon = -180.0 + (f64::from(col * n + j) + 0.5) / PER_DEGREE;
            let v = match sources.height(lat, lon) {
                Some(h) => {
                    land = true;
                    h
                }
                None => gebco.height(lat, lon),
            };
            samples.push(meters(v));
        }
    }
    (samples, land)
}

/// Level `level` tile `(row, col)` from its four children of `level - 1`,
/// each `Vec` of [`Grid::tile_samples`] (all inside one block, so the grid's
/// wrap and clamp rules never apply below level 4).
fn coarsen(children: [&[i16]; 4]) -> Vec<i16> {
    let n = GRID.tile_size() as usize;
    let half = n / 2;
    let mut out = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            let child = children[(i / half) * 2 + j / half];
            let (ci, cj) = (2 * (i % half), 2 * (j % half));
            out.push(mean4(
                child[ci * n + cj],
                child[ci * n + cj + 1],
                child[(ci + 1) * n + cj],
                child[(ci + 1) * n + cj + 1],
            ));
        }
    }
    out
}

/// What a band or a level wrote: manifest entries, kept level 0 tiles, and
/// the elevation range.
#[derive(Clone, Debug, Default)]
pub struct Written {
    /// Manifest entries of every tile written.
    pub entries: Vec<Entry>,
    /// Level 0 tiles kept, `(row, col)`.
    pub level0: Vec<(u32, u32)>,
    /// Lowest and highest elevation, meters.
    pub range: Option<(i16, i16)>,
}

impl Written {
    fn add(&mut self, key: TileKey, samples: Vec<i16>, out: &Path) -> Result<(), Glo30Error> {
        for &v in &samples {
            self.range = Some(match self.range {
                None => (v, v),
                Some((lo, hi)) => (lo.min(v), hi.max(v)),
            });
        }
        self.entries.push(write_tile(out, key, samples)?);
        Ok(())
    }

    fn encode(&self) -> String {
        let mut s = String::new();
        let (lo, hi) = self.range.unwrap_or((0, 0));
        s.push_str(&format!("range {lo} {hi}\n"));
        for &(r, c) in &self.level0 {
            s.push_str(&format!("land {r} {c}\n"));
        }
        for e in &self.entries {
            s.push_str(&format!(
                "tile {} {} {}\n",
                e.id,
                to_hex(&e.sha256),
                e.length
            ));
        }
        s
    }

    fn decode(text: &str) -> Result<Written, Glo30Error> {
        let mut w = Written::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            let num = |i: usize| -> Result<i64, Glo30Error> {
                f.get(i)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| err(format!("bad line {line:?}")))
            };
            match f[0] {
                "range" => w.range = Some((num(1)? as i16, num(2)? as i16)),
                "land" => w.level0.push((num(1)? as u32, num(2)? as u32)),
                "tile" => {
                    let hex = f.get(2).ok_or_else(|| err(format!("bad line {line:?}")))?;
                    let mut sha = [0u8; DIGEST_LEN];
                    for (k, b) in sha.iter_mut().enumerate() {
                        *b = u8::from_str_radix(&hex[2 * k..2 * k + 2], 16).map_err(err)?;
                    }
                    w.entries.push(Entry {
                        id: f[1].parse().map_err(err)?,
                        sha256: sha,
                        length: num(3)? as u64,
                    });
                }
                _ => return Err(err(format!("bad line {line:?}"))),
            }
        }
        Ok(w)
    }
}

/// Encodes and writes one tile unless its file already holds these bytes.
fn write_tile(out: &Path, key: TileKey, samples: Vec<i16>) -> Result<Entry, Glo30Error> {
    let tile = Tile::new(key, samples).map_err(err)?;
    let bytes = tile.encode();
    let hash = sha256(&bytes);
    let path = tile_path(out, key);
    let same = fs::read(&path).map(|e| sha256(&e) == hash).unwrap_or(false);
    if !same {
        write_atomic(&path, &bytes).map_err(err)?;
    }
    Ok(Entry {
        id: key.id(),
        sha256: hash,
        length: bytes.len() as u64,
    })
}

fn band_file(out: &Path, band: u32) -> PathBuf {
    out.join("bands").join(format!("{band}.entries"))
}

/// Whether band `band` is done in `out`.
pub fn band_done(out: &Path, band: u32) -> bool {
    band_file(out, band).exists()
}

/// Cuts band `band` (0 northernmost, [`BANDS`] in all) into `out` from the
/// raw GLO-30 tiles in `raw` (`{name}_DEM.tif` and `{name}_WBM.tif`, flat)
/// and the GEBCO grid file `gebco`, on `threads` threads (module
/// documentation). Returns what it wrote.
pub fn cut_band(
    band: u32,
    raw: &Path,
    gebco: &Path,
    out: &Path,
    threads: usize,
) -> Result<Written, Glo30Error> {
    if band >= BANDS {
        return Err(err(format!("band {band}: there are {BANDS}")));
    }
    let mut sources = Sources {
        tiles: BTreeMap::new(),
        missing: BTreeSet::new(),
    };
    let lats = band_latitudes(band);
    let mut gebco_file = GridFile::open(gebco, Grid::GEBCO).map_err(err)?;
    let gebco = Gebco::read(&mut gebco_file, band)?;

    let mut written = Written::default();
    let n = GRID.tile_size();
    for block in 0..GRID.tile_cols(4) {
        let west = -180 + 4 * block as i32;
        // Only this block's raw tiles and the column east of it (the
        // bilinear reads one pixel past a tile's east edge) are in memory:
        // a whole band does not fit.
        sources.load_window(raw, &lats, west, west + 4, threads)?;
        let any = lats
            .iter()
            .any(|&lat| (west..west + 4).any(|lon| sources.get(lat, lon).is_some()));
        let key4 = TileKey {
            level: 4,
            row: band,
            col: block,
        };
        if !any {
            // Open ocean: no fine tiles; level 4 is GEBCO sample for sample.
            let mut s = Vec::with_capacity(GRID.tile_samples());
            for i in 0..n {
                for j in 0..n {
                    s.push(gebco.at(i64::from(band * n + i), i64::from(block * n + j)) as i16);
                }
            }
            written.add(key4, s, out)?;
            continue;
        }
        // Level 0: 16 x 16 tiles.
        let keys0: Vec<(u32, u32)> = (0..16)
            .flat_map(|i| (0..16).map(move |j| (band * 16 + i, block * 16 + j)))
            .collect();
        let tiles0 = parallel(&keys0, threads, |&(r, c)| {
            level0_tile(&sources, &gebco, r, c)
        });
        let mut level: Vec<(u32, u32, Vec<i16>, bool)> = keys0
            .iter()
            .zip(tiles0)
            .map(|(&(r, c), (s, land))| (r, c, s, land))
            .collect();
        for (r, c, s, land) in &level {
            if *land {
                written.level0.push((*r, *c));
                written.add(
                    TileKey {
                        level: 0,
                        row: *r,
                        col: *c,
                    },
                    s.clone(),
                    out,
                )?;
            }
        }
        // Levels 1 to 3, and level 4.
        for k in 1..=FINE_LEVELS {
            let side = 16 >> k;
            let (r0, c0) = (band * side, block * side);
            let mut next = Vec::new();
            for i in 0..side {
                for j in 0..side {
                    let (r, c) = (r0 + i, c0 + j);
                    let child = |a: u32, b: u32| {
                        level
                            .iter()
                            .find(|t| t.0 == 2 * r + a && t.1 == 2 * c + b)
                            .expect("a child in the block")
                    };
                    let kids = [child(0, 0), child(0, 1), child(1, 0), child(1, 1)];
                    let land = kids.iter().any(|t| t.3);
                    let samples = if k < FINE_LEVELS {
                        coarsen([&kids[0].2, &kids[1].2, &kids[2].2, &kids[3].2])
                    } else {
                        // Level 4: the mean of four only under a kept level
                        // 3 tile, GEBCO elsewhere.
                        let mean = coarsen([&kids[0].2, &kids[1].2, &kids[2].2, &kids[3].2]);
                        let half = (n / 2) as usize;
                        let mut s = Vec::with_capacity(GRID.tile_samples());
                        for ii in 0..n as usize {
                            for jj in 0..n as usize {
                                let kid = kids[(ii / half) * 2 + jj / half];
                                s.push(if kid.3 {
                                    mean[ii * n as usize + jj]
                                } else {
                                    gebco.at(
                                        i64::from(r * n) + ii as i64,
                                        i64::from(c * n) + jj as i64,
                                    ) as i16
                                });
                            }
                        }
                        s
                    };
                    next.push((r, c, samples, land));
                }
            }
            for (r, c, s, land) in &next {
                if k == FINE_LEVELS || *land {
                    written.add(
                        TileKey {
                            level: k,
                            row: *r,
                            col: *c,
                        },
                        s.clone(),
                        out,
                    )?;
                }
            }
            level = next;
        }
    }
    let path = band_file(out, band);
    write_atomic(&path, written.encode().as_bytes()).map_err(err)?;
    Ok(written)
}

/// Runs `f` over `items` on `threads` threads, results in item order.
fn parallel<T: Sync, R: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = threads.clamp(1, items.len().max(1));
    let per = items.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(per.max(1))
            .map(|chunk| {
                let f = &f;
                s.spawn(move || chunk.iter().map(f).collect::<Vec<R>>())
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a worker thread does not panic"))
            .collect()
    })
}

/// What [`finish`] produced.
#[derive(Clone, Debug)]
pub struct Finished {
    /// Tiles in all levels.
    pub tiles: u64,
    /// Bytes of all tiles.
    pub bytes: u64,
    /// Level 0 tiles kept.
    pub level0: u64,
    /// SHA-256 of the manifest.
    pub manifest_hash: [u8; DIGEST_LEN],
    /// Bytes of the manifest.
    pub manifest_bytes: u64,
}

/// Once every band is done: builds levels 5 to the top from level 4, writes
/// the presence map, the manifest, and the summary.
pub fn finish(out: &Path) -> Result<Finished, Glo30Error> {
    let mut all = Written::default();
    for band in 0..BANDS {
        let text = fs::read_to_string(band_file(out, band))
            .map_err(|e| err(format!("band {band} is not done: {e}")))?;
        let w = Written::decode(&text)?;
        all.entries.extend(w.entries);
        all.level0.extend(w.level0);
        if let Some((lo, hi)) = w.range {
            all.range = Some(match all.range {
                None => (lo, hi),
                Some((a, b)) => (a.min(lo), b.max(hi)),
            });
        }
    }
    // Levels 5 and up, from the level below on disk. Only the rows of
    // source tiles one output row reads are held.
    let n = GRID.tile_size();
    for level in (FINE_LEVELS + 1)..=GRID.top_level() {
        let below = level - 1;
        let mut cache: BTreeMap<(u32, u32), Vec<i16>> = BTreeMap::new();
        for row in 0..GRID.tile_rows(level) {
            let src_rows: BTreeSet<u32> = (row * n..(row + 1) * n)
                .flat_map(|r| {
                    [
                        GRID.finer_row(level, r, 0) / n,
                        GRID.finer_row(level, r, 1) / n,
                    ]
                })
                .collect();
            cache.retain(|k, _| src_rows.contains(&k.0));
            for &sr in &src_rows {
                for sc in 0..GRID.tile_cols(below) {
                    if cache.contains_key(&(sr, sc)) {
                        continue;
                    }
                    let key = TileKey {
                        level: below,
                        row: sr,
                        col: sc,
                    };
                    let bytes =
                        fs::read(tile_path(out, key)).map_err(|e| err(format!("{key}: {e}")))?;
                    let t = Tile::decode_id(key.id(), &bytes).map_err(err)?;
                    cache.insert((sr, sc), t.samples().to_vec());
                }
            }
            for col in 0..GRID.tile_cols(level) {
                let mut s = Vec::with_capacity(GRID.tile_samples());
                for i in 0..n {
                    let r = row * n + i;
                    let src_r = [GRID.finer_row(level, r, 0), GRID.finer_row(level, r, 1)];
                    for j in 0..n {
                        let c = col * n + j;
                        let src_c = [GRID.finer_col(level, c, 0), GRID.finer_col(level, c, 1)];
                        let mut v = [0i16; 4];
                        for (k, value) in v.iter_mut().enumerate() {
                            let (sr, sc) = (src_r[k / 2], src_c[k % 2]);
                            let t = &cache[&(sr / n, sc / n)];
                            *value = t[((sr % n) * n + sc % n) as usize];
                        }
                        s.push(mean4(v[0], v[1], v[2], v[3]));
                    }
                }
                all.add(TileKey { level, row, col }, s, out)?;
            }
        }
    }
    let present: BTreeSet<(u32, u32)> = all.level0.iter().copied().collect();
    let presence = Presence::from_level0(&GRID, &present);
    write_atomic(&out.join(PRESENCE_FILE), &presence.encode()).map_err(err)?;
    let tiles = all.entries.len() as u64;
    let bytes: u64 = all.entries.iter().map(|e| e.length).sum();
    let m = Manifest::new(all.entries).map_err(err)?;
    let mbytes = manifest::encode(&m);
    write_atomic(&out.join(MANIFEST_FILE), &mbytes).map_err(err)?;
    let hash = manifest::manifest_hash(&mbytes);
    let (lo, hi) = all.range.unwrap_or((0, 0));
    let summary = format!(
        "{{\n  \"grid\": {{ \"rows\": {}, \"cols\": {}, \"tileSize\": {} }},\n  \"tiles\": {tiles},\n  \"level0Tiles\": {},\n  \"tileBytes\": {bytes},\n  \"manifestBytes\": {},\n  \"manifestHash\": \"{}\",\n  \"elevationMinMeters\": {lo},\n  \"elevationMaxMeters\": {hi}\n}}\n",
        GRID.rows(),
        GRID.cols(),
        GRID.tile_size(),
        present.len(),
        mbytes.len(),
        to_hex(&hash)
    );
    write_atomic(&out.join(SUMMARY_FILE), summary.as_bytes()).map_err(err)?;
    Ok(Finished {
        tiles,
        bytes,
        level0: present.len() as u64,
        manifest_hash: hash,
        manifest_bytes: mbytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_bands() {
        assert_eq!(tile_name(46, 7), "Copernicus_DSM_COG_10_N46_00_E007_00");
        assert_eq!(tile_name(-1, -180), "Copernicus_DSM_COG_10_S01_00_W180_00");
        assert_eq!(band_latitudes(0), [89, 88, 87, 86]);
        assert_eq!(band_latitudes(44), [-87, -88, -89, -90]);
        assert_eq!(GRID.tile_rows(4), BANDS);
        assert_eq!(GRID.tile_cols(4), 90);
        assert_eq!(GRID.spacing(4), Grid::GEBCO.spacing(0));
    }

    #[test]
    fn coarsen_is_the_mean_of_four() {
        let n = GRID.tile_samples();
        let a = vec![4i16; n];
        let b = vec![8i16; n];
        let out = coarsen([&a, &b, &a, &b]);
        let w = GRID.tile_size() as usize;
        assert_eq!(out[0], 4);
        assert_eq!(out[w - 1], 8);
    }
}
