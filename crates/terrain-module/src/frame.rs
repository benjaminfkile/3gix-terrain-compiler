//! The frame this module builds and the frames it only answers for.
//!
//! Frame ids are a convention between compiler authors (`matter-format.md`
//! section 5.2): the system module declares the frames, this module attaches
//! matter to one of them. The values below are copied from the system
//! module's data (`data/system.toml` in the system compiler repository):
//! frame 4 is the third planet's frame, with `root_extent` 5.096808e7 m
//! (8 times its mean radius) and the body axes described in
//! [`crate::geodesy`].
//!
//! The layer is attached to [`FRAME_ID`] alone (`space-model.md` section 8),
//! so the hub never directs it for another frame. If asked anyway, the
//! module answers every cell of the other frames the system module declares
//! with an empty section of the right geometry.

/// The frame whose whole matter this module builds.
pub const FRAME_ID: u64 = 4;

/// The frame's `root_extent`, meters.
pub const ROOT_EXTENT: f64 = 5.096808e7;

/// The deepest cell the module fills: at depth 21 a cell is 24.3 m on an
/// edge and its samples 0.76 m apart. Deeper cells are empty sections.
pub const MAX_DEPTH: u8 = 21;

/// Samples per axis of every non-empty section.
pub const RESOLUTION: u8 = 32;

/// Sub-sampling points per axis inside one sample's sub-cube, as in the
/// system module: each sample averages `SUBSAMPLES^3` stratified points.
pub const SUBSAMPLES: u32 = 4;

/// `(frame_id, root_extent in meters)` of every frame the system module
/// declares, ascending.
pub const SYSTEM_FRAMES: [(u64, f64); 11] = [
    (0, 2.0e13),
    (1, 5.5656e9),
    (2, 1.95152e7),
    (3, 4.841472e7),
    (4, ROOT_EXTENT),
    (5, 2.711936e7),
    (6, 5.59288e8),
    (7, 4.65856e8),
    (8, 2.02896e8),
    (9, 1.96992e8),
    (10, 1.390024e7),
];

/// The `root_extent` of a frame the system module declares, or `None`.
pub fn root_extent(frame_id: u64) -> Option<f64> {
    SYSTEM_FRAMES
        .iter()
        .find(|&&(id, _)| id == frame_id)
        .map(|&(_, e)| e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_sample_spacing_at_max_depth_is_under_a_meter() {
        let spacing = ROOT_EXTENT / f64::from(1u32 << MAX_DEPTH) / f64::from(RESOLUTION);
        assert!(spacing < 1.0 && spacing > 0.5, "{spacing}");
        assert_eq!(root_extent(FRAME_ID), Some(ROOT_EXTENT));
        assert_eq!(root_extent(11), None);
    }
}
