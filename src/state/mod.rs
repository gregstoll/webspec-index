//! State analysis: object model, minimal statement IR, field writers.
//!
//! Layering: `parse::steps` → `state` → (later) `effects`. This module never
//! imports the effects layer; shared rule primitives live in `crate::semantics`.
pub(crate) mod block;
pub(crate) mod call;
pub mod catalog;
pub(crate) mod classify;
pub(crate) mod declare;
pub(crate) mod def_sig;
pub(crate) mod expr;
pub mod extract;
pub(crate) mod grammar;
pub(crate) mod idl_sig;
pub(crate) mod intro;
pub mod ir;
pub mod lookup;
pub mod model;
pub(crate) mod names;
pub(crate) mod predicate;
pub(crate) mod prose;
pub mod query;
pub(crate) mod reflect;
pub mod render;
pub(crate) mod rules;
#[cfg(feature = "native")]
pub mod service;
pub(crate) mod signature;
pub mod slice;
pub mod testing;
pub(crate) mod typeexpr;
pub(crate) mod types;

pub use extract::{derive_occurrence_counts, derive_sites, extract_state, StateInputs};
pub use model::*;
