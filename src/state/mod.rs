//! State analysis: object model, minimal statement IR, field writers.
//!
//! Layering: `parse::steps` → `state` → (later) `effects`. This module never
//! imports the effects layer; shared rule primitives live in `crate::semantics`.
pub mod ir;
pub mod model;

pub use model::*;
