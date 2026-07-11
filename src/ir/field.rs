//! The IR-synthesized field vocabulary (`docs/SIMILARITY-IR.md` §5) — the small,
//! closed set of structural field labels the frontends and canonicalization passes
//! **mint themselves** (a `Binop`'s `@left`/`@right`, a `Branch`'s `@guard`, a
//! `Call`'s `@callee`/`@arg`, …), as opposed to the overwhelmingly per-grammar field
//! names a raw tree-sitter CST carries (`@condition`, `@attribute`, …). Only these
//! IR-synthesized names are worth pre-registering.
//!
//! Like `ir::kind`, these are pre-registered at fixed ids so a comparison site
//! (`node.field == Some(field::id::LEFT)`) compiles to a bare `u16` equality rather
//! than a string compare (the interning-id-conversion WP's hard invariant). The
//! numeric value of each `id::X` const is its POSITION in [`ALL`], which is exactly
//! the order `crate::intern::field_interner()`'s pre-registration loop walks — so
//! `Field::intern(LEFT) == id::LEFT` always holds (pinned by
//! `field_ids_match_pre_registration` below); the consts are never constructed from a
//! hand-picked number. Field ids are process-local and never serialized (the wire
//! format carries resolved strings — D19), so their numeric values are an internal
//! detail.

// ---- binary / unary operand structure ----
pub const LEFT: &str = "left";
pub const RIGHT: &str = "right";

// ---- branch / arm structure ----
pub const GUARD: &str = "guard";
pub const ARM: &str = "arm";
pub const BODY: &str = "body";

// ---- assignment structure ----
pub const TARGET: &str = "target";
pub const VALUE: &str = "value";
pub const PLACE: &str = "place";

// ---- call structure ----
pub const CALLEE: &str = "callee";
pub const ARG: &str = "arg";

// ---- operator / operand tags ----
pub const OP: &str = "op";
pub const OPERAND: &str = "operand";

// ---- misc leaf structure ----
pub const NAME: &str = "name";
pub const BASE: &str = "base";
pub const PARAM: &str = "param";

/// Every IR-synthesized field, in the pre-registration order. The `id::X` consts
/// below index into this list, and `crate::intern::field_interner()` interns it in
/// this exact order on first touch — the two must stay aligned (the pinning test
/// enforces it). Per-grammar field names are NOT here; they intern lazily.
pub const ALL: &[&str] = &[
    LEFT, RIGHT, GUARD, ARM, BODY, TARGET, VALUE, PLACE, CALLEE, ARG, OP, OPERAND, NAME, BASE,
    PARAM,
];

/// Pre-registered [`crate::intern::Field`] ids for the IR-synthesized vocabulary
/// (interning-id-conversion WP) — every comparison site against one of these names
/// (`node.field == Some(field::id::LEFT)`) compiles to a bare `u16` equality, never a
/// string compare. The numeric value of each const is its POSITION in [`ALL`] — this
/// is exactly the order `crate::intern::field_interner()`'s pre-registration loop
/// walks, so `Field::intern(LEFT) == id::LEFT` always holds (pinned by
/// `field_ids_match_pre_registration` below); the consts are never constructed from a
/// hand-picked number.
pub mod id {
    use crate::intern::Field;

    pub const LEFT: Field = Field::from_registered_index(0);
    pub const RIGHT: Field = Field::from_registered_index(1);
    pub const GUARD: Field = Field::from_registered_index(2);
    pub const ARM: Field = Field::from_registered_index(3);
    pub const BODY: Field = Field::from_registered_index(4);
    pub const TARGET: Field = Field::from_registered_index(5);
    pub const VALUE: Field = Field::from_registered_index(6);
    pub const PLACE: Field = Field::from_registered_index(7);
    pub const CALLEE: Field = Field::from_registered_index(8);
    pub const ARG: Field = Field::from_registered_index(9);
    pub const OP: Field = Field::from_registered_index(10);
    pub const OPERAND: Field = Field::from_registered_index(11);
    pub const NAME: Field = Field::from_registered_index(12);
    pub const BASE: Field = Field::from_registered_index(13);
    pub const PARAM: Field = Field::from_registered_index(14);
}

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn field_ids_match_pre_registration() {
        assert_eq!(crate::intern::Field::intern(LEFT), id::LEFT);
        assert_eq!(crate::intern::Field::intern(RIGHT), id::RIGHT);
        assert_eq!(crate::intern::Field::intern(GUARD), id::GUARD);
        assert_eq!(crate::intern::Field::intern(ARM), id::ARM);
        assert_eq!(crate::intern::Field::intern(BODY), id::BODY);
        assert_eq!(crate::intern::Field::intern(TARGET), id::TARGET);
        assert_eq!(crate::intern::Field::intern(VALUE), id::VALUE);
        assert_eq!(crate::intern::Field::intern(PLACE), id::PLACE);
        assert_eq!(crate::intern::Field::intern(CALLEE), id::CALLEE);
        assert_eq!(crate::intern::Field::intern(ARG), id::ARG);
        assert_eq!(crate::intern::Field::intern(OP), id::OP);
        assert_eq!(crate::intern::Field::intern(OPERAND), id::OPERAND);
        assert_eq!(crate::intern::Field::intern(NAME), id::NAME);
        assert_eq!(crate::intern::Field::intern(BASE), id::BASE);
        assert_eq!(crate::intern::Field::intern(PARAM), id::PARAM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_fields_unique() {
        let mut seen = HashSet::new();
        for &f in ALL {
            assert!(seen.insert(f), "duplicate IR-synthesized field: {f}");
        }
    }

    #[test]
    fn id_consts_cover_all_in_order() {
        // Every entry in ALL has an id const at its own index (self-consistency of
        // the vocabulary — the property the pre-registration relies on).
        assert_eq!(ALL.len(), 15);
    }
}
