//! One tile's bytes and its id.
//!
//! A tile is one source tile in the sense of `client-compute.md` section
//! 5.1: the hub stores its bytes under their SHA-256 and the manifest lists
//! it under its id. Both are private to this crate and the terrain module.
//!
//! # Id
//!
//! `(level << 48) | (row << 24) | col`, with `row` and `col` the tile's row
//! (from the north) and column (from 180 degrees west) on its level, each
//! below `2^24`. Bits 56 to 63 are 0. Ids ascend by level, then row, then
//! column.
//!
//! # Bytes
//!
//! One zstd frame at [`ZSTD_LEVEL`] that decompresses to exactly
//! [`raw_len`] of the tile's size bytes, the size being one of [`SIZES`]
//! (240 for the GEBCO pyramid, 960 for GLO-30; the length tells them
//! apart):
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `3GTE` |
//! | 4 | 2 | version, `u16` LE, 1 |
//! | 6 | 1 | level, `u8` |
//! | 7 | 1 | reserved, 0 |
//! | 8 | 4 | row, `u32` LE |
//! | 12 | 4 | col, `u32` LE |
//! | 16 | 2 N^2 | N x N elevations, `i16` LE, meters, north row first, west column first |
//!
//! Elevations are meters above the dataset's vertical datum, negative below
//! it. The compressed bytes depend on the zstd crate and its bundled C
//! library, which this workspace pins exactly as the core library does
//! (`zstd = "=0.14.0"`, core `docs/determinism.md`), and on the level.
//! Changing any of them changes every tile hash and so the manifest.

use core::fmt;
use std::io::Read;

use crate::grid::{GLO30_TILE_SIZE, TILE_SIZE};

/// The four magic bytes that open a tile: `3GTE` in ASCII.
pub const MAGIC: [u8; 4] = *b"3GTE";

/// The tile version this crate reads and writes.
pub const VERSION: u16 = 1;

/// Length of the tile header in bytes.
pub const HEADER_LEN: usize = 16;

/// The tile sizes a tile may have, samples per edge: [`TILE_SIZE`] (the
/// GEBCO pyramid) and [`GLO30_TILE_SIZE`]. The size is not stored; it
/// follows from the length.
pub const SIZES: [u32; 2] = [TILE_SIZE, GLO30_TILE_SIZE];

/// Length of a tile of `size` samples per edge before compression: the
/// header and the samples.
pub const fn raw_len(size: u32) -> usize {
    HEADER_LEN + 2 * (size as usize) * (size as usize)
}

/// The tile size whose samples number `n`, if it is one of [`SIZES`].
fn size_of_samples(n: usize) -> Option<u32> {
    SIZES
        .iter()
        .copied()
        .find(|&s| (s as usize) * (s as usize) == n)
}

/// The zstd level every tile is compressed at. The same value the core
/// library uses for matter sections, fixed here so the tile bytes do not
/// move if the library's ever does.
pub const ZSTD_LEVEL: i32 = 9;

/// The largest row or column a tile id holds.
pub const MAX_INDEX: u32 = (1 << 24) - 1;

/// A tile's place in the pyramid.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileKey {
    /// The level, 0 finest.
    pub level: u8,
    /// The tile row, from the north.
    pub row: u32,
    /// The tile column, from 180 degrees west.
    pub col: u32,
}

impl TileKey {
    /// The tile id, `(level << 48) | (row << 24) | col`. `row` and `col`
    /// must be at most [`MAX_INDEX`]; larger values are masked.
    pub const fn id(&self) -> u64 {
        ((self.level as u64) << 48)
            | (((self.row & MAX_INDEX) as u64) << 24)
            | ((self.col & MAX_INDEX) as u64)
    }

    /// The key of a tile id, or `None` if bits 56 to 63 are not 0.
    pub const fn from_id(id: u64) -> Option<TileKey> {
        if id >> 56 != 0 {
            return None;
        }
        Some(TileKey {
            level: (id >> 48) as u8,
            row: ((id >> 24) as u32) & MAX_INDEX,
            col: (id as u32) & MAX_INDEX,
        })
    }
}

impl fmt::Display for TileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "level {} row {} col {}", self.level, self.row, self.col)
    }
}

/// Why tile bytes did not decode, or why a tile could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileError {
    /// The bytes are not exactly one zstd frame of a tile's length.
    Compression(String),
    /// The magic is not `3GTE`.
    BadMagic([u8; 4]),
    /// The version is not [`VERSION`].
    BadVersion(u16),
    /// The reserved byte is not 0.
    ReservedNonzero(u8),
    /// The header names a tile other than the one expected.
    KeyMismatch {
        /// The tile expected.
        expected: TileKey,
        /// The tile the header names.
        found: TileKey,
    },
    /// The samples are not a square of one of the [`SIZES`].
    SampleCount(usize),
    /// A row or column above [`MAX_INDEX`].
    IndexTooLarge(TileKey),
    /// An interpolator needed a tile the caller did not supply.
    Missing(u64),
}

impl fmt::Display for TileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TileError::Compression(why) => write!(f, "tile: {why}"),
            TileError::BadMagic(m) => write!(
                f,
                "tile: magic {:02x} {:02x} {:02x} {:02x} is not 33 47 54 45",
                m[0], m[1], m[2], m[3]
            ),
            TileError::BadVersion(v) => write!(f, "tile: version {v} is not {VERSION}"),
            TileError::ReservedNonzero(v) => write!(f, "tile: reserved byte {v} is not 0"),
            TileError::KeyMismatch { expected, found } => {
                write!(f, "tile: header names {found}, expected {expected}")
            }
            TileError::SampleCount(n) => {
                write!(f, "tile: {n} samples is not a tile of {SIZES:?} squared")
            }
            TileError::IndexTooLarge(k) => write!(f, "tile: {k} does not fit a tile id"),
            TileError::Missing(id) => write!(f, "tile: id {id:#x} was not supplied"),
        }
    }
}

impl std::error::Error for TileError {}

/// One decoded tile: its key and its 240 x 240 elevations in meters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    key: TileKey,
    size: u32,
    samples: Box<[i16]>,
}

impl Tile {
    /// A tile from its key and `size * size` elevations (`size` one of
    /// [`SIZES`]), north row
    /// first, west column first.
    pub fn new(key: TileKey, samples: Vec<i16>) -> Result<Tile, TileError> {
        if key.row > MAX_INDEX || key.col > MAX_INDEX {
            return Err(TileError::IndexTooLarge(key));
        }
        let Some(size) = size_of_samples(samples.len()) else {
            return Err(TileError::SampleCount(samples.len()));
        };
        Ok(Tile {
            key,
            size,
            samples: samples.into_boxed_slice(),
        })
    }

    /// The tile's key.
    pub fn key(&self) -> TileKey {
        self.key
    }

    /// The tile's id.
    pub fn id(&self) -> u64 {
        self.key.id()
    }

    /// Every elevation, meters, north row first, west column first.
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// The elevation at `row`, `col` inside the tile, meters. Both must be
    /// below [`Tile::size`].
    pub fn get(&self, row: u32, col: u32) -> i16 {
        assert!(row < self.size && col < self.size, "inside the tile");
        self.samples[(row * self.size + col) as usize]
    }

    /// Samples along each edge.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The uncompressed bytes: header and samples ([`raw_len`] bytes).
    pub fn raw_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(raw_len(self.size));
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.push(self.key.level);
        out.push(0);
        out.extend_from_slice(&self.key.row.to_le_bytes());
        out.extend_from_slice(&self.key.col.to_le_bytes());
        for &s in self.samples.iter() {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// The tile bytes the hub stores: [`Tile::raw_bytes`] as one zstd frame
    /// at [`ZSTD_LEVEL`]. Identical tiles give identical bytes.
    pub fn encode(&self) -> Vec<u8> {
        zstd::bulk::compress(&self.raw_bytes(), ZSTD_LEVEL)
            .expect("in-memory zstd compression of a bounded block does not fail")
    }

    /// Decodes tile bytes: exactly one zstd frame of a tile's length, the
    /// magic, the version, and the reserved byte.
    pub fn decode(bytes: &[u8]) -> Result<Tile, TileError> {
        let raw = decompress(bytes)?;
        let magic: [u8; 4] = raw[0..4].try_into().expect("4 bytes");
        if magic != MAGIC {
            return Err(TileError::BadMagic(magic));
        }
        let version = u16::from_le_bytes([raw[4], raw[5]]);
        if version != VERSION {
            return Err(TileError::BadVersion(version));
        }
        if raw[7] != 0 {
            return Err(TileError::ReservedNonzero(raw[7]));
        }
        let key = TileKey {
            level: raw[6],
            row: u32::from_le_bytes(raw[8..12].try_into().expect("4 bytes")),
            col: u32::from_le_bytes(raw[12..16].try_into().expect("4 bytes")),
        };
        let samples = raw[HEADER_LEN..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect();
        Tile::new(key, samples)
    }

    /// Decodes tile bytes and checks the header names the tile with `id`.
    pub fn decode_id(id: u64, bytes: &[u8]) -> Result<Tile, TileError> {
        let tile = Tile::decode(bytes)?;
        if tile.id() != id {
            let expected = TileKey::from_id(id).unwrap_or(TileKey {
                level: u8::MAX,
                row: MAX_INDEX,
                col: MAX_INDEX,
            });
            return Err(TileError::KeyMismatch {
                expected,
                found: tile.key,
            });
        }
        Ok(tile)
    }
}

/// Exactly one zstd frame that yields exactly the length of a tile of one of
/// the [`SIZES`].
fn decompress(bytes: &[u8]) -> Result<Vec<u8>, TileError> {
    let frame_len = zstd::zstd_safe::find_frame_compressed_size(bytes).map_err(|e| {
        TileError::Compression(format!(
            "no readable zstd frame ({})",
            zstd::zstd_safe::get_error_name(e)
        ))
    })?;
    if frame_len != bytes.len() {
        return Err(TileError::Compression(format!(
            "{} bytes follow the {frame_len} byte zstd frame",
            bytes.len() - frame_len
        )));
    }
    let invalid = |e: std::io::Error| TileError::Compression(format!("zstd frame ({e})"));
    let decoder = zstd::stream::read::Decoder::with_buffer(bytes)
        .map_err(invalid)?
        .single_frame();
    let max = SIZES.iter().map(|&s| raw_len(s)).max().unwrap_or(0);
    let mut out = Vec::new();
    decoder
        .take(max as u64 + 1)
        .read_to_end(&mut out)
        .map_err(invalid)?;
    if !SIZES.iter().any(|&s| raw_len(s) == out.len()) {
        let lens: Vec<usize> = SIZES.iter().map(|&s| raw_len(s)).collect();
        return Err(TileError::Compression(format!(
            "zstd frame decompresses to {}{} bytes, a tile is one of {lens:?}",
            if out.len() > max { "more than " } else { "" },
            out.len().min(max)
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(key: TileKey) -> Tile {
        let samples = (0..crate::grid::TILE_SAMPLES as i32)
            .map(|i| ((i * 37) % 20001 - 11000) as i16)
            .collect();
        Tile::new(key, samples).unwrap()
    }

    #[test]
    fn ids() {
        let k = TileKey {
            level: 3,
            row: 22,
            col: 44,
        };
        assert_eq!(k.id(), (3 << 48) | (22 << 24) | 44);
        assert_eq!(TileKey::from_id(k.id()), Some(k));
        assert_eq!(TileKey::from_id(1 << 56), None);
        let a = TileKey {
            level: 0,
            row: 1,
            col: 0,
        }
        .id();
        let b = TileKey {
            level: 0,
            row: 0,
            col: 359,
        }
        .id();
        let c = TileKey {
            level: 1,
            row: 0,
            col: 0,
        }
        .id();
        assert!(b < a && a < c);
    }

    #[test]
    fn round_trip() {
        let t = ramp(TileKey {
            level: 2,
            row: 5,
            col: 7,
        });
        let bytes = t.encode();
        assert_eq!(bytes, t.encode());
        assert_eq!(Tile::decode(&bytes).unwrap(), t);
        assert_eq!(Tile::decode_id(t.id(), &bytes).unwrap(), t);
        assert!(matches!(
            Tile::decode_id(t.id() + 1, &bytes),
            Err(TileError::KeyMismatch { .. })
        ));
        let raw = t.raw_bytes();
        assert_eq!(raw.len(), raw_len(TILE_SIZE));
        assert_eq!(&raw[0..8], b"3GTE\x01\x00\x02\x00");
        assert_eq!(&raw[8..16], &[5, 0, 0, 0, 7, 0, 0, 0]);
        assert_eq!(&raw[16..18], &(-11000i16).to_le_bytes());
    }

    #[test]
    fn rejects_bad_bytes() {
        let t = ramp(TileKey {
            level: 0,
            row: 0,
            col: 0,
        });
        let mut bytes = t.encode();
        bytes.push(0);
        assert!(matches!(
            Tile::decode(&bytes),
            Err(TileError::Compression(_))
        ));
        assert!(matches!(
            Tile::decode(b"nope"),
            Err(TileError::Compression(_))
        ));
        let mut raw = t.raw_bytes();
        raw[0] = b'X';
        let z = zstd::bulk::compress(&raw, ZSTD_LEVEL).unwrap();
        assert!(matches!(Tile::decode(&z), Err(TileError::BadMagic(_))));
        let mut raw = t.raw_bytes();
        raw[7] = 1;
        let z = zstd::bulk::compress(&raw, ZSTD_LEVEL).unwrap();
        assert_eq!(Tile::decode(&z), Err(TileError::ReservedNonzero(1)));
        let raw = t.raw_bytes();
        let z = zstd::bulk::compress(&raw[..raw_len(TILE_SIZE) - 2], ZSTD_LEVEL).unwrap();
        assert!(matches!(Tile::decode(&z), Err(TileError::Compression(_))));
        assert_eq!(
            Tile::new(t.key(), vec![0; 3]),
            Err(TileError::SampleCount(3))
        );
    }
}
