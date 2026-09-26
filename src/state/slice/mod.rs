//! Views of one algorithm (spec: 2026-09-25-state-slicing-design.md): a per-algorithm slice index
//! built at index time, and query-time slices rendered by cutting the stored section markdown.
pub mod index;
pub mod render;
pub mod select;

pub use index::{build_slice_indexes, DefEdge, DefKind, SliceIndex, SliceStep};
pub use render::{marker_text, render_view, RenderedView, Rendering};
pub use select::{
    slice, validate_shape, FeedingSelector, KeptStep, LaterDefinition, OmittedRun, Rebound, Slice,
    SliceError, SliceErrorCode, SliceVariable, StepRole, StoreNote, Unfollowed, UnfollowedReason,
    VarBasis, ViewRequest,
};
