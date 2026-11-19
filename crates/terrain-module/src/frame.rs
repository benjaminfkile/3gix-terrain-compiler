//! The frame this module fills, its size from the frame record the layer
//! reads, and the depth and resolution of its cells.
//!
//! Frame ids are a convention between compiler authors (`matter-format.md`
//! section 5.2): the system module declares the frames, this module attaches
//! matter to one of them. The layer is attached to [`FRAME_ID`] alone
//! (`space-model.md` section 8) and reads that frame's record from the
//! system layer through its one frames slot (`client-compute.md` sections
//! 4.1 and 10.2, `matter-format.md` section 11). The frame's size,
//! `root_extent`, comes from the record alone: the module holds no copy of
//! it.
//!
//! The hub never directs the layer for another frame. If asked anyway, the
//! module answers every cell and surface key of another frame with an empty
//! section or tile; it knows no other frame's size, so an empty cell
//! section of another frame carries the cube its key names in a frame of
//! the record's `root_extent`.

use gx_core::frame_record::FrameRecord;

/// The frame whose whole matter this module builds: the layer's attachment.
pub const FRAME_ID: u64 = 4;

/// The deepest cell the module fills: at depth 21 a cell is 24.3 m on an
/// edge and its samples 0.76 m apart for the frame's `root_extent` of
/// 5.096808e7 m. Deeper cells are empty sections.
pub const MAX_DEPTH: u8 = 21;

/// Samples per axis of every non-empty section.
pub const RESOLUTION: u8 = 32;

/// Sub-sampling points per axis inside one sample's sub-cube, as in the
/// system module: each sample averages `SUBSAMPLES^3` stratified points.
pub const SUBSAMPLES: u32 = 4;

/// The frame the module fills, as its frame record describes it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Frame {
    /// Edge of the frame's root cube, meters.
    pub root_extent: f64,
}

/// Why the frame records passed to the module do not describe its frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The number of records is not one: the layer has one frames slot
    /// listing one frame.
    Count(usize),
    /// The record is empty: its layer's module does not declare the frame.
    Empty(u64),
    /// The record describes another frame than [`FRAME_ID`].
    OtherFrame(u64),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::Count(n) => write!(
                f,
                "{n} frame records passed; the module reads exactly the record of frame {FRAME_ID}"
            ),
            FrameError::Empty(id) => write!(
                f,
                "the record of frame {id} is empty: the system layer does not declare the frame, so its root extent is unknown"
            ),
            FrameError::OtherFrame(id) => write!(
                f,
                "the frame record is of frame {id}; the module fills frame {FRAME_ID} and reads its record"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    /// The frame of a record: the record must be the non-empty record of
    /// [`FRAME_ID`].
    pub fn from_record(record: &FrameRecord) -> Result<Frame, FrameError> {
        if record.frame_id != FRAME_ID {
            return Err(FrameError::OtherFrame(record.frame_id));
        }
        if record.empty {
            return Err(FrameError::Empty(record.frame_id));
        }
        Ok(Frame {
            root_extent: record.root_extent.value(),
        })
    }

    /// The frame of the records passed to the module: exactly one, the
    /// record of [`FRAME_ID`] (see [`Frame::from_record`]).
    pub fn from_records(records: &[FrameRecord]) -> Result<Frame, FrameError> {
        match records {
            [r] => Frame::from_record(r),
            _ => Err(FrameError::Count(records.len())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gx_core::units::Meters;

    fn record(frame_id: u64, root_extent: f64) -> FrameRecord {
        FrameRecord {
            root_extent: Meters::new(root_extent),
            empty: false,
            max_depth: 6,
            ..FrameRecord::empty(frame_id)
        }
    }

    #[test]
    fn surface_sample_spacing_at_max_depth_is_under_a_meter() {
        let f = Frame::from_record(&record(FRAME_ID, 5.096808e7)).unwrap();
        let spacing = f.root_extent / f64::from(1u32 << MAX_DEPTH) / f64::from(RESOLUTION);
        assert!(spacing < 1.0 && spacing > 0.5, "{spacing}");
    }

    #[test]
    fn records_that_do_not_describe_the_frame_fail() {
        assert_eq!(
            Frame::from_record(&record(3, 1e7)),
            Err(FrameError::OtherFrame(3))
        );
        assert_eq!(
            Frame::from_record(&FrameRecord::empty(FRAME_ID)),
            Err(FrameError::Empty(FRAME_ID))
        );
        assert_eq!(Frame::from_records(&[]), Err(FrameError::Count(0)));
        let r = record(FRAME_ID, 1e7);
        assert_eq!(Frame::from_records(&[r, r]), Err(FrameError::Count(2)));
        assert_eq!(Frame::from_records(&[r]), Ok(Frame { root_extent: 1e7 }));
    }
}
