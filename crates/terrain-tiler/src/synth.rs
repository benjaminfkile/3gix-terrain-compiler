//! A synthetic grid file in the dataset's layout.
//!
//! Writes an HDF5 file with the three variables [`crate::source`] reads:
//! `elevation` (`i16`, rows by columns, chunked and deflate compressed like
//! the dataset's NetCDF4 file), and `lat` and `lon` (`f64`, the cell
//! centers in degrees), with latitude ascending as the dataset stores it.
//! The tests cut such a file, and `terrain-tiler synth` writes one for
//! timing.

use std::path::Path;

use ndarray::{Array1, Array2};
use terrain_tiles::Grid;

use crate::source::{RowOrder, SourceError, ELEVATION, LAT, LON};

/// The synthetic elevation of data sample `(r, c)` (counted from the north
/// and from 180 degrees west), meters: a large basin and rise, ridges at two
/// scales, and a few meters of roughness, between about -11500 and 7500.
/// Neighboring samples differ by a few meters, as in real relief, so tiles
/// compress about as real ones do.
pub fn elevation(grid: &Grid, r: u32, c: u32) -> i16 {
    let (rows, cols) = (i64::from(grid.rows()), i64::from(grid.cols()));
    let (r, c) = (i64::from(r), i64::from(c));
    // An integer paraboloid over the whole grid, 7000 to -11000.
    let y = r * 2000 / rows - 1000;
    let x = c * 2000 / cols - 1000;
    let large = 7000 - (y * y + x * x / 2) * 12 / 1000;
    // Triangle waves across rows and columns, periods that do not divide
    // the tile size.
    let tri = |v: i64, period: i64, amp: i64| {
        let p = v.rem_euclid(period);
        let half = period / 2;
        (if p < half { p } else { period - p }) * 2 * amp / period - amp / 2
    };
    let ridges = tri(r, 373, 600) + tri(c + r / 3, 531, 400);
    // Roughness of plus or minus 8 meters.
    let rough = (r * 7919 + c * 104_729 + r * c) % 17 - 8;
    (large + ridges + rough) as i16
}

/// Writes a synthetic grid file of `grid`'s shape to `path`, with rows in
/// `order`, filled with [`elevation`].
pub fn write(path: &Path, grid: Grid, order: RowOrder) -> Result<(), SourceError> {
    let (rows, cols) = (grid.rows(), grid.cols());
    let file = hdf5::File::create(path)?;
    let s = 180.0 / f64::from(rows);
    let lat: Array1<f64> = (0..rows)
        .map(|i| {
            let north = 90.0 - (f64::from(i) + 0.5) * s;
            match order {
                RowOrder::NorthFirst => north,
                RowOrder::SouthFirst => -north,
            }
        })
        .collect();
    let lon: Array1<f64> = (0..cols)
        .map(|j| -180.0 + (f64::from(j) + 0.5) * s)
        .collect();
    file.new_dataset_builder().with_data(&lat).create(LAT)?;
    file.new_dataset_builder().with_data(&lon).create(LON)?;
    let chunk_rows = rows.min(256) as usize;
    let chunk_cols = cols.min(512) as usize;
    let ds = file
        .new_dataset::<i16>()
        .chunk((chunk_rows, chunk_cols))
        .deflate(4)
        .shape((rows as usize, cols as usize))
        .create(ELEVATION)?;
    // Written a few rows at a time, file row order.
    let band = 64u32;
    let mut start = 0u32;
    while start < rows {
        let n = band.min(rows - start);
        let block = Array2::from_shape_fn((n as usize, cols as usize), |(i, j)| {
            let file_row = start + i as u32;
            let north_row = match order {
                RowOrder::NorthFirst => file_row,
                RowOrder::SouthFirst => rows - 1 - file_row,
            };
            elevation(&grid, north_row, j as u32)
        });
        ds.write_slice(&block, (start as usize..(start + n) as usize, ..))?;
        start += n;
    }
    Ok(())
}
