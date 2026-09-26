//! State analysis: object model, minimal statement IR, field writers.
//!
//! Layering: `parse::steps` → `state` → (later) `effects`. This module never
//! imports the effects layer; shared rule primitives live in `crate::semantics`.
pub(crate) mod block;
pub(crate) mod classify;
pub(crate) mod declare;
pub mod extract;
pub mod ir;
pub mod lookup;
pub mod model;
pub mod testing;
pub(crate) mod typeexpr;
pub(crate) mod types;

pub use extract::{derive_occurrence_counts, derive_sites, extract_state, StateInputs};
pub use model::*;
