//! The terrain tiler: cuts the elevation dataset into the tile pyramid of
//! [`terrain_tiles`] and uploads it to a hub as a source set.
//!
//! This is the producing side of `client-compute.md` section 5 (source
//! data): section 5.3 says a source set is made by a tool in the compiler's
//! repository that reads the raw dataset from local disk, writes tiles,
//! uploads each with `PUT /source/{hash}`, then uploads the manifest of
//! section 5.1.
//!
//! - [`source`]: the dataset file (NetCDF4, which is HDF5) and its checks.
//! - [`synth`]: a synthetic grid file in the same layout, for tests and
//!   timing.
//! - [`cut`]: the deterministic, resumable, streaming cutter.
//! - [`upload`]: the uploader.

#![warn(missing_docs)]

pub mod cut;
pub mod source;
pub mod synth;
pub mod upload;
