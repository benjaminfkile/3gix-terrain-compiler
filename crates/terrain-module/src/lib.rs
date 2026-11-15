//! The terrain module: builds the whole matter of one frame from the
//! elevation tile pyramid and a radial interior profile.
//!
//! This crate is a compiler module in the sense of `space-model.md` section
//! 2: a WebAssembly binary with no imports, a pure function from a request
//! and the source tiles it asked for to one section, built with
//! `gx-module-guest` to the module ABI v1 of `client-compute.md` section 4.
//! It implements `space-model.md` section 11 item 2: solid rock up to the
//! elevation surface on the reference ellipsoid, a radial interior density
//! profile below, no water.
//!
//! - [`TerrainModule`]: the module. `gx_sources` lists the tiles a cell
//!   needs ([`plan`]); `gx_compile` builds the cell ([`compile`]). The
//!   registry key gets the empty registry.
//! - [`frame`]: which frame the module fills, its size, the depth and
//!   resolution of its cells.
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
//! does not depend on time. Every output is a function of the chunk key and
//! the tiles alone, and identical inputs give identical bytes natively and
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

use gx_module_guest::{gx_module, Module, Request, Tiles};

pub use compile::{compile, sources, CompileError};
pub use plan::{Plan, PlanError};

/// The terrain module (`client-compute.md` section 4).
///
/// The request's origin and span are ignored: the layer's `usesOrigin` is
/// false and its `spanSeconds` 0.
pub struct TerrainModule;

impl Module for TerrainModule {
    /// The tiles the request's cell needs; none for the registry and for
    /// cells entirely above or entirely below the elevation shell.
    fn sources(req: &Request) -> Vec<u64> {
        compile::sources(&req.key)
    }

    /// Decodes the tiles and compiles the request's key.
    fn compile(req: &Request, tiles: &Tiles) -> Result<Vec<u8>, String> {
        let decoded = compile::decode_tiles(tiles.as_slice().iter().map(|t| (t.id, t.bytes)))
            .map_err(|e| format!("{}: {e}", req.key))?;
        compile::compile(&req.key, &decoded).map_err(|e| format!("{}: {e}", req.key))
    }
}

gx_module!(TerrainModule);
