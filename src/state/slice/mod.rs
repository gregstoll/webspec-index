//! Views of one algorithm (spec: 2026-09-25-state-slicing-design.md): a per-algorithm slice index
//! built at index time, and query-time slices rendered by cutting the stored section markdown.
pub mod index;

pub use index::{DefEdge, DefKind, SliceIndex, SliceStep};
