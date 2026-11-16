//! The terrain tile format, shared by the tiler and the terrain module.
//!
//! A source set in the sense of `client-compute.md` section 5 (source data):
//! an elevation grid cut into a pyramid of square tiles, each stored by the
//! hub as opaque bytes under its SHA-256 and listed in a source manifest
//! (section 5.1) under an integer id. The ids and the bytes are private to
//! this crate and the module that reads them (section 5.1); the hub and the
//! worker never look inside.
//!
//! - [`grid`]: the geometry of the pyramid. Levels, tile counts, the
//!   sample lattice, how a coarser level is built from the one below, and
//!   where a geodetic latitude and longitude falls on a level.
//! - [`tile`]: one tile's bytes (`3GTE`, version 1) and its id.
//! - [`interp`]: nearest sample and bicubic (Catmull-Rom) interpolation
//!   over a level, across tile edges, from tiles the caller supplies, and
//!   the same with a fallback to coarser levels where a fine tile does not
//!   exist.
//! - [`presence`]: which fine tiles of a sparse pyramid (GLO-30, land
//!   only) exist.
//! - [`check`]: a fixed digest over encoding, decoding, and interpolation,
//!   used to prove that native code and WebAssembly agree byte for byte.
//!
//! Every function here is deterministic (`space-model.md` section 1 rule 7).
//! Encoding is integer work plus zstd at a fixed level with the same zstd pin
//! as the core library. Interpolation uses only `+`, `-`, `*`, `/`, and
//! `floor`, which are exact or correctly rounded in IEEE 754 on every target,
//! with no fused multiply add, so it needs nothing from `gx_core::detmath` and
//! gives the same bits natively and in WebAssembly. All angles are radians
//! and all elevations meters.

#![warn(missing_docs)]

pub mod check;
pub mod grid;
pub mod interp;
pub mod presence;
pub mod tile;

pub use grid::{Grid, Position};
pub use interp::{bicubic, bicubic_present, nearest, TileSource};
pub use presence::Presence;
pub use tile::{Tile, TileError, TileKey};
