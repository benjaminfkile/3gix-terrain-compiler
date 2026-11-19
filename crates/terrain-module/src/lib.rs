//! The terrain module: builds the whole matter of one frame from the
//! elevation tile pyramid and a radial interior profile.
//!
//! This crate is a compiler module in the sense of `space-model.md` section
//! 2: a WebAssembly binary with no imports, a pure function from a request
//! the frame record of its frame, and the source tiles it asked for to one
//! section, built with `gx-module-guest` to the module ABI v2 of
//! `client-compute.md` sections 4.1 and 10.
//! It implements `space-model.md` section 11 item 2: solid rock up to the
//! elevation surface on the reference ellipsoid, a radial interior density
//! profile below, no water.
//!
//! - [`TerrainModule`]: the module. Its layer reads one input, a frames
//!   slot with the record of the frame it fills, which the layer config
//!   chooses: `"inputs": [{"layer": "system", "frames": [4]}]` for the
//!   system module's frame 4. `gx_inputs` asks for
//!   no sections; `gx_sources` lists the tiles a cell needs ([`plan`]);
//!   `gx_compile` builds the cell ([`compile`]). The registry key gets the
//!   empty registry and a frame key the empty record.
//! - [`frame`]: which frame the module fills, its size from the frame
//!   record ([`frame::Frame`]), the depth and resolution of its cells.
//! - [`geodesy`]: the reference ellipsoid, frame coordinates to geodetic
//!   latitude, longitude, and height, deterministic inverse tangent.
//! - [`plan`]: what one cell needs: nothing, the interior alone, or one
//!   pyramid level and its tiles.
//! - [`surface`]: the bicubic elevation surface of one level.
//! - [`interior`]: density (the Preliminary Reference Earth Model) and
//!   temperature (a geotherm) below the surface.
//! - [`compile`](mod@compile): sampling a cell into a validated section.
//! - `fixture` and `conformance` (native only): the synthetic tiles and the
//!   conformance vectors.
//!
//! The layer runs with `usesOrigin` false and `spanSeconds` 0: the matter
//! does not depend on time. Every output is a function of the chunk key,
//! the frame record, and the tiles alone, and identical inputs give identical bytes natively and
//! in WebAssembly (`space-model.md` section 1 rule 7).

#![warn(missing_docs)]

pub mod compile;
#[cfg(not(target_arch = "wasm32"))]
pub mod conformance;
#[cfg(not(target_arch = "wasm32"))]
pub mod fixture;
pub mod frame;
pub mod geodesy;
pub mod interior;
pub mod plan;
pub mod surface;
pub mod surface_tile;

use gx_core::frame_record::{self as record, FrameRecord};
use gx_core::key::ChunkKey;
use gx_core::module_io::InputEntry;
use gx_module_guest::{gx_module_v2, FramesSlot, Input, ModuleV2, Request, Tiles};

pub use compile::{compile, sources, CompileError};
pub use frame::{Frame, FrameError};
pub use plan::Plan;

/// The terrain module (`client-compute.md` sections 4.1 and 10).
///
/// The request's origin and span are ignored: the layer's `usesOrigin` is
/// false and its `spanSeconds` 0. Every key but a frame key needs the one
/// frame record, the non-empty record of the frame the layer fills
/// ([`Frame::from_records`]); otherwise `gx_sources`
/// lists no tiles and `gx_compile` fails with the reason.
pub struct TerrainModule;

impl ModuleV2 for TerrainModule {
    /// Slot 0 lists one frame, the one the layer fills.
    const FRAMES_SLOTS: &'static [FramesSlot] = &[FramesSlot { slot: 0, frames: 1 }];

    /// The layer has no layer slots: always empty.
    fn inputs(_req: &Request, _frames: &[FrameRecord]) -> Vec<InputEntry> {
        Vec::new()
    }

    /// The tiles the request's cell needs; none for the registry, a frame
    /// key, cells entirely above or entirely below the elevation shell, and
    /// records that do not describe the frame.
    fn sources(req: &Request, frames: &[FrameRecord]) -> Vec<u64> {
        match Frame::from_records(frames) {
            Ok(f) => compile::sources(&f, &req.key),
            Err(_) => Vec::new(),
        }
    }

    /// Decodes the tiles and compiles the request's key.
    fn compile(
        req: &Request,
        tiles: &Tiles,
        frames: &[FrameRecord],
        _inputs: &[Input],
    ) -> Result<Vec<u8>, String> {
        if let ChunkKey::Frame(id) = req.key {
            return Ok(record::encode(&FrameRecord::empty(id)));
        }
        let frame = Frame::from_records(frames).map_err(|e| format!("{}: {e}", req.key))?;
        let decoded = compile::decode_tiles(tiles.as_slice().iter().map(|t| (t.id, t.bytes)))
            .map_err(|e| format!("{}: {e}", req.key))?;
        compile::compile(&frame, &req.key, &decoded).map_err(|e| format!("{}: {e}", req.key))
    }
}

gx_module_v2!(TerrainModule);
