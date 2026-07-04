//! Similarity IR — the purpose-built normalization substrate (design:
//! `docs/SIMILARITY-IR.md`). Front-half greenfield: per-language frontends lower
//! tree-sitter CST directly to a canonical form, so the loop/recursion/iteration/
//! order algorithms currently copied across `src/lang/*.rs` live once on the IR.
//!
//! Under D-IR-1(a) the IR reuses [`crate::tree::NormNode`]'s shape; a "kind" is one
//! of the closed canonical vocabulary in [`kind`]. Normalization is recorded as an
//! append-only [`transform`] event log (in memory only, never persisted — D-IR-10),
//! and every node keeps [`provenance`] back to source so evidence renders in the
//! user's own syntax.
//!
//! Pipeline integration (replacing `normalize::convert` + the per-language passes)
//! is gated behind a build flag per D-IR-3 and lands in a later increment. This
//! module is the P1 foundation: the vocabulary, the event log, and provenance.

pub mod kind;
pub mod pass;
pub mod provenance;
pub mod render;
pub mod transform;

pub use pass::{abstract_idents, canonicalize_order};
pub use provenance::Provenance;
pub use render::to_sexpr;
pub use transform::{TransformEvent, TransformKind, TransformLog, Witness};
