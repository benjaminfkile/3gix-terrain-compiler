//! The dataset file: one `elevation` variable of `i16` with `lat` and `lon`
//! coordinate variables, in NetCDF4 (an HDF5 file).
//!
//! [`GridFile::open`] checks, before anything is cut, that the variable has
//! the shape of the expected [`Grid`] and the `i16` type, that `lat` and
//! `lon` have one value per row and column, and that their first and last
//! values are the cell centers the grid says (`terrain_tiles::grid`, the
//! data grid). Latitude may ascend (south row first, as the dataset's
//! NetCDF file stores it) or descend; [`RowSource::read_rows`] always
//! returns rows north first.

use std::fmt;
use std::path::Path;

use terrain_tiles::Grid;

/// Rows of the data grid, north row first.
pub trait RowSource {
    /// Data rows `start` to `start + count - 1` (counted from the north),
    /// all columns, west column first, row after row.
    fn read_rows(&mut self, start: u32, count: u32) -> Result<Vec<i16>, SourceError>;
}

/// Why the dataset could not be opened or read.
#[derive(Debug)]
pub struct SourceError(pub String);

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SourceError {}

impl From<hdf5::Error> for SourceError {
    fn from(e: hdf5::Error) -> Self {
        SourceError(format!("hdf5: {e}"))
    }
}

/// The name of the elevation variable.
pub const ELEVATION: &str = "elevation";
/// The name of the latitude coordinate variable, degrees north.
pub const LAT: &str = "lat";
/// The name of the longitude coordinate variable, degrees east.
pub const LON: &str = "lon";

/// The order of the rows in the file.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RowOrder {
    /// The first row is the southernmost (latitude ascends).
    SouthFirst,
    /// The first row is the northernmost (latitude descends).
    NorthFirst,
}

/// An open, checked dataset file.
pub struct GridFile {
    grid: Grid,
    order: RowOrder,
    elevation: hdf5::Dataset,
    _file: hdf5::File,
}

impl GridFile {
    /// Opens `path` and checks it against `grid` (module documentation).
    pub fn open(path: &Path, grid: Grid) -> Result<GridFile, SourceError> {
        // A large chunk cache, so a chunk taller than one band of rows is
        // decompressed once rather than once per band.
        let file = hdf5::File::with_options()
            .with_fapl(|p| p.chunk_cache(100_003, 512 << 20, 1.0))
            .open(path)?;
        let elevation = file.dataset(ELEVATION)?;
        let want = [grid.rows() as usize, grid.cols() as usize];
        if elevation.shape() != want {
            return Err(SourceError(format!(
                "{ELEVATION}: shape {:?}, expected {want:?}",
                elevation.shape()
            )));
        }
        if !elevation.dtype()?.is::<i16>() {
            return Err(SourceError(format!(
                "{ELEVATION}: not a 16-bit signed integer"
            )));
        }
        let s = 180.0 / f64::from(grid.rows());
        let (north, south) = (90.0 - s / 2.0, -90.0 + s / 2.0);
        let (west, east) = (-180.0 + s / 2.0, 180.0 - s / 2.0);
        let (lat0, lat1) = ends(&file, LAT, grid.rows())?;
        let (lon0, lon1) = ends(&file, LON, grid.cols())?;
        let near = |a: f64, b: f64| (a - b).abs() <= s * 1e-3;
        let order = if near(lat0, south) && near(lat1, north) {
            RowOrder::SouthFirst
        } else if near(lat0, north) && near(lat1, south) {
            RowOrder::NorthFirst
        } else {
            return Err(SourceError(format!(
                "{LAT}: runs from {lat0} to {lat1}, expected {south} to {north} or back"
            )));
        };
        if !(near(lon0, west) && near(lon1, east)) {
            return Err(SourceError(format!(
                "{LON}: runs from {lon0} to {lon1}, expected {west} to {east}"
            )));
        }
        Ok(GridFile {
            grid,
            order,
            elevation,
            _file: file,
        })
    }

    /// The grid the file was checked against.
    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// The order of the rows in the file.
    pub fn order(&self) -> RowOrder {
        self.order
    }
}

/// The first and last values of a one-dimensional coordinate variable of
/// length `len`.
fn ends(file: &hdf5::File, name: &str, len: u32) -> Result<(f64, f64), SourceError> {
    let ds = file.dataset(name)?;
    if ds.shape() != [len as usize] {
        return Err(SourceError(format!(
            "{name}: shape {:?}, expected [{len}]",
            ds.shape()
        )));
    }
    let len = len as usize;
    let first = ds.read_slice_1d::<f64, _>(0..1)?;
    let last = ds.read_slice_1d::<f64, _>(len - 1..len)?;
    Ok((first[0], last[0]))
}

impl RowSource for GridFile {
    fn read_rows(&mut self, start: u32, count: u32) -> Result<Vec<i16>, SourceError> {
        let rows = self.grid.rows();
        if count == 0 || start.checked_add(count).is_none_or(|end| end > rows) {
            return Err(SourceError(format!(
                "rows {start} to {} are outside the grid",
                u64::from(start) + u64::from(count)
            )));
        }
        let (start, count) = (start as usize, count as usize);
        let cols = self.grid.cols() as usize;
        match self.order {
            RowOrder::NorthFirst => {
                let a = self
                    .elevation
                    .read_slice_2d::<i16, _>((start..start + count, ..))?;
                Ok(a.as_standard_layout().iter().copied().collect())
            }
            RowOrder::SouthFirst => {
                let rows = rows as usize;
                let (lo, hi) = (rows - start - count, rows - start);
                let a = self.elevation.read_slice_2d::<i16, _>((lo..hi, ..))?;
                let a = a.as_standard_layout();
                let flat = a.as_slice().expect("standard layout");
                let mut out = Vec::with_capacity(count * cols);
                for row in flat.chunks_exact(cols).rev() {
                    out.extend_from_slice(row);
                }
                Ok(out)
            }
        }
    }
}

/// A grid held in memory, north row first: for tests and small inputs.
pub struct MemorySource {
    /// The grid.
    pub grid: Grid,
    /// Every sample, north row first, west column first.
    pub samples: Vec<i16>,
}

impl RowSource for MemorySource {
    fn read_rows(&mut self, start: u32, count: u32) -> Result<Vec<i16>, SourceError> {
        let cols = self.grid.cols() as usize;
        let (a, b) = (start as usize * cols, (start + count) as usize * cols);
        self.samples
            .get(a..b)
            .map(<[i16]>::to_vec)
            .ok_or_else(|| SourceError(format!("rows {start} to {} outside", start + count)))
    }
}
